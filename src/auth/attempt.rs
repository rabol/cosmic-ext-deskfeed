// SPDX-License-Identifier: MIT

//! One interactive sign-in attempt: authorization code flow with PKCE S256
//! (spec §6).
//!
//! Each attempt has fresh random `state`, `nonce` and PKCE verifier values.
//! The callback is accepted only once, only before the attempt expires, only
//! for the exact redirect URI, and only with the exact `state`.

use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use url::Url;

/// Time beyond the sign-in window's own limit (`Attempt::for_window`).
const WINDOW_SLACK: Duration = Duration::from_mins(1);

/// Upper bound for a callback URL. Real ones are a few kilobytes.
const MAX_CALLBACK_LEN: usize = 16 * 1024;

/// Settings for one identity (spec §4): client ID, authority, scope, redirect.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub client_id: String,
    /// For example `https://login.microsoftonline.com/organizations`.
    pub authority: Url,
    pub scope: String,
    pub redirect_uri: Url,
}

impl ClientConfig {
    /// Microsoft's public Remote Desktop client identity, as used by `FreeRDP`
    /// and the reference app (spec §3, §4).
    pub fn public_remote_desktop() -> Self {
        Self {
            client_id: "a85cf173-4192-42f8-81fa-777a763e6e2c".to_owned(),
            authority: Url::parse("https://login.microsoftonline.com/organizations")
                .expect("static URL"),
            scope: "https://www.wvd.microsoft.com/.default offline_access openid profile"
                .to_owned(),
            redirect_uri: Url::parse(
                "https://login.microsoftonline.com/common/oauth2/nativeclient",
            )
            .expect("static URL"),
        }
    }
}

/// A successful callback. The code is only valid together with the verifier.
pub struct AuthorizationCode {
    pub code: String,
    pub code_verifier: String,
    /// Must be checked against the ID token's `nonce` claim.
    pub nonce: String,
}

impl std::fmt::Debug for AuthorizationCode {
    // Never print the code or verifier (spec §6: no codes in logs).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthorizationCode(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackError {
    /// The attempt was already completed or cancelled.
    NotPending,
    Expired,
    TooLong,
    Malformed,
    /// Not the registered redirect URI (scheme, host, port or path differ).
    WrongRedirect,
    MissingState,
    StateMismatch,
    /// A parameter appeared more than once.
    DuplicateParameter,
    MissingCode,
    /// The identity provider returned an error, e.g. `access_denied` or
    /// `interaction_required`. Only the short codes are kept; descriptions
    /// can contain user data.
    Provider {
        error: String,
        aadsts: Option<String>,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    Pending,
    Completed,
    /// Its lifetime ran out before a callback arrived.
    Expired,
}

pub struct Attempt {
    config: ClientConfig,
    state: String,
    nonce: String,
    code_verifier: String,
    started: Instant,
    lifetime: Duration,
    status: State,
}

impl std::fmt::Debug for Attempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attempt")
            .field("client_id", &self.config.client_id)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Attempt {
    /// An attempt for a sign-in window that may stay open for `window`
    /// (the "Time allowed for signing in" setting): it lives as long, plus
    /// a minute for starting the window and handing back its answer, so a
    /// sign-in finished in time is never refused as expired.
    pub fn for_window(config: ClientConfig, window: Duration) -> Result<Self, getrandom::Error> {
        Self::with_lifetime(config, window + WINDOW_SLACK)
    }

    pub fn with_lifetime(
        config: ClientConfig,
        lifetime: Duration,
    ) -> Result<Self, getrandom::Error> {
        Ok(Self {
            config,
            state: random_token()?,
            nonce: random_token()?,
            // 32 random bytes give a 43-character verifier (RFC 7636 §4.1).
            code_verifier: random_token()?,
            started: Instant::now(),
            lifetime,
            status: State::Pending,
        })
    }

    /// The URL the sign-in web view opens.
    ///
    /// `login_hint` pre-selects the account on later sign-ins, so a browser
    /// session for another account is not picked by mistake.
    pub fn authorize_url(&self, login_hint: Option<&str>) -> Url {
        let mut url = self.config.authority.clone();
        url.path_segments_mut()
            .expect("https authority")
            .pop_if_empty()
            .extend(["oauth2", "v2.0", "authorize"]);
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("client_id", &self.config.client_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", self.config.redirect_uri.as_str())
                .append_pair("scope", &self.config.scope)
                .append_pair("state", &self.state)
                .append_pair("nonce", &self.nonce)
                .append_pair("code_challenge", &code_challenge(&self.code_verifier))
                .append_pair("code_challenge_method", "S256");
            match login_hint {
                Some(hint) => q.append_pair("login_hint", hint),
                None => q.append_pair("prompt", "select_account"),
            };
        }
        url
    }

    /// True if `url` is the registered redirect URI (ignoring the query).
    pub fn is_redirect(&self, url: &Url) -> bool {
        same_endpoint(url, &self.config.redirect_uri)
    }

    /// Validates the callback URL and, on success, ends the attempt.
    /// Any callback, valid or not, after the first one is rejected.
    pub fn complete(&mut self, callback: &str) -> Result<AuthorizationCode, CallbackError> {
        self.complete_at(callback, Instant::now())
    }

    fn complete_at(
        &mut self,
        callback: &str,
        now: Instant,
    ) -> Result<AuthorizationCode, CallbackError> {
        if self.status != State::Pending {
            return Err(CallbackError::NotPending);
        }
        if now.duration_since(self.started) > self.lifetime {
            self.status = State::Expired;
            return Err(CallbackError::Expired);
        }
        // From here on the attempt is used up, whatever the outcome: a
        // second callback (replay or a retry by an attacker) never succeeds.
        self.status = State::Completed;

        if callback.len() > MAX_CALLBACK_LEN {
            return Err(CallbackError::TooLong);
        }
        let url = Url::parse(callback).map_err(|_| CallbackError::Malformed)?;
        if !self.is_redirect(&url) {
            return Err(CallbackError::WrongRedirect);
        }
        if url.fragment().is_some() {
            // We ask for query responses; a fragment means something else
            // produced this URL.
            return Err(CallbackError::Malformed);
        }

        let mut state = None;
        let mut code = None;
        let mut error = None;
        let mut aadsts = None;
        for (key, value) in url.query_pairs() {
            let slot = match key.as_ref() {
                "state" => &mut state,
                "code" => &mut code,
                "error" => &mut error,
                "error_codes" => &mut aadsts,
                _ => continue,
            };
            if slot.replace(value.into_owned()).is_some() {
                return Err(CallbackError::DuplicateParameter);
            }
        }

        let state = state.ok_or(CallbackError::MissingState)?;
        // A plain comparison: its timing can't help an attacker, since the
        // attempt is used up by this one callback, matching or not.
        if state != self.state {
            return Err(CallbackError::StateMismatch);
        }
        if let Some(error) = error {
            return Err(CallbackError::Provider {
                error: short_code(&error),
                aadsts: aadsts.map(|c| short_code(&c)),
            });
        }
        let code = code
            .filter(|c| !c.is_empty())
            .ok_or(CallbackError::MissingCode)?;
        Ok(AuthorizationCode {
            code,
            code_verifier: self.code_verifier.clone(),
            nonce: self.nonce.clone(),
        })
    }
}

/// 32 random bytes, base64url without padding (43 characters).
fn random_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn same_endpoint(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str().map(str::to_ascii_lowercase) == b.host_str().map(str::to_ascii_lowercase)
        && a.port_or_known_default() == b.port_or_known_default()
        && a.path() == b.path()
        && a.username().is_empty()
        && a.password().is_none()
}

/// Keeps only a short machine-readable code (letters, digits, `_`, `,`).
fn short_code(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ','))
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt() -> Attempt {
        Attempt::for_window(
            ClientConfig::public_remote_desktop(),
            Duration::from_mins(5),
        )
        .unwrap()
    }

    fn param(url: &Url, name: &str) -> String {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
            .unwrap()
    }

    fn callback(a: &Attempt, query: &str) -> String {
        format!("{}?{query}", a.config.redirect_uri)
    }

    #[test]
    fn authorize_url_has_pkce_state_and_nonce() {
        let a = attempt();
        let url = a.authorize_url(None);
        assert_eq!(
            url.as_str().split('?').next().unwrap(),
            "https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize"
        );
        assert_eq!(param(&url, "response_type"), "code");
        assert_eq!(param(&url, "code_challenge_method"), "S256");
        assert_eq!(
            param(&url, "code_challenge"),
            code_challenge(&a.code_verifier)
        );
        assert_eq!(param(&url, "state"), a.state);
        assert_eq!(param(&url, "nonce"), a.nonce);
        assert_eq!(param(&url, "prompt"), "select_account");
        assert_eq!(a.code_verifier.len(), 43);
    }

    #[test]
    fn login_hint_replaces_account_picker() {
        let url = attempt().authorize_url(Some("anna@example.com"));
        assert_eq!(param(&url, "login_hint"), "anna@example.com");
        assert!(!url.query_pairs().any(|(k, _)| k == "prompt"));
    }

    #[test]
    fn fresh_values_per_attempt() {
        let (a, b) = (attempt(), attempt());
        assert_ne!(a.state, b.state);
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.code_verifier, b.code_verifier);
    }

    #[test]
    fn rfc7636_challenge_example() {
        // RFC 7636 appendix B.
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn valid_callback_returns_code_once() {
        let mut a = attempt();
        let cb = callback(&a, &format!("code=abc&state={}&session_state=x", a.state));
        let code = a.complete(&cb).unwrap();
        assert_eq!(code.code, "abc");
        assert_eq!(code.code_verifier, a.code_verifier);
        assert_eq!(a.complete(&cb).unwrap_err(), CallbackError::NotPending);
    }

    #[test]
    fn debug_output_hides_code() {
        let mut a = attempt();
        let cb = callback(&a, &format!("code=secretcode&state={}", a.state));
        let code = a.complete(&cb).unwrap();
        assert!(!format!("{code:?}").contains("secretcode"));
    }

    #[test]
    fn rejects_bad_state() {
        let mut a = attempt();
        assert_eq!(
            a.complete(&callback(&a, "code=abc")).unwrap_err(),
            CallbackError::MissingState
        );
        let mut a = attempt();
        assert_eq!(
            a.complete(&callback(&a, "code=abc&state=wrong"))
                .unwrap_err(),
            CallbackError::StateMismatch
        );
        let mut a = attempt();
        let other = attempt();
        assert_eq!(
            a.complete(&callback(&a, &format!("code=abc&state={}", other.state)))
                .unwrap_err(),
            CallbackError::StateMismatch
        );
    }

    #[test]
    fn failed_callback_still_ends_attempt() {
        let mut a = attempt();
        let good = callback(&a, &format!("code=abc&state={}", a.state));
        assert!(a.complete(&callback(&a, "code=abc&state=wrong")).is_err());
        assert_eq!(a.complete(&good).unwrap_err(), CallbackError::NotPending);
    }

    #[test]
    fn rejects_wrong_redirect() {
        for prefix in [
            "https://login.microsoftonline.com/common/oauth2/nativeclient/extra",
            "https://evil.example/common/oauth2/nativeclient",
            "http://login.microsoftonline.com/common/oauth2/nativeclient",
            "https://login.microsoftonline.com:8443/common/oauth2/nativeclient",
            "https://user@login.microsoftonline.com/common/oauth2/nativeclient",
            "https://login.microsoftonline.com.evil.example/common/oauth2/nativeclient",
        ] {
            let mut a = attempt();
            let cb = format!("{prefix}?code=abc&state={}", a.state);
            assert_eq!(
                a.complete(&cb).unwrap_err(),
                CallbackError::WrongRedirect,
                "{prefix}"
            );
        }
    }

    #[test]
    fn host_case_does_not_matter() {
        let mut a = attempt();
        let cb = format!(
            "https://LOGIN.microsoftonline.com/common/oauth2/nativeclient?code=abc&state={}",
            a.state
        );
        assert!(a.complete(&cb).is_ok());
    }

    #[test]
    fn rejects_duplicates_fragment_missing_code_and_garbage() {
        let mut a = attempt();
        let s = a.state.clone();
        assert_eq!(
            a.complete(&callback(&a, &format!("code=a&code=b&state={s}")))
                .unwrap_err(),
            CallbackError::DuplicateParameter
        );
        let mut a = attempt();
        let s = a.state.clone();
        assert_eq!(
            a.complete(&callback(&a, &format!("state={s}&state={s}&code=a")))
                .unwrap_err(),
            CallbackError::DuplicateParameter
        );
        let mut a = attempt();
        let s = a.state.clone();
        assert_eq!(
            a.complete(&format!("{}#x", callback(&a, &format!("code=a&state={s}"))))
                .unwrap_err(),
            CallbackError::Malformed
        );
        let mut a = attempt();
        let s = a.state.clone();
        assert_eq!(
            a.complete(&callback(&a, &format!("code=&state={s}")))
                .unwrap_err(),
            CallbackError::MissingCode
        );
        let mut a = attempt();
        assert_eq!(
            a.complete("not a url").unwrap_err(),
            CallbackError::Malformed
        );
        let mut a = attempt();
        let long = callback(&a, &"x".repeat(MAX_CALLBACK_LEN));
        assert_eq!(a.complete(&long).unwrap_err(), CallbackError::TooLong);
    }

    #[test]
    fn provider_error_keeps_only_short_codes() {
        let mut a = attempt();
        let cb = callback(
            &a,
            &format!(
                "error=access_denied&error_description=AADSTS53003%3A+user+anna%40example.com+blocked&error_codes=53003&state={}",
                a.state
            ),
        );
        assert_eq!(
            a.complete(&cb).unwrap_err(),
            CallbackError::Provider {
                error: "access_denied".into(),
                aadsts: Some("53003".into())
            }
        );
    }

    #[test]
    fn provider_error_requires_matching_state() {
        // A spoofed error must not cancel a real sign-in with a false message.
        let mut a = attempt();
        let cb = callback(&a, "error=access_denied&state=forged");
        assert_eq!(a.complete(&cb).unwrap_err(), CallbackError::StateMismatch);
    }

    #[test]
    fn an_attempt_lasts_as_long_as_its_sign_in_window() {
        let min = Duration::from_mins;
        for window in [min(2), min(10), min(11), min(15)] {
            // Finished just before the window's own limit, and at it.
            for at in [window.saturating_sub(Duration::from_secs(1)), window] {
                let mut a =
                    Attempt::for_window(ClientConfig::public_remote_desktop(), window).unwrap();
                let cb = callback(&a, &format!("code=abc&state={}", a.state));
                assert!(
                    a.complete_at(&cb, a.started + at).is_ok(),
                    "{window:?} at {at:?}"
                );
            }
            // Well after it: expired.
            let mut a = Attempt::for_window(ClientConfig::public_remote_desktop(), window).unwrap();
            let cb = callback(&a, &format!("code=abc&state={}", a.state));
            assert_eq!(
                a.complete_at(&cb, a.started + window + min(2)).unwrap_err(),
                CallbackError::Expired
            );
        }
    }

    #[test]
    fn expired_attempts_reject_callbacks() {
        let mut a = Attempt::with_lifetime(
            ClientConfig::public_remote_desktop(),
            Duration::from_secs(1),
        )
        .unwrap();
        let cb = callback(&a, &format!("code=abc&state={}", a.state));
        let later = a.started + Duration::from_secs(2);
        assert_eq!(
            a.complete_at(&cb, later).unwrap_err(),
            CallbackError::Expired
        );
        assert_eq!(a.complete(&cb).unwrap_err(), CallbackError::NotPending);
    }

    #[test]
    fn random_callbacks_never_break_the_check() {
        let a = attempt();
        let good = callback(&a, &format!("code=abc&state={}&session_state=x", a.state));
        let error = callback(
            &a,
            &format!(
                "error=access_denied&error_description=AADSTS53003&state={}",
                a.state
            ),
        );
        crate::test_util::random_inputs(
            "callback",
            &[good.as_bytes(), error.as_bytes()],
            |input| {
                let mut a = attempt();
                // The sample's state, swapped for this attempt's: a
                // mutation that keeps it can still succeed.
                let input = String::from_utf8_lossy(input).replace(
                    &good[good.find("state=").unwrap_or(0) + 6..][..a.state.len()],
                    &a.state,
                );
                if let Ok(code) = a.complete(&input) {
                    assert_ne!(code.code, "");
                    assert!(input.contains(&a.state));
                }
                // Used up, whatever happened.
                assert!(a.complete(&input).is_err());
            },
        );
    }
}
