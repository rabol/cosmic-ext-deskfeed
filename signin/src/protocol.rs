// SPDX-License-Identifier: MIT

//! What the helper accepts from the app, which navigations it allows, and
//! what it sends back. No GTK here, so it can be tested directly.
//!
//! Protocol: the app writes one JSON [`Request`] line to the helper's
//! stdin and reads one JSON [`Reply`] line from its stdout. Closing stdin
//! cancels the sign-in. Both are private pipes created by the app.

use serde::{Deserialize, Serialize};
use url::Url;

/// The sign-in must start here (spec §6).
pub const LOGIN_HOST: &str = "login.microsoftonline.com";
/// The only redirect the helper waits for.
#[cfg(test)]
pub const NATIVE_CLIENT: &str = "https://login.microsoftonline.com/common/oauth2/nativeclient";

const MAX_REQUEST_LEN: usize = 16 * 1024;
const MAX_TIMEOUT_MS: u64 = 15 * 60 * 1000;

#[derive(Debug, Deserialize)]
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

#[derive(Debug)]
pub struct ValidRequest {
    pub url: Url,
    pub title: String,
    pub silent_first: bool,
    pub timeout_ms: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RequestError {
    TooLong,
    Json,
    /// Not an HTTPS URL on the Microsoft sign-in host.
    BadStartUrl,
}

pub fn parse_request(line: &str) -> Result<ValidRequest, RequestError> {
    if line.len() > MAX_REQUEST_LEN {
        return Err(RequestError::TooLong);
    }
    let req: Request = serde_json::from_str(line).map_err(|_| RequestError::Json)?;
    let url = Url::parse(&req.url).map_err(|_| RequestError::BadStartUrl)?;
    if url.scheme() != "https"
        || url.host_str().map(str::to_ascii_lowercase).as_deref() != Some(LOGIN_HOST)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RequestError::BadStartUrl);
    }
    let timeout_ms = match req.timeout_ms {
        0 => 5 * 60 * 1000,
        t => t.min(MAX_TIMEOUT_MS),
    };
    let title: String = req
        .title
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect();
    Ok(ValidRequest {
        url,
        title,
        silent_first: req.silent_first,
        timeout_ms,
    })
}

/// What to do with a navigation the web view is about to make.
#[derive(Debug, PartialEq, Eq)]
pub enum Navigation {
    Allow,
    /// Not HTTPS (or not a URL): block it.
    Block,
    /// The sign-in finished; stop and report.
    Finished(Reply),
}

pub fn decide(uri: &str) -> Navigation {
    let Ok(url) = Url::parse(uri) else {
        return Navigation::Block;
    };
    if is_native_client(&url) {
        return Navigation::Finished(reply_from_redirect(&url));
    }
    // Any HTTPS host is allowed, because federated identity providers
    // (ADFS, Okta, …) must work (spec §6). `about:blank` is harmless.
    match url.scheme() {
        "https" => Navigation::Allow,
        "about" if uri == "about:blank" => Navigation::Allow,
        _ => Navigation::Block,
    }
}

fn is_native_client(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().map(str::to_ascii_lowercase).as_deref() == Some(LOGIN_HOST)
        && url.port().is_none()
        && url
            .path()
            .eq_ignore_ascii_case("/common/oauth2/nativeclient")
}

/// The single line the helper writes to stdout.
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
        #[serde(skip_serializing_if = "Option::is_none")]
        error_codes: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
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

fn reply_from_redirect(url: &Url) -> Reply {
    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_codes = None;
    for (k, v) in url.query_pairs() {
        let slot = match k.as_ref() {
            "code" => &mut code,
            "state" => &mut state,
            "error" => &mut error,
            "error_codes" => &mut error_codes,
            _ => continue,
        };
        if slot.is_some() {
            return Reply::Failed {
                reason: "duplicate_parameter".into(),
            };
        }
        *slot = Some(v.into_owned());
    }
    if let Some(error) = error {
        return Reply::Error {
            error: short(&error),
            error_codes: error_codes.map(|c| short(&c)),
            state,
        };
    }
    match (code, state) {
        (Some(code), Some(state)) if !code.is_empty() => Reply::Ok { code, state },
        _ => Reply::Failed {
            reason: "missing_code_or_state".into(),
        },
    }
}

fn short(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ','))
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH: &str =
        "https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize?client_id=x";

    fn req(url: &str) -> String {
        serde_json::json!({ "url": url, "title": "Sign in" }).to_string()
    }

    #[test]
    fn accepts_microsoft_authorize_url() {
        let r = parse_request(&req(AUTH)).unwrap();
        assert_eq!(r.url.as_str(), AUTH);
        assert_eq!(r.timeout_ms, 300_000);
        assert!(!r.silent_first);
    }

    #[test]
    fn rejects_other_start_urls_and_bad_json() {
        for url in [
            "http://login.microsoftonline.com/x",
            "https://login.microsoftonline.com.evil.example/x",
            "https://evil.example/x",
            "https://login.microsoftonline.com:444/x",
            "https://u@login.microsoftonline.com/x",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert_eq!(
                parse_request(&req(url)).unwrap_err(),
                RequestError::BadStartUrl,
                "{url}"
            );
        }
        assert_eq!(parse_request("{").unwrap_err(), RequestError::Json);
        assert_eq!(
            parse_request(r#"{"url":"https://login.microsoftonline.com/","extra":1}"#).unwrap_err(),
            RequestError::Json
        );
        assert_eq!(
            parse_request(&"x".repeat(20_000)).unwrap_err(),
            RequestError::TooLong
        );
    }

    #[test]
    fn caps_timeout_and_cleans_title() {
        let line =
            serde_json::json!({ "url": AUTH, "title": "a\nb\u{7}c", "timeout_ms": u64::MAX })
                .to_string();
        let r = parse_request(&line).unwrap();
        assert_eq!(r.timeout_ms, MAX_TIMEOUT_MS);
        assert_eq!(r.title, "abc");
    }

    #[test]
    fn navigation_rules() {
        assert_eq!(decide(AUTH), Navigation::Allow);
        assert_eq!(
            decide("https://adfs.example.com/adfs/ls"),
            Navigation::Allow
        );
        assert_eq!(decide("about:blank"), Navigation::Allow);
        for uri in [
            "http://adfs.example.com/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,x",
            "ms-appx-web://Microsoft.AAD.BrokerPlugin/x",
            "about:config",
            "not a url",
        ] {
            assert_eq!(decide(uri), Navigation::Block, "{uri}");
        }
    }

    #[test]
    fn redirect_returns_only_code_and_state() {
        let nav = decide(&format!(
            "{NATIVE_CLIENT}?code=abc&state=s1&session_state=zzz&client_info=yyy"
        ));
        assert_eq!(
            nav,
            Navigation::Finished(Reply::Ok {
                code: "abc".into(),
                state: "s1".into()
            })
        );
        let Navigation::Finished(reply) = nav else {
            unreachable!()
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(json, r#"{"status":"ok","code":"abc","state":"s1"}"#);
    }

    #[test]
    fn redirect_errors_keep_short_codes_only() {
        let nav = decide(&format!(
            "{NATIVE_CLIENT}?error=access_denied&error_description=AADSTS53003%3A+anna%40example.com&error_codes=53003&state=s1"
        ));
        assert_eq!(
            nav,
            Navigation::Finished(Reply::Error {
                error: "access_denied".into(),
                error_codes: Some("53003".into()),
                state: Some("s1".into())
            })
        );
    }

    #[test]
    fn lookalike_redirects_are_just_navigation() {
        // Not the real redirect: allowed as a page (HTTPS) but never reported.
        for uri in [
            "https://login.microsoftonline.com.evil.example/common/oauth2/nativeclient?code=x&state=y",
            "https://login.microsoftonline.com/common/oauth2/nativeclientx?code=x&state=y",
            "https://evil.example/common/oauth2/nativeclient?code=x&state=y",
        ] {
            assert_eq!(decide(uri), Navigation::Allow, "{uri}");
        }
        assert_eq!(
            decide("http://login.microsoftonline.com/common/oauth2/nativeclient?code=x&state=y"),
            Navigation::Block
        );
    }

    #[test]
    fn malformed_redirects_fail() {
        for q in [
            "code=a&code=b&state=s",
            "state=s",
            "code=a",
            "code=&state=s",
        ] {
            assert!(
                matches!(
                    decide(&format!("{NATIVE_CLIENT}?{q}")),
                    Navigation::Finished(Reply::Failed { .. })
                ),
                "{q}"
            );
        }
    }
}
