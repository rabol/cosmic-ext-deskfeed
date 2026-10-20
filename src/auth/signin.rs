// SPDX-License-Identifier: MIT

//! Interactive sign-in with the default identity: the sign-in web view
//! helper (ADR 0007) → callback validation → code exchange → ID token
//! validation.
//!
//! Blocking; run it on `tokio::task::spawn_blocking`. [`Cancel`] stops it
//! from another thread by closing the helper's stdin.

use std::io::{BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::helper::{Reply, Request};
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
use url::Url;

use super::attempt::{Attempt, CallbackError, ClientConfig};
use super::id_token::{self, IdTokenError, Identity};
use super::token::{self, TokenError, TokenSet};
use crate::service::http::Http;

const HELPER_NAME: &str = "cosmic-ext-deskfeed-signin";
const MAX_REPLY_LEN: u64 = 16 * 1024;
/// The sign-in window's time is a setting ("Time allowed for signing in"),
/// sent to the helper. Its own timer should end it by then; if it hangs,
/// the app ends it this much later (spec §8: every attempt ends).
#[cfg(not(test))]
const HELPER_GRACE: Duration = Duration::from_secs(30);
/// Short in tests, so the time limit can be tested.
#[cfg(test)]
const HELPER_GRACE: Duration = Duration::from_millis(300);

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
    /// The helper couldn't start or failed.
    HelperFailed(HelperProblem),
    /// No secure random numbers for the sign-in request.
    NoRandom,
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

/// Why the sign-in helper failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperProblem {
    /// The app's proxy couldn't be handed over.
    Proxy,
    /// It couldn't be started (the I/O error's kind).
    Start(String),
    /// It ended without an answer.
    NoReply,
    /// Its answer wasn't the protocol's.
    BadReply,
    /// The app's background task for it stopped.
    Task,
    /// The helper's own reasons (`signin/src/protocol.rs`): the sign-in
    /// page couldn't connect securely, or there's no display.
    Tls,
    NoDisplay,
    /// Any other reason it gave: short, plain characters only.
    Other(String),
}

impl HelperProblem {
    /// A reason from the helper's answer.
    fn reported(reason: &str) -> Self {
        match reason {
            "tls_error" => Self::Tls,
            "no_display" => Self::NoDisplay,
            other => Self::Other(
                other
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .take(32)
                    .collect(),
            ),
        }
    }
}

/// Cancels a running sign-in.
#[derive(Clone, Default)]
pub struct Cancel(Arc<(Mutex<CancelState>, Condvar)>);

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    stdin: Option<ChildStdin>,
    /// The helper runs (started, not yet reaped).
    running: bool,
    /// The helper's pidfd, to end it. It always refers to that process,
    /// even once reaped, so a reused process ID is never signalled.
    pidfd: Option<OwnedFd>,
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
            .wait_timeout_while(guard, timeout, |s| s.running)
            .expect("not poisoned");
        !guard.running
    }

    fn kill(&self) {
        if let Some(pidfd) = &self.state().pidfd {
            let _ = pidfd_send_signal(pidfd, Signal::KILL);
        }
    }

    fn started(&self, pidfd: Option<OwnedFd>) {
        let mut state = self.state();
        state.running = true;
        state.pidfd = pidfd;
    }

    fn finished(&self) {
        let mut state = self.state();
        state.running = false;
        state.pidfd = None;
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
    .find(|p| crate::sessions::launcher::is_executable(p))
}

/// Runs an interactive sign-in. `profile` is the account's private web
/// view directory. `login_hint` pre-selects a known account.
pub fn sign_in(
    http: &Http,
    config: &ClientConfig,
    helper: &Path,
    profile: &Path,
    login_hint: Option<&str>,
    time: Duration,
    cancel: &Cancel,
) -> Result<SignedIn, SignInError> {
    // As long as the window may stay open: a fixed 10 minutes refused
    // sign-ins that finished in minutes 11–15.
    let mut attempt =
        Attempt::for_window(config.clone(), time).map_err(|_| SignInError::NoRandom)?;
    let reply = run_helper(
        helper,
        profile,
        &attempt.authorize_url(login_hint),
        time,
        cancel,
    )?;
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
    time: Duration,
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
    // The app's proxy, from a private file; removed when this
    // sign-in ends.
    let proxy = crate::sessions::launcher::HelperProxy::new()
        .map_err(|_| SignInError::HelperFailed(HelperProblem::Proxy))?;
    proxy.apply(&mut cmd);
    let mut child = crate::sessions::launcher::spawn_tied(cmd)
        .map_err(|e| SignInError::HelperFailed(HelperProblem::Start(e.kind().to_string())))?;
    // Opened before anything can reap the helper, so it is this process.
    let pidfd = pidfd_open(Pid::from_child(&child), PidfdFlags::empty())
        .inspect_err(|e| crate::app_log!("sign-in window: no pidfd ({e}); it can't be ended early"))
        .ok();
    cancel.started(pidfd);
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    // Ends a helper that outlives its own timer (for example a hung web
    // view); the reply read below then ends too.
    {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            if !cancel.wait_finished(time + HELPER_GRACE) {
                crate::app_log!("sign-in window: no answer in time, ending it");
                cancel.cancel_and_wait(Duration::ZERO);
            }
        });
    }

    let request = Request {
        url: url.to_string(),
        title: crate::fl!("sign-in-window-title"),
        silent_first: false,
        timeout_ms: u64::try_from(time.as_millis()).unwrap_or(u64::MAX),
    };
    let wrote = serde_json::to_string(&request)
        .map_err(std::io::Error::other)
        .and_then(|line| writeln!(stdin, "{line}"))
        .and_then(|()| stdin.flush());
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
    // The helper exits after replying (or is ended through its pidfd).
    let _ = child.wait();
    cancel.finished();

    if cancel.is_cancelled() {
        return Err(SignInError::Cancelled);
    }
    if wrote.is_err() || read.is_err() || line.is_empty() {
        return Err(SignInError::HelperFailed(HelperProblem::NoReply));
    }
    serde_json::from_str(line.trim_end())
        .map_err(|_| SignInError::HelperFailed(HelperProblem::BadReply))
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
                return Err(SignInError::HelperFailed(HelperProblem::reported(reason)));
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

    fn run(script: &str) -> Result<SignedIn, SignInError> {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let helper = fake_helper(dir, script);
        let http = Http::new(EndpointPolicy::local_test(1));
        sign_in(
            &http,
            &ClientConfig::public_remote_desktop(),
            &helper,
            dir,
            None,
            Duration::from_mins(5),
            &Cancel::default(),
        )
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
            SignInError::HelperFailed(HelperProblem::NoDisplay)
        );
        assert_eq!(
            run("exit 1").unwrap_err(),
            SignInError::HelperFailed(HelperProblem::NoReply)
        );
        assert_eq!(
            run("read line; echo garbage").unwrap_err(),
            SignInError::HelperFailed(HelperProblem::BadReply)
        );
    }

    #[test]
    fn the_helpers_reasons_are_read_and_cleaned() {
        assert_eq!(HelperProblem::reported("tls_error"), HelperProblem::Tls);
        assert_eq!(
            HelperProblem::reported("no_display"),
            HelperProblem::NoDisplay
        );
        // Anything else is kept short and plain, for the log.
        assert_eq!(
            HelperProblem::reported("bad thing\n<b>!"),
            HelperProblem::Other("badthingb".into())
        );
        assert_eq!(
            HelperProblem::reported(&"x".repeat(100)),
            HelperProblem::Other("x".repeat(32))
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
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let out = dir.join("env.txt");
        let helper = fake_helper(
            dir,
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
            dir,
            None,
            Duration::from_mins(5),
            &Cancel::default(),
        );
        let env = std::fs::read_to_string(&out).unwrap();
        assert!(!env.contains("AVD_TEST_SECRET"));
    }

    #[test]
    fn cancel_closes_helper_stdin() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // The fake helper waits for EOF on stdin, like the real one.
        let helper = fake_helper(
            dir,
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
            dir,
            None,
            Duration::from_mins(5),
            &cancel,
        );
        assert_eq!(r.unwrap_err(), SignInError::Cancelled);
    }

    #[test]
    fn a_helper_that_ignores_cancel_is_ended() {
        // Sign-out waits for the sign-in window before deleting cookies; a
        // window that doesn't close is ended after the grace period.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let helper = fake_helper(dir, "read line; exec sleep 300");
        let cancel = Cancel::default();
        let c2 = cancel.clone();
        let (dir2, helper2) = (dir.to_path_buf(), helper.clone());
        let signing_in = std::thread::spawn(move || {
            let http = Http::new(EndpointPolicy::local_test(1));
            sign_in(
                &http,
                &ClientConfig::public_remote_desktop(),
                &helper2,
                &dir2,
                None,
                Duration::from_mins(5),
                &c2,
            )
        });
        // Until the helper runs.
        while !cancel.state().running {
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
    }

    /// A helper that outlives its own time limit (a hung web view) is
    /// ended, and the sign-in ends with it (spec §8: every attempt ends).
    #[test]
    fn a_helper_past_its_time_is_ended() {
        let tmp = tempfile::tempdir().unwrap();
        let helper = fake_helper(tmp.path(), "read line; exec sleep 300");
        let dir = tmp.path().to_path_buf();
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(sign_in(
                &Http::new(EndpointPolicy::local_test(1)),
                &ClientConfig::public_remote_desktop(),
                &helper,
                &dir,
                None,
                Duration::from_millis(100),
                &Cancel::default(),
            ));
        });
        let result = result
            .recv_timeout(Duration::from_secs(5))
            .expect("the helper wasn't ended");
        assert_eq!(result.unwrap_err(), SignInError::Cancelled);
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
            Duration::from_mins(5),
            &Cancel::default(),
        );
        assert_eq!(r.unwrap_err(), SignInError::HelperMissing);
    }
}
