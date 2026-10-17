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
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use url::Url;

use super::attempt::{Attempt, CallbackError, ClientConfig};
use super::id_token::{self, IdTokenError, Identity};
use super::token::{self, TokenError, TokenSet};
use crate::service::http::Http;

const HELPER_NAME: &str = "cosmic-ext-avd-signin";
const MAX_REPLY_LEN: u64 = 16 * 1024;

/// Environment passed to the helper; everything else is dropped.
const ENV_ALLOWLIST: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "DBUS_SESSION_BUS_ADDRESS",
    "HOME",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_MESSAGES",
    "PATH",
    "GTK_THEME",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
];

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
pub struct Cancel(Arc<Mutex<CancelState>>);

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    stdin: Option<ChildStdin>,
}

impl Cancel {
    pub fn cancel(&self) {
        let mut state = self.0.lock().expect("not poisoned");
        state.cancelled = true;
        // Closing stdin makes the helper close its window and exit.
        state.stdin.take();
    }

    fn is_cancelled(&self) -> bool {
        self.0.lock().expect("not poisoned").cancelled
    }
}

impl std::fmt::Debug for Cancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cancel")
    }
}

/// Where the helper is installed: next to the app (development builds) or
/// in `<prefix>/libexec/cosmic-ext-avd/` (installed builds).
pub fn find_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        dir.join(HELPER_NAME),
        dir.join("../libexec/cosmic-ext-avd").join(HELPER_NAME),
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
    for key in ENV_ALLOWLIST {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    let mut child = crate::sessions::launcher::spawn_retrying(&mut cmd)
        .map_err(|e| SignInError::HelperFailed(e.kind().to_string()))?;
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");

    let request = serde_json::json!({
        "url": url.as_str(),
        "title": crate::fl!("sign-in-window-title"),
    });
    let wrote = writeln!(stdin, "{request}").and_then(|()| stdin.flush());
    {
        // Hand stdin to the canceller; if already cancelled, drop it now.
        let mut state = cancel.0.lock().expect("not poisoned");
        if !state.cancelled {
            state.stdin = Some(stdin);
        }
    }

    let mut line = String::new();
    let read = BufReader::new(std::io::Read::take(stdout, MAX_REPLY_LEN)).read_line(&mut line);
    cancel.0.lock().expect("not poisoned").stdin.take();
    // The helper exits after replying; reap it so it never lingers.
    let _ = child.wait();

    if cancel.is_cancelled() {
        return Err(SignInError::Cancelled);
    }
    if wrote.is_err() || read.is_err() || line.is_empty() {
        return Err(SignInError::HelperFailed("no reply".into()));
    }
    serde_json::from_str(line.trim_end()).map_err(|_| SignInError::HelperFailed("bad reply".into()))
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
