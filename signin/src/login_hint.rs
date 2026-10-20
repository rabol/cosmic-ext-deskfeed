// SPDX-License-Identifier: MIT

//! The account's name as a hint for FreeRDP's own sign-in for the desktop
//! (ADR 0027).
//!
//! FreeRDP 3.32.2 asks for the session host's token with no hint about the
//! account (`client/common/client.c`, the authorize URL has `client_id`,
//! `scope`, `redirect_uri`, `state` and PKCE only), so a browser profile
//! could pick another account. The helper adds `login_hint` with the
//! account's user name, as Microsoft's own libraries do, so Microsoft's
//! page starts with the account selected. Which account finally signs in
//! can't be checked here: FreeRDP redeems the code itself (ADR 0027
//! addendum). The app passes the name in the environment (not a secret).

use url::Url;

pub use common::helper::LOGIN_HINT_ENV;

const MAX_HINT: usize = 256;

/// The hint the app passed, if it's a plausible user name.
pub fn from_env() -> Option<String> {
    std::env::var(LOGIN_HINT_ENV)
        .ok()
        .filter(|h| !h.is_empty() && h.len() <= MAX_HINT && !h.chars().any(char::is_control))
}

/// FreeRDP's sign-in URL with `login_hint` added, when given and not there
/// yet. Anything that isn't a URL comes back as is; the usual checks then
/// refuse it.
pub fn prepare(url: &str, login_hint: Option<&str>) -> String {
    let Ok(mut u) = Url::parse(url) else {
        return url.to_owned();
    };
    if let Some(hint) = login_hint
        && !u.query_pairs().any(|(k, _)| k == "login_hint")
    {
        u.query_pairs_mut().append_pair("login_hint", hint);
    }
    u.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=x&scope=s&state=t";

    #[test]
    fn adds_the_hint_once() {
        let u = Url::parse(&prepare(AUTH, Some("anna@example.com"))).unwrap();
        let pairs: Vec<(String, String)> =
            u.query_pairs().map(|(k, v)| (k.into(), v.into())).collect();
        assert!(pairs.contains(&("login_hint".into(), "anna@example.com".into())));
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
        assert!(
            u.query_pairs()
                .any(|(k, v)| k == "login_hint" && v == "x@y")
        );
        // Without a hint nothing changes; a non-URL stays as it was.
        assert_eq!(prepare(AUTH, None), Url::parse(AUTH).unwrap().as_str());
        assert_eq!(prepare("not a url", Some("a@b")), "not a url");
    }

    #[test]
    fn freerdps_own_parameters_keep_their_bytes() {
        // Shaped like FreeRDP 3.32.2's request (`client/common/client.c`):
        // encoded scope and redirect URI, PKCE. Ours only goes after it.
        let original = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=a85cf173-4192-42f8-81fa-777a763e6e2c&response_type=code&code_challenge=Zx_9-abc&code_challenge_method=S256&scope=ms-device-service%3A%2F%2Ftermsrv.wvd.microsoft.com%2Fname%2Fhost-1%2Fuser_impersonation&redirect_uri=ms-appx-web%3a%2f%2fMicrosoft.AAD.BrokerPlugin%2fa85cf173-4192-42f8-81fa-777a763e6e2c&state=s%2B1";
        let prepared = prepare(original, Some("anna+test@example.com"));
        let query = |u: &str| u.split_once('?').unwrap().1.to_owned();
        assert!(query(&prepared).starts_with(&query(original)), "{prepared}");
        assert!(prepared.ends_with("&login_hint=anna%2Btest%40example.com"));
    }

    #[test]
    fn the_name_matches_the_app() {
        // The app sets it (src/sessions/launcher/args.rs): same name.
        assert_eq!(LOGIN_HINT_ENV, "COSMIC_EXT_DESKFEED_SIGNIN_LOGIN_HINT");
    }
}
