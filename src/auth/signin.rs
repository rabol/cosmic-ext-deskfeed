// SPDX-License-Identifier: MIT

//! Interactive sign-in with the default identity: the sign-in web view
//! helper (ADR 0007) → callback validation → code exchange → ID token
//! validation.
//!
//! Blocking; run it on `tokio::task::spawn_blocking`. [`Cancel`] stops it
//! from another thread by closing the helper's stdin.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use url::Url;

use super::attempt::{Attempt, CallbackError, ClientConfig};
use super::id_token::{self, IdTokenError, Identity};
use super::token::{self, TokenError, TokenSet};
use crate::service::http::Http;

const HELPER_NAME: &str = "cosmic-ext-deskfeed-signin";
const MAX_REPLY_LEN: u64 = 16 * 1024;
/// How long the sign-in window stays open; sent to the helper.
const HELPER_TIMEOUT: Duration = Duration::from_mins(5);
/// The helper's own timer should end it by then; if it hangs, the app
/// ends it (spec §8: every attempt ends).
const HELPER_DEADLINE: Duration = Duration::from_secs(HELPER_TIMEOUT.as_secs() + 30);

pub struct SignedIn {
    pub identity: Identity,
    pub tokens: TokenSet,
}

impl std::fmt::Debug for SignedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedIn")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInError {
    /// The helper binary isn't installed next to the app.
    HelperMissing,
    /// The helper couldn't start or failed (no display, TLS error, …).
    HelperFailed(String),
    Cancelled,
    Timeout,
    /// The identity provider refused (policy, consent, …).
    Provider {
        error: String,
        aadsts: Option<String>,
    },
    Callback(CallbackError),
    Token(TokenError),
    IdToken(IdTokenError),
}

/// Cancels a running sign-in.
#[derive(Clone, Default)]
pub struct Cancel(Arc<(Mutex<CancelState>, Condvar)>);

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    stdin: Option<ChildStdin>,
    /// The helper's process ID while it runs (not yet reaped).
    helper: Option<u32>,
}

impl Cancel {
    fn state(&self) -> std::sync::MutexGuard<'_, CancelState> {
        self.0.0.lock().expect("not poisoned")
    }

    pub fn cancel(&self) {
        let mut state = self.state();
        state.cancelled = true;
        // Closing stdin makes the helper close its window and exit.
        state.stdin.take();
    }

    /// Cancels, and waits up to `grace` for the helper to exit; then ends
    /// it. Afterwards it writes no more cookies. Blocking.
    pub fn cancel_and_wait(&self, grace: Duration) {
        self.cancel();
        if !self.wait_finished(grace) {
            self.kill();
            self.wait_finished(Duration::from_secs(2));
        }
    }

    fn is_cancelled(&self) -> bool {
        self.state().cancelled
    }

    /// `true` once no helper runs (or none was started).
    fn wait_finished(&self, timeout: Duration) -> bool {
        let guard = self.state();
        let (guard, _) = self
            .0
            .1
            .wait_timeout_while(guard, timeout, |s| s.helper.is_some())
            .expect("not poisoned");
        guard.helper.is_none()
    }

    fn kill(&self) {
        if let Some(pid) = self.state().helper.and_then(|p| i32::try_from(p).ok()) {
            // SAFETY: plain syscall. The ID is our unreaped child's: it is
            // cleared (under this lock) only after `wait` reaped it.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }

    fn started(&self, pid: u32) {
        self.state().helper = Some(pid);
    }

    fn finished(&self) {
        self.state().helper = None;
        self.0.1.notify_all();
    }
}

impl std::fmt::Debug for Cancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cancel")
    }
}

/// Where the helper is installed: next to the app (development builds) or
/// in `<prefix>/libexec/cosmic-ext-deskfeed/` (installed builds).
pub fn find_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        dir.join(HELPER_NAME),
        dir.join("../libexec/cosmic-ext-deskfeed").join(HELPER_NAME),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Reply {
    Ok {
        code: String,
        state: String,
    },
    Error {
        error: String,
        error_codes: Option<String>,
        state: Option<String>,
    },
    Cancelled,
    Timeout,
    Failed {
        reason: String,
    },
}

/// Runs an interactive sign-in. `profile` is the account's private web
/// view directory. `login_hint` pre-selects a known account.
pub fn sign_in(
    http: &Http,
    config: &ClientConfig,
    helper: &Path,
    profile: &Path,
    login_hint: Option<&str>,
    cancel: &Cancel,
) -> Result<SignedIn, SignInError> {
    let mut attempt =
        Attempt::new(config.clone()).map_err(|_| SignInError::HelperFailed("random".into()))?;
    let reply = run_helper(helper, profile, &attempt.authorize_url(login_hint), cancel)?;
    let callback = callback_url(config, &reply)?;
    let code = attempt.complete(callback.as_str()).map_err(|e| match e {
        CallbackError::Provider { error, aadsts } => SignInError::Provider { error, aadsts },
        other => SignInError::Callback(other),
    })?;
    if cancel.is_cancelled() {
        return Err(SignInError::Cancelled);
    }
    let tokens = token::exchange_code(http, config, &code).map_err(SignInError::Token)?;
    let raw_id = tokens
        .id_token
        .as_deref()
        .ok_or(SignInError::IdToken(IdTokenError::MissingClaims))?;
    let keys = id_token::fetch_keys(http).map_err(SignInError::IdToken)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let identity = id_token::validate(raw_id, &keys, &config.client_id, &code.nonce, now)
        .map_err(SignInError::IdToken)?;
    Ok(SignedIn { identity, tokens })
}

fn run_helper(
    helper: &Path,
    profile: &Path,
    url: &Url,
    cancel: &Cancel,
) -> Result<Reply, SignInError> {
    if !helper.is_file() {
        return Err(SignInError::HelperMissing);
    }
    let mut cmd = Command::new(helper);
    cmd.arg("--profile")
        .arg(profile)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::sessions::launcher::pass_environment(&mut cmd, &["GTK_THEME"]);
    let mut child = crate::sessions::launcher::spawn_retrying(&mut cmd)
        .map_err(|e| SignInError::HelperFailed(e.kind().to_string()))?;
    cancel.started(child.id());
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    // Ends a helper that outlives its own timer (for example a hung web
    // view); the reply read below then ends too.
    {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            if !cancel.wait_finished(HELPER_DEADLINE) {
                crate::app_log!("sign-in window: no answer in time, ending it");
                cancel.cancel_and_wait(Duration::ZERO);
            }
        });
    }

    let request = serde_json::json!({
        "url": url.as_str(),
        "title": crate::fl!("sign-in-window-title"),
        "timeout_ms": u64::try_from(HELPER_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
    });
    let wrote = writeln!(stdin, "{request}").and_then(|()| stdin.flush());
    {
        // Hand stdin to the canceller; if already cancelled, drop it now.
        let mut state = cancel.state();
        if !state.cancelled {
            state.stdin = Some(stdin);
        }
    }

    let mut line = String::new();
    let read = BufReader::new(std::io::Read::take(stdout, MAX_REPLY_LEN)).read_line(&mut line);
    cancel.state().stdin.take();
    // The helper exits after replying. Wait for that without reaping, then
    // reap it under the lock, so `kill` never targets a reaped (possibly
    // reused) process ID.
    wait_without_reaping(child.id());
    {
        let _state = cancel.state();
        let _ = child.wait();
    }
    cancel.finished();

    if cancel.is_cancelled() {
        return Err(SignInError::Cancelled);
    }
    if wrote.is_err() || read.is_err() || line.is_empty() {
        return Err(SignInError::HelperFailed("no reply".into()));
    }
    serde_json::from_str(line.trim_end()).map_err(|_| SignInError::HelperFailed("bad reply".into()))
}

/// Blocks until the child `pid` has exited, leaving it unreaped.
fn wait_without_reaping(pid: libc::id_t) {
    loop {
        // SAFETY: plain syscall with a zeroed, writable siginfo.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &raw mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if rc == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return;
        }
    }
}

/// Rebuilds the redirect URL from the helper's reply so the attempt can
/// check state and single use exactly as for any callback.
fn callback_url(config: &ClientConfig, reply: &Reply) -> Result<Url, SignInError> {
    let mut url = config.redirect_uri.clone();
    {
        let mut q = url.query_pairs_mut();
        match reply {
            Reply::Ok { code, state } => {
                q.append_pair("code", code).append_pair("state", state);
            }
            Reply::Error {
                error,
                error_codes,
                state,
            } => {
                q.append_pair("error", error);
                if let Some(c) = error_codes {
                    q.append_pair("error_codes", c);
                }
                if let Some(s) = state {
                    q.append_pair("state", s);
                }
            }
            Reply::Cancelled => return Err(SignInError::Cancelled),
            Reply::Timeout => return Err(SignInError::Timeout),
            Reply::Failed { reason } => {
                return Err(SignInError::HelperFailed(
                    reason
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .take(32)
                        .collect(),
                ));
            }
        }
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::endpoint::EndpointPolicy;
    use std::os::unix::fs::PermissionsExt;

    fn fake_helper(dir: &Path, script: &str) -> PathBuf {
        let path = dir.join("helper.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn tmp() -> PathBuf {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).unwrap();
        let dir =
            std::env::temp_dir().join(format!("avd-signin-test-{:x}", u64::from_le_bytes(bytes)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run(script: &str) -> Result<SignedIn, SignInError> {
        let dir = tmp();
        let helper = fake_helper(&dir, script);
        let http = Http::new(EndpointPolicy::local_test(1));
        let r = sign_in(
            &http,
            &ClientConfig::public_remote_desktop(),
            &helper,
            &dir,
            None,
            &Cancel::default(),
        );
        std::fs::remove_dir_all(dir).unwrap();
        r
    }

    #[test]
    fn forged_state_from_helper_is_rejected() {
        let r = run(r#"read line; echo '{"status":"ok","code":"c","state":"forged"}'"#);
        assert_eq!(
            r.unwrap_err(),
            SignInError::Callback(CallbackError::StateMismatch)
        );
    }

    #[test]
    fn helper_outcomes_map_to_errors() {
        assert_eq!(
            run(r#"read line; echo '{"status":"cancelled"}'"#).unwrap_err(),
            SignInError::Cancelled
        );
        assert_eq!(
            run(r#"read line; echo '{"status":"timeout"}'"#).unwrap_err(),
            SignInError::Timeout
        );
        assert_eq!(
            run(r#"read line; echo '{"status":"failed","reason":"no_display"}'"#).unwrap_err(),
            SignInError::HelperFailed("no_display".into())
        );
        assert_eq!(
            run("exit 1").unwrap_err(),
            SignInError::HelperFailed("no reply".into())
        );
        assert_eq!(
            run("read line; echo garbage").unwrap_err(),
            SignInError::HelperFailed("bad reply".into())
        );
    }

    #[test]
    fn provider_error_needs_matching_state() {
        // The helper can't know the state, so a forged error is a mismatch,
        // not a believable "access denied".
        let r = run(r#"read line; echo '{"status":"error","error":"access_denied","state":"x"}'"#);
        assert_eq!(
            r.unwrap_err(),
            SignInError::Callback(CallbackError::StateMismatch)
        );
    }

    #[test]
    fn helper_gets_only_allowlisted_environment() {
        let dir = tmp();
        let out = dir.join("env.txt");
        let helper = fake_helper(
            &dir,
            &format!(
                r#"env > "{}"; read line; echo '{{"status":"cancelled"}}'"#,
                out.display()
            ),
        );
        // SAFETY: tests in this module don't read this variable concurrently.
        unsafe { std::env::set_var("AVD_TEST_SECRET", "leak") };
        let http = Http::new(EndpointPolicy::local_test(1));
        let _ = sign_in(
            &http,
            &ClientConfig::public_remote_desktop(),
            &helper,
            &dir,
            None,
            &Cancel::default(),
        );
        let env = std::fs::read_to_string(&out).unwrap();
        assert!(!env.contains("AVD_TEST_SECRET"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancel_closes_helper_stdin() {
        let dir = tmp();
        // The fake helper waits for EOF on stdin, like the real one.
        let helper = fake_helper(
            &dir,
            r#"read line; cat > /dev/null; echo '{"status":"cancelled"}'"#,
        );
        let cancel = Cancel::default();
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            c2.cancel();
        });
        let http = Http::new(EndpointPolicy::local_test(1));
        let r = sign_in(
            &http,
            &ClientConfig::public_remote_desktop(),
            &helper,
            &dir,
            None,
            &cancel,
        );
        assert_eq!(r.unwrap_err(), SignInError::Cancelled);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_helper_that_ignores_cancel_is_ended() {
        // Sign-out waits for the sign-in window before deleting cookies; a
        // window that doesn't close is ended after the grace period.
        let dir = tmp();
        let helper = fake_helper(&dir, "read line; exec sleep 300");
        let cancel = Cancel::default();
        let c2 = cancel.clone();
        let (dir2, helper2) = (dir.clone(), helper.clone());
        let signing_in = std::thread::spawn(move || {
            let http = Http::new(EndpointPolicy::local_test(1));
            sign_in(
                &http,
                &ClientConfig::public_remote_desktop(),
                &helper2,
                &dir2,
                None,
                &c2,
            )
        });
        // Until the helper runs.
        while cancel.state().helper.is_none() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let started = std::time::Instant::now();
        cancel.cancel_and_wait(Duration::from_millis(300));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(cancel.wait_finished(Duration::ZERO));
        assert_eq!(
            signing_in.join().unwrap().unwrap_err(),
            SignInError::Cancelled
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_helper() {
        let http = Http::new(EndpointPolicy::local_test(1));
        let r = sign_in(
            &http,
            &ClientConfig::public_remote_desktop(),
            Path::new("/nonexistent/helper"),
            Path::new("/tmp"),
            None,
            &Cancel::default(),
        );
        assert_eq!(r.unwrap_err(), SignInError::HelperMissing);
    }
}
