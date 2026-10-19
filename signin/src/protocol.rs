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
/// Longest redirect URL handed back, as the app's own limit
/// (`MAX_CALLBACK_LEN` in `auth/attempt.rs`). Real ones are a few KiB.
const MAX_REDIRECT_LEN: usize = 16 * 1024;
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

/// An HTTPS URL on the Microsoft sign-in host, without port or user info.
pub fn valid_start_url(s: &str) -> Option<Url> {
    let url = Url::parse(s).ok()?;
    (url.scheme() == "https"
        && url.host_str().map(str::to_ascii_lowercase).as_deref() == Some(LOGIN_HOST)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

pub fn parse_request(line: &str) -> Result<ValidRequest, RequestError> {
    if line.len() > MAX_REQUEST_LEN {
        return Err(RequestError::TooLong);
    }
    let req: Request = serde_json::from_str(line).map_err(|_| RequestError::Json)?;
    let url = valid_start_url(&req.url).ok_or(RequestError::BadStartUrl)?;
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

/// The same authorize URL with `prompt=none`, or `None` if it already has
/// a `prompt`. Microsoft: `prompt=none` "ensures that the user isn't
/// presented with any interactive prompt. If the request can't be completed
/// silently by using single-sign on, the Microsoft identity platform returns
/// an `interaction_required` error" (OAuth 2.0 authorization code flow
/// reference, learn.microsoft.com).
pub fn with_prompt_none(url: &str) -> Option<String> {
    let mut url = valid_start_url(url)?;
    if url.query_pairs().any(|(k, _)| k == "prompt") {
        return None;
    }
    url.query_pairs_mut().append_pair("prompt", "none");
    Some(url.into())
}

/// Whether a `prompt=none` attempt needs the normal, visible sign-in:
/// the documented "can't do this silently" errors (`interaction_required`,
/// `login_required`, `consent_required`), or no answer in time.
pub fn needs_interaction(reply: &Reply) -> bool {
    match reply {
        Reply::Error { error, .. } => matches!(
            error.as_str(),
            "interaction_required" | "login_required" | "consent_required"
        ),
        Reply::Timeout => true,
        _ => false,
    }
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

#[cfg(test)]
pub fn decide(uri: &str) -> Navigation {
    decide_for(uri, &RedirectTarget::NativeClient)
}

/// The redirect a sign-in waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectTarget {
    /// `https://login.microsoftonline.com/<tenant>/oauth2/nativeclient`.
    NativeClient,
    /// `ms-appx-web://Microsoft.AAD.BrokerPlugin/<client id>`, which FreeRDP
    /// uses for the AVD gateway token in GUI clients.
    Broker { client_id: String },
}

impl RedirectTarget {
    /// Accepts only the two redirects FreeRDP 3.32.2 uses with Microsoft's
    /// public client. Anything else is refused, so a helper request can't
    /// be turned into "send me the code for some other site".
    pub fn from_freerdp(redirect_uri: &str) -> Option<Self> {
        let url = Url::parse(redirect_uri.trim()).ok()?;
        if url.query().is_some() || url.fragment().is_some() {
            return None;
        }
        if url.scheme() == "https" {
            return is_native_client(&url).then_some(Self::NativeClient);
        }
        if url.scheme().eq_ignore_ascii_case(BROKER_SCHEME)
            && url
                .host_str()
                .is_some_and(|h| h.eq_ignore_ascii_case(BROKER_HOST))
        {
            let id = url.path().trim_start_matches('/');
            if !id.is_empty()
                && id.len() <= 64
                && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            {
                return Some(Self::Broker {
                    client_id: id.to_ascii_lowercase(),
                });
            }
        }
        None
    }

    fn matches(&self, url: &Url) -> bool {
        match self {
            Self::NativeClient => is_native_client(url),
            Self::Broker { client_id } => {
                url.scheme().eq_ignore_ascii_case(BROKER_SCHEME)
                    && url
                        .host_str()
                        .is_some_and(|h| h.eq_ignore_ascii_case(BROKER_HOST))
                    && url
                        .path()
                        .trim_start_matches('/')
                        .eq_ignore_ascii_case(client_id)
            }
        }
    }
}

pub const BROKER_SCHEME: &str = "ms-appx-web";
const BROKER_HOST: &str = "microsoft.aad.brokerplugin";

pub fn decide_for(uri: &str, target: &RedirectTarget) -> Navigation {
    let Ok(url) = Url::parse(uri) else {
        return Navigation::Block;
    };
    if target.matches(&url) {
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

/// `https://login.microsoftonline.com/<tenant>/oauth2/nativeclient`, where
/// `<tenant>` is one plain path segment (FreeRDP may use `common` or the
/// tenant ID).
fn is_native_client(url: &Url) -> bool {
    if url.scheme() != "https"
        || url.host_str().map(str::to_ascii_lowercase).as_deref() != Some(LOGIN_HOST)
        || url.port().is_some()
        || !url.username().is_empty()
    {
        return false;
    }
    let mut parts = url.path().split('/');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(""), Some(tenant), Some(a), Some(b), None)
            if !tenant.is_empty()
                && tenant.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
                && a.eq_ignore_ascii_case("oauth2")
                && b.eq_ignore_ascii_case("nativeclient")
    )
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
        /// The account Entra named (`client_info`), for the helper's own
        /// check (`account.rs`); never sent on.
        #[serde(skip)]
        account: Option<crate::account::Account>,
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
    // The app refuses longer callbacks too (`auth/attempt.rs`); refusing
    // here keeps an oversized one from being copied around at all.
    if url.as_str().len() > MAX_REDIRECT_LEN {
        return Reply::Failed {
            reason: "callback_too_long".into(),
        };
    }
    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_codes = None;
    let mut client_info = None;
    for (k, v) in url.query_pairs() {
        let slot = match k.as_ref() {
            "code" => &mut code,
            "state" => &mut state,
            "error" => &mut error,
            "error_codes" => &mut error_codes,
            "client_info" => &mut client_info,
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
        (Some(code), Some(state)) if !code.is_empty() => Reply::Ok {
            code,
            state,
            account: client_info
                .as_deref()
                .and_then(crate::account::from_client_info),
        },
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
mod silent_tests {
    use super::*;

    const AUTH: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=a&response_type=code&scope=s%20t&redirect_uri=r";

    #[test]
    fn adds_prompt_none_once() {
        let u = with_prompt_none(AUTH).unwrap();
        let url = Url::parse(&u).unwrap();
        let prompts: Vec<_> = url.query_pairs().filter(|(k, _)| k == "prompt").collect();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].1, "none");
        // The other parameters are unchanged.
        assert!(url.query_pairs().any(|(k, v)| k == "scope" && v == "s t"));
        assert_eq!(with_prompt_none(&format!("{AUTH}&prompt=login")), None);
        assert_eq!(with_prompt_none("https://evil.example/authorize"), None);
    }

    #[test]
    fn documented_errors_fall_back_to_interactive() {
        let err = |e: &str| Reply::Error {
            error: e.into(),
            error_codes: None,
            state: None,
        };
        for e in ["interaction_required", "login_required", "consent_required"] {
            assert!(needs_interaction(&err(e)), "{e}");
        }
        assert!(needs_interaction(&Reply::Timeout));
        assert!(!needs_interaction(&err("access_denied")));
        assert!(!needs_interaction(&Reply::Cancelled));
        assert!(!needs_interaction(&Reply::Ok {
            code: "c".into(),
            state: String::new(),
            account: None,
        }));
    }
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
    fn an_oversized_redirect_is_refused_early() {
        let base = "https://login.microsoftonline.com/common/oauth2/nativeclient";
        let ok = Url::parse(&format!("{base}?code=abc&state=s")).unwrap();
        assert!(matches!(reply_from_redirect(&ok), Reply::Ok { .. }));
        // As in the review's harness: a 1 MiB code.
        let huge = Url::parse(&format!("{base}?code={}&state=s", "a".repeat(1024 * 1024))).unwrap();
        assert!(matches!(
            reply_from_redirect(&huge),
            Reply::Failed { reason } if reason == "callback_too_long"
        ));
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
                state: "s1".into(),
                account: None,
            })
        );
        let Navigation::Finished(reply) = nav else {
            unreachable!()
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(json, r#"{"status":"ok","code":"abc","state":"s1"}"#);
    }

    #[test]
    fn redirect_keeps_the_named_account_but_never_sends_it() {
        use base64::Engine as _;
        let info = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"uid":"oid-1","utid":"tid-1"}"#);
        let nav = decide(&format!(
            "{NATIVE_CLIENT}?code=abc&state=s1&client_info={info}"
        ));
        let Navigation::Finished(reply) = nav else {
            panic!("{nav:?}")
        };
        let Reply::Ok { account, .. } = &reply else {
            panic!("{reply:?}")
        };
        assert_eq!(
            account.as_ref(),
            Some(&crate::account::Account {
                oid: "oid-1".into(),
                tid: "tid-1".into()
            })
        );
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(json, r#"{"status":"ok","code":"abc","state":"s1"}"#);
        // Two accounts in one answer: refused, not either one.
        let twice = decide(&format!(
            "{NATIVE_CLIENT}?code=abc&state=s1&client_info={info}&client_info={info}"
        ));
        assert!(
            matches!(twice, Navigation::Finished(Reply::Failed { .. })),
            "{twice:?}"
        );
        // An oversized one names no account (and the code still counts).
        let huge = "A".repeat(4000);
        let nav = decide(&format!(
            "{NATIVE_CLIENT}?code=abc&state=s1&client_info={huge}"
        ));
        assert!(
            matches!(nav, Navigation::Finished(Reply::Ok { account: None, .. })),
            "{nav:?}"
        );
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

    #[test]
    fn freerdp_redirect_targets() {
        assert_eq!(
            RedirectTarget::from_freerdp(
                "https://login.microsoftonline.com/common/oauth2/nativeclient"
            ),
            Some(RedirectTarget::NativeClient)
        );
        assert_eq!(
            RedirectTarget::from_freerdp(
                "https://login.microsoftonline.com/11111111-2222-3333-4444-555555555555/oauth2/nativeclient"
            ),
            Some(RedirectTarget::NativeClient)
        );
        assert_eq!(
            RedirectTarget::from_freerdp(
                "ms-appx-web://Microsoft.AAD.BrokerPlugin/a85cf173-4192-42f8-81fa-777a763e6e2c"
            ),
            Some(RedirectTarget::Broker {
                client_id: "a85cf173-4192-42f8-81fa-777a763e6e2c".into()
            })
        );
        for bad in [
            "https://evil.example/cb",
            "https://login.microsoftonline.com/common/oauth2/authorize",
            "https://login.microsoftonline.com/a/b/oauth2/nativeclient",
            "https://login.microsoftonline.com/common/oauth2/nativeclient?x=1",
            "ms-appx-web://evil.example/a85cf173",
            "ms-appx-web://Microsoft.AAD.BrokerPlugin/",
            "ms-appx-web://Microsoft.AAD.BrokerPlugin/../../x",
            "http://localhost:1234/",
        ] {
            assert_eq!(RedirectTarget::from_freerdp(bad), None, "{bad}");
        }
    }

    #[test]
    fn broker_redirect_is_caught() {
        let target = RedirectTarget::from_freerdp(
            "ms-appx-web://Microsoft.AAD.BrokerPlugin/a85cf173-4192-42f8-81fa-777a763e6e2c",
        )
        .unwrap();
        assert_eq!(
            decide_for(
                "ms-appx-web://microsoft.aad.brokerplugin/A85CF173-4192-42F8-81FA-777A763E6E2C?code=c1&state=s1",
                &target
            ),
            Navigation::Finished(Reply::Ok {
                code: "c1".into(),
                state: "s1".into(),
                account: None,
            })
        );
        // Another broker client ID is not ours.
        assert_eq!(
            decide_for(
                "ms-appx-web://Microsoft.AAD.BrokerPlugin/other?code=c&state=s",
                &target
            ),
            Navigation::Block
        );
        // With the broker target, the nativeclient page is ordinary navigation.
        assert_eq!(
            decide_for(&format!("{NATIVE_CLIENT}?code=c&state=s"), &target),
            Navigation::Allow
        );
    }

    #[test]
    fn tenant_specific_nativeclient_matches() {
        assert!(matches!(
            decide(
                "https://login.microsoftonline.com/organizations/oauth2/nativeclient?code=c&state=s"
            ),
            Navigation::Finished(Reply::Ok { .. })
        ));
    }

    /// The app's request line, every URL the web view navigates to, and
    /// FreeRDP's redirect target, with random and mangled input (ADR 0023,
    /// review U2).
    #[test]
    fn random_requests_and_navigations_never_break_the_rules() {
        let request = req(AUTH);
        let callback = "https://login.microsoftonline.com/common/oauth2/nativeclient?code=abc&state=s&session_state=x";
        let error = "https://login.microsoftonline.com/common/oauth2/nativeclient?error=access_denied&error_codes=53003&state=s";
        let broker =
            "ms-appx-web://Microsoft.AAD.BrokerPlugin/a85cf173-4192-42f8-81fa-777a763e6e2c";
        testkit::random_inputs(
            "helper protocol",
            &[
                request.as_bytes(),
                callback.as_bytes(),
                error.as_bytes(),
                broker.as_bytes(),
            ],
            |input| {
                let text = String::from_utf8_lossy(input);
                // A request is accepted only for Microsoft's sign-in page.
                if let Ok(r) = parse_request(&text) {
                    assert_eq!(r.url.scheme(), "https");
                    assert_eq!(r.url.host_str(), Some("login.microsoftonline.com"));
                }
                let targets = [
                    Some(RedirectTarget::NativeClient),
                    RedirectTarget::from_freerdp(&text),
                    RedirectTarget::from_freerdp(broker),
                ];
                for target in targets.iter().flatten() {
                    match decide_for(&text, target) {
                        // Only a bounded redirect carries a code back, and
                        // only a non-empty one.
                        Navigation::Finished(Reply::Ok { code, state, .. }) => {
                            assert!(text.len() <= MAX_REDIRECT_LEN);
                            assert_ne!(code, "");
                            let _ = state;
                        }
                        // Errors pass on only short, plain codes.
                        Navigation::Finished(Reply::Error {
                            error, error_codes, ..
                        }) => {
                            for s in std::iter::once(&error).chain(error_codes.iter()) {
                                assert!(s.chars().count() <= 64);
                                assert!(
                                    s.chars().all(
                                        |c| c.is_ascii_alphanumeric() || matches!(c, '_' | ',')
                                    )
                                );
                            }
                        }
                        // Anything else the web view may load is HTTPS, as the URL
                        // standard reads it (it drops tabs and line breaks,
                        // so `https\r://…` is HTTPS too).
                        Navigation::Allow => {
                            assert!(
                                Url::parse(&text).is_ok_and(|u| u.scheme() == "https"),
                                "{text:?}"
                            );
                        }
                        Navigation::Finished(_) | Navigation::Block => {}
                    }
                }
            },
        );
    }
}
