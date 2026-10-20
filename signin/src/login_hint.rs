// SPDX-License-Identifier: MIT

//! Which account FreeRDP's sign-in for the desktop uses.
//!
//! FreeRDP 3.32.2 asks for the session host's token with no hint about the
//! account (`client/common/client.c`, the authorize URL has `client_id`,
//! `scope`, `redirect_uri`, `state` and PKCE only), and nothing checks who
//! signed in: FreeRDP redeems the code itself. The account's cookie profile
//! makes the right account likely, but "Use another account" on the page
//! could pick a different one.
//!
//! So the helper adds what Microsoft's own libraries send: `login_hint` (the
//! account's user name, which pre-selects it) and `client_info=1` (Entra
//! then names the account in its answer, as `uid.utid` in base64url JSON).
//! An answer naming a different account than Deskfeed's is refused. One
//! without `client_info` is let through and noted: it can't be confirmed.
//! The app passes the account in the environment (identifiers, not
//! secrets).

use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use url::Url;

/// The account's object ID and tenant ID, as `<oid>.<tid>`.
pub const ACCOUNT_ENV: &str = "COSMIC_EXT_DESKFEED_SIGNIN_ACCOUNT";
/// The account's user name, for `login_hint`.
pub const LOGIN_HINT_ENV: &str = "COSMIC_EXT_DESKFEED_SIGNIN_LOGIN_HINT";

const MAX_CLIENT_INFO: usize = 2048;
const MAX_HINT: usize = 256;

/// An Entra account: object ID and tenant ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub oid: String,
    pub tid: String,
}

/// What the app expects: the account, and its user name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expected {
    pub account: Option<Account>,
    pub login_hint: Option<String>,
}

impl Expected {
    pub fn from_env() -> Self {
        let get = |name| std::env::var(name).ok();
        let account = get(ACCOUNT_ENV).and_then(|v| {
            let parsed = parse_account(&v);
            if parsed.is_none() {
                // Never silently off: the log says so.
                eprintln!("cosmic-ext-deskfeed-signin: account check off: unreadable account");
            }
            parsed
        });
        Self {
            account,
            login_hint: get(LOGIN_HINT_ENV).filter(|h| {
                !h.is_empty() && h.len() <= MAX_HINT && !h.chars().any(char::is_control)
            }),
        }
    }
}

/// `<oid>.<tid>`: two IDs of plain characters.
fn parse_account(v: &str) -> Option<Account> {
    let (oid, tid) = v.split_once('.')?;
    let id = |s: &str| {
        (!s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
            .then(|| s.to_ascii_lowercase())
    };
    Some(Account {
        oid: id(oid)?,
        tid: id(tid)?,
    })
}

/// FreeRDP's sign-in URL with `login_hint` (when given and not there yet)
/// and `client_info=1` added. Anything that isn't a URL comes back as is;
/// the usual checks then refuse it.
pub fn prepare(url: &str, login_hint: Option<&str>) -> String {
    let Ok(mut u) = Url::parse(url) else {
        return url.to_owned();
    };
    let has = |u: &Url, key: &str| u.query_pairs().any(|(k, _)| k == key);
    if let Some(hint) = login_hint
        && !has(&u, "login_hint")
    {
        u.query_pairs_mut().append_pair("login_hint", hint);
    }
    if !has(&u, "client_info") {
        u.query_pairs_mut().append_pair("client_info", "1");
    }
    u.into()
}

/// The URL again with `prompt=select_account`: Microsoft's page shows the
/// account list instead of reusing the remembered one. Used after the
/// silent attempt came back as another account. A URL that already has a
/// `prompt` stays as it is.
pub fn with_select_account(url: &str) -> String {
    let Ok(mut u) = Url::parse(url) else {
        return url.to_owned();
    };
    if !u.query_pairs().any(|(k, _)| k == "prompt") {
        u.query_pairs_mut().append_pair("prompt", "select_account");
    }
    u.into()
}

/// The account in Entra's `client_info` (base64url JSON with `uid`, the
/// object ID, and `utid`, the tenant ID); `None` if it isn't one.
pub fn from_client_info(v: &str) -> Option<Account> {
    if v.len() > MAX_CLIENT_INFO {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(v.trim_end_matches('='))
        .or_else(|_| URL_SAFE.decode(v))
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    parse_account(&format!(
        "{}.{}",
        json.get("uid")?.as_str()?,
        json.get("utid")?.as_str()?
    ))
}

/// How the account in the answer compares with the expected one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Same,
    Different,
    /// Nothing to compare: no expected account, or none in the answer.
    Unknown,
}

pub fn check(expected: Option<&Account>, got: Option<&Account>) -> Check {
    match (expected, got) {
        (Some(e), Some(g)) if e == g => Check::Same,
        (Some(_), Some(_)) => Check::Different,
        _ => Check::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=x&scope=s&state=t";

    fn info(object: &str, tenant: &str) -> String {
        URL_SAFE_NO_PAD.encode(format!(r#"{{"uid":"{object}","utid":"{tenant}"}}"#))
    }

    #[test]
    fn adds_the_hint_and_asks_for_client_info_once() {
        let u = Url::parse(&prepare(AUTH, Some("anna@example.com"))).unwrap();
        let pairs: Vec<(String, String)> =
            u.query_pairs().map(|(k, v)| (k.into(), v.into())).collect();
        assert!(pairs.contains(&("login_hint".into(), "anna@example.com".into())));
        assert!(pairs.contains(&("client_info".into(), "1".into())));
        assert!(
            pairs.contains(&("state".into(), "t".into())),
            "the rest stays"
        );
        // Not twice, and FreeRDP's own hint wins.
        let again = prepare(&prepare(AUTH, Some("x@y")), Some("other@y"));
        let u = Url::parse(&again).unwrap();
        assert_eq!(
            u.query_pairs().filter(|(k, _)| k == "login_hint").count(),
            1
        );
        assert_eq!(
            u.query_pairs().filter(|(k, _)| k == "client_info").count(),
            1
        );
        assert!(
            u.query_pairs()
                .any(|(k, v)| k == "login_hint" && v == "x@y")
        );
        // Without a hint, only client_info; a non-URL stays as it was.
        assert!(!prepare(AUTH, None).contains("login_hint"));
        assert_eq!(prepare("not a url", Some("a@b")), "not a url");
    }

    #[test]
    fn reads_and_compares_the_account() {
        let me = Account {
            oid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            tid: "11111111-2222-3333-4444-555555555555".into(),
        };
        let mine = from_client_info(&info(&me.oid, &me.tid)).unwrap();
        assert_eq!(mine, me);
        // Upper case and padding are the same account.
        let padded = URL_SAFE.encode(format!(
            r#"{{"uid":"{}","utid":"{}"}}"#,
            me.oid.to_uppercase(),
            me.tid
        ));
        assert_eq!(from_client_info(&padded).as_ref(), Some(&me));
        let other =
            from_client_info(&info("ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee", &me.tid)).unwrap();
        assert_eq!(check(Some(&me), Some(&mine)), Check::Same);
        assert_eq!(check(Some(&me), Some(&other)), Check::Different);
        assert_eq!(check(Some(&me), None), Check::Unknown);
        assert_eq!(check(None, Some(&other)), Check::Unknown);
        for bad in [
            "",
            "!!!",
            &URL_SAFE_NO_PAD.encode("{}"),
            &URL_SAFE_NO_PAD.encode(r#"{"uid":"a b","utid":"c"}"#),
        ] {
            assert_eq!(from_client_info(bad), None, "{bad}");
        }
        assert_eq!(parse_account("oid.tid").map(|a| a.oid), Some("oid".into()));
        assert_eq!(parse_account("no-dot"), None);
    }

    #[test]
    fn freerdps_own_parameters_keep_their_bytes() {
        // Shaped like FreeRDP 3.32.2's request (`client/common/client.c`):
        // encoded scope and redirect URI, PKCE. Ours only go after it.
        let original = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=a85cf173-4192-42f8-81fa-777a763e6e2c&response_type=code&code_challenge=Zx_9-abc&code_challenge_method=S256&scope=ms-device-service%3A%2F%2Ftermsrv.wvd.microsoft.com%2Fname%2Fhost-1%2Fuser_impersonation&redirect_uri=ms-appx-web%3a%2f%2fMicrosoft.AAD.BrokerPlugin%2fa85cf173-4192-42f8-81fa-777a763e6e2c&state=s%2B1";
        let prepared = prepare(original, Some("anna+test@example.com"));
        let query = |u: &str| u.split_once('?').unwrap().1.to_owned();
        assert!(query(&prepared).starts_with(&query(original)), "{prepared}");
        assert!(prepared.ends_with("&login_hint=anna%2Btest%40example.com&client_info=1"));
        let select = with_select_account(&prepared);
        assert!(select.starts_with(&prepared) && select.ends_with("&prompt=select_account"));
        assert_eq!(with_select_account(&select), select, "once");
    }

    #[test]
    fn names_match_the_app() {
        // The app sets these (src/sessions/launcher/args.rs): same names.
        assert_eq!(ACCOUNT_ENV, "COSMIC_EXT_DESKFEED_SIGNIN_ACCOUNT");
        assert_eq!(LOGIN_HINT_ENV, "COSMIC_EXT_DESKFEED_SIGNIN_LOGIN_HINT");
    }

    #[test]
    fn random_client_info_never_breaks_the_reader() {
        let sample = info(
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "11111111-2222-3333-4444-555555555555",
        );
        testkit::random_inputs("client_info", &[sample.as_bytes()], |input| {
            if let Some(a) = from_client_info(&String::from_utf8_lossy(input)) {
                assert!(!a.oid.is_empty() && a.oid.len() <= 64);
                assert!(!a.tid.is_empty() && a.tid.len() <= 64);
            }
        });
    }
}
