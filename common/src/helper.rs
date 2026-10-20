// SPDX-License-Identifier: MIT

//! What the app and the sign-in helper exchange: the request line the app
//! writes to the helper's stdin, the reply line the helper writes back,
//! the proxy file, and the environment variables and folder they share.
//! Only the shapes are here; the helper checks everything it receives
//! (`signin/src/protocol.rs`), and the app checks the reply's code and
//! state (`src/auth/attempt.rs`).

use serde::{Deserialize, Serialize};

/// The account's web view profile, for the helper FreeRDP starts.
pub const PROFILE_ENV: &str = "COSMIC_EXT_DESKFEED_SIGNIN_PROFILE";
/// The sign-in window's title in the user's language, for the helper
/// FreeRDP starts (it has no translations of its own).
pub const TITLE_ENV: &str = "COSMIC_EXT_DESKFEED_SIGNIN_TITLE";
/// The path of the proxy file ([`ProxyFile`]); only the path, since the
/// proxy address may carry a user name and password.
pub const PROXY_FILE_ENV: &str = "COSMIC_EXT_DESKFEED_SIGNIN_PROXY_FILE";
/// The account's user name, for Microsoft's `login_hint` (ADR 0026).
pub const LOGIN_HINT_ENV: &str = "COSMIC_EXT_DESKFEED_SIGNIN_LOGIN_HINT";

/// The folder of the RAM cookie folders, under `$XDG_RUNTIME_DIR`: the
/// helper writes them, the app removes them on sign-out.
pub const JAR_DIR: &str = "cosmic-ext-deskfeed-web";

/// The one line the app writes to the helper's stdin.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// The authorize URL to open.
    pub url: String,
    /// Window title.
    #[serde(default)]
    pub title: String,
    /// `true`: show the window only if the user has to do something
    /// (silent renewal with existing cookies). `false`: show at once.
    #[serde(default)]
    pub silent_first: bool,
    /// 0 means the default (5 minutes).
    #[serde(default)]
    pub timeout_ms: u64,
}

/// The one line the helper writes to stdout.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Reply {
    /// Only `code` and `state` are passed on (spec §6). The app checks
    /// `state`.
    Ok {
        code: String,
        state: String,
    },
    /// The identity provider returned an error. Only short codes.
    Error {
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_codes: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    /// The user closed the window, or the app closed the pipe.
    Cancelled,
    Timeout,
    /// The request was refused or the page could not load (for example a
    /// TLS error).
    Failed {
        reason: String,
    },
}

/// The proxy file the app writes (0600, in a private folder) for the
/// helper.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProxyFile {
    pub uri: String,
    /// Hosts that bypass it, as in `no_proxy`.
    #[serde(default)]
    pub no_proxy: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each reply and its line: the wire format, fixed.
    #[test]
    fn replies_have_a_fixed_line_format() {
        let replies = [
            (
                Reply::Ok {
                    code: "c".into(),
                    state: "s".into(),
                },
                r#"{"status":"ok","code":"c","state":"s"}"#,
            ),
            (
                Reply::Error {
                    error: "access_denied".into(),
                    error_codes: Some("53003".into()),
                    state: Some("s".into()),
                },
                r#"{"status":"error","error":"access_denied","error_codes":"53003","state":"s"}"#,
            ),
            (
                Reply::Error {
                    error: "login_required".into(),
                    error_codes: None,
                    state: None,
                },
                r#"{"status":"error","error":"login_required"}"#,
            ),
            (Reply::Cancelled, r#"{"status":"cancelled"}"#),
            (Reply::Timeout, r#"{"status":"timeout"}"#),
            (
                Reply::Failed {
                    reason: "tls".into(),
                },
                r#"{"status":"failed","reason":"tls"}"#,
            ),
        ];
        for (reply, line) in replies {
            assert_eq!(serde_json::to_string(&reply).unwrap(), line);
            assert_eq!(serde_json::from_str::<Reply>(line).unwrap(), reply);
        }
    }

    #[test]
    fn requests_round_trip_and_refuse_unknown_fields() {
        let request = Request {
            url: "https://login.microsoftonline.com/x".into(),
            title: "Sign in".into(),
            silent_first: true,
            timeout_ms: 1000,
        };
        let line = serde_json::to_string(&request).unwrap();
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(
            (back.url, back.title, back.silent_first, back.timeout_ms),
            (request.url, request.title, true, 1000)
        );
        assert!(serde_json::from_str::<Request>(r#"{"url":"u","extra":1}"#).is_err());
        let proxy: ProxyFile = serde_json::from_str(r#"{"uri":"http://p:8080"}"#).unwrap();
        assert_eq!(
            (proxy.uri.as_str(), proxy.no_proxy),
            ("http://p:8080", None)
        );
    }
}
