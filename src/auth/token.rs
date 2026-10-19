// SPDX-License-Identifier: MIT

//! Token endpoint calls: code exchange and refresh (spec §6).
//!
//! Tokens are kept in memory only; refresh tokens go to the Secret Service
//! (never to files). `Debug` output never shows token values.

use std::fmt;
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use url::Url;

use super::attempt::{AuthorizationCode, ClientConfig};
use crate::service::endpoint::Role;
use crate::service::http::{BearerToken, Http, HttpError};

/// Renew this long before the access token expires.
const EXPIRY_MARGIN: Duration = Duration::from_mins(5);

pub struct TokenSet {
    pub access_token: BearerToken,
    pub refresh_token: Option<Secret>,
    /// Raw ID token; validate with `id_token::validate` before using claims.
    pub id_token: Option<String>,
    pub expires_at: Instant,
    /// The same on the wall clock: `Instant` stops while the computer
    /// sleeps (Linux `CLOCK_MONOTONIC`), so after a night asleep it would
    /// still look fresh.
    pub expires_wall: SystemTime,
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSet")
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("has_id_token", &self.id_token.is_some())
            .finish_non_exhaustive()
    }
}

impl TokenSet {
    pub fn needs_refresh(&self, now: Instant) -> bool {
        now + EXPIRY_MARGIN >= self.expires_at
            || SystemTime::now() + EXPIRY_MARGIN >= self.expires_wall
    }
}

/// A secret string that never appears in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: String) -> Self {
        Self(s)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Why a token request failed, in the categories the UI explains
/// differently (spec §5, §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// Network or server trouble; retrying later may help.
    Transient(String),
    /// The refresh token is no longer valid: sign in again.
    InvalidGrant { aadsts: Option<u32> },
    /// Sign-in needs the user (MFA, consent, policy).
    InteractionRequired { aadsts: Option<u32> },
    /// Administrator consent is required.
    ConsentRequired { aadsts: Option<u32> },
    /// Conditional Access or another policy blocked access.
    Denied { aadsts: Option<u32> },
    /// Anything else the server reported.
    Other { error: String, aadsts: Option<u32> },
    /// The response wasn't what the token endpoint should send.
    Malformed,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: Option<String>,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
    #[serde(default)]
    error_codes: Vec<u32>,
    #[serde(default)]
    suberror: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// The first line of Microsoft's error description: the AADSTS text
/// without the trace and correlation IDs that follow it. Bounded, no
/// control characters.
fn description_first_line(d: &str) -> String {
    let line = d.split(['\r', '\n']).next().unwrap_or_default();
    // Microsoft may put the IDs on the same line (seen 2026-10-17).
    line.split(" Trace ID:")
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(400)
        .collect()
}

pub fn token_url(config: &ClientConfig) -> Url {
    let mut url = config.authority.clone();
    url.path_segments_mut()
        .expect("https authority")
        .pop_if_empty()
        .extend(["oauth2", "v2.0", "token"]);
    url
}

pub fn exchange_code(
    http: &Http,
    config: &ClientConfig,
    code: &AuthorizationCode,
) -> Result<TokenSet, TokenError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("client_id", config.client_id.as_str()),
        ("code", code.code.as_str()),
        ("redirect_uri", config.redirect_uri.as_str()),
        ("code_verifier", code.code_verifier.as_str()),
        ("scope", config.scope.as_str()),
    ];
    request(http, config, &form, None)
}

/// Refreshes the tokens. Microsoft may rotate the refresh token; the caller
/// must store the new one (if any) and only then forget the old one.
pub fn refresh(
    http: &Http,
    config: &ClientConfig,
    refresh_token: &Secret,
) -> Result<TokenSet, TokenError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("client_id", config.client_id.as_str()),
        ("refresh_token", refresh_token.expose()),
        ("scope", config.scope.as_str()),
    ];
    request(http, config, &form, Some(refresh_token))
}

fn request(
    http: &Http,
    config: &ClientConfig,
    form: &[(&str, &str)],
    previous_refresh: Option<&Secret>,
) -> Result<TokenSet, TokenError> {
    let started = Instant::now();
    let started_wall = SystemTime::now();
    let body = match http.post_form(&token_url(config), Role::Token, form) {
        Ok(resp) => resp.body,
        Err(HttpError::Status { status, body }) => return Err(classify_error(status, &body)),
        Err(e) => return Err(TokenError::Transient(e.to_string())),
    };
    let resp: TokenResponse = serde_json::from_slice(&body).map_err(|_| TokenError::Malformed)?;
    if resp.access_token.is_empty()
        || resp
            .token_type
            .as_deref()
            .is_some_and(|t| !t.eq_ignore_ascii_case("bearer"))
    {
        return Err(TokenError::Malformed);
    }
    let lifetime = Duration::from_secs(resp.expires_in.unwrap_or(3600).clamp(60, 24 * 3600));
    Ok(TokenSet {
        access_token: BearerToken::new(resp.access_token),
        // Keep the old refresh token when the server doesn't rotate it.
        refresh_token: resp
            .refresh_token
            .filter(|t| !t.is_empty())
            .map(Secret::new)
            .or_else(|| previous_refresh.cloned()),
        id_token: resp.id_token.filter(|t| !t.is_empty()),
        expires_at: started + lifetime,
        expires_wall: started_wall + lifetime,
    })
}

fn classify_error(status: u16, body: &[u8]) -> TokenError {
    let Ok(err) = serde_json::from_slice::<ErrorResponse>(body) else {
        return if status >= 500 {
            TokenError::Transient(format!("HTTP {status}"))
        } else {
            TokenError::Malformed
        };
    };
    let aadsts = err.error_codes.first().copied();
    // Diagnostics for the sign-in frequency policy (AADSTS70043): its text
    // says when the token was issued and the lifetime the policy allows.
    if aadsts == Some(70043)
        && let Some(d) = &err.error_description
    {
        crate::app_log!("token endpoint: {}", description_first_line(d));
    }
    // AADSTS codes from Microsoft's public error reference.
    let code_is = |codes: &[u32]| aadsts.is_some_and(|c| codes.contains(&c));
    match err.error.as_str() {
        _ if code_is(&[53000, 53001, 53002, 53003, 53004, 530_003]) => {
            TokenError::Denied { aadsts }
        }
        _ if code_is(&[65001, 90094, 90095]) => TokenError::ConsentRequired { aadsts },
        "consent_required" => TokenError::ConsentRequired { aadsts },
        "interaction_required" | "login_required" => TokenError::InteractionRequired { aadsts },
        "invalid_grant" if err.suberror.as_deref() == Some("consent_required") => {
            TokenError::ConsentRequired { aadsts }
        }
        "invalid_grant" if code_is(&[50076, 50079, 50158]) => {
            TokenError::InteractionRequired { aadsts }
        }
        "invalid_grant" => TokenError::InvalidGrant { aadsts },
        "temporarily_unavailable" | "server_error" => TokenError::Transient(err.error),
        "access_denied" | "unauthorized_client" => TokenError::Denied { aadsts },
        other => TokenError::Other {
            error: other
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(64)
                .collect(),
            aadsts,
        },
    }
}

#[cfg(test)]
mod description_tests {
    use super::*;

    #[test]
    fn keeps_only_the_first_line() {
        let d = "AADSTS70043: The refresh token has expired. The token was issued on 2026-10-17T17:55:00Z and the maximum allowed lifetime for this request is 3600.\r\nTrace ID: abc\r\nCorrelation ID: def";
        let line = description_first_line(d);
        assert!(line.ends_with("is 3600."));
        assert!(!line.contains("Trace") && !line.contains("Correlation"));
        // As seen from Microsoft: everything on one line.
        let one = "AADSTS70043: ... lifetime for this request is 300. Trace ID: x Correlation ID: y Timestamp: z";
        assert_eq!(
            description_first_line(one),
            "AADSTS70043: ... lifetime for this request is 300."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::endpoint::EndpointPolicy;
    use std::sync::{Arc, Mutex};

    fn serve(status: &str, body: &str) -> (u16, Arc<Mutex<String>>) {
        crate::test_util::serve_once(status, body)
    }

    fn config(port: u16) -> ClientConfig {
        let mut c = ClientConfig::public_remote_desktop();
        c.authority = Url::parse(&format!("http://127.0.0.1:{port}/organizations")).unwrap();
        c
    }

    fn code() -> AuthorizationCode {
        AuthorizationCode {
            code: "the-code".into(),
            code_verifier: "the-verifier".into(),
            nonce: "n".into(),
        }
    }

    #[test]
    fn exchange_sends_pkce_and_parses_tokens() {
        let (port, seen) = serve(
            "200 OK",
            r#"{"token_type":"Bearer","expires_in":3599,"access_token":"AT","refresh_token":"RT","id_token":"ID"}"#,
        );
        let http = Http::new(EndpointPolicy::local_test(port));
        let tokens = exchange_code(&http, &config(port), &code()).unwrap();
        assert_eq!(tokens.refresh_token.as_ref().unwrap().expose(), "RT");
        assert_eq!(tokens.id_token.as_deref(), Some("ID"));
        assert!(!tokens.needs_refresh(Instant::now()));
        let body = seen.lock().unwrap().clone();
        for part in [
            "grant_type=authorization_code",
            "code=the-code",
            "code_verifier=the-verifier",
            "client_id=a85cf173-4192-42f8-81fa-777a763e6e2c",
            "redirect_uri=https%3A%2F%2Flogin.microsoftonline.com%2Fcommon%2Foauth2%2Fnativeclient",
        ] {
            assert!(body.contains(part), "{part} missing in {body}");
        }
        assert!(!format!("{tokens:?}").contains("RT"));
    }

    #[test]
    fn refresh_keeps_old_token_when_not_rotated() {
        let (port, _) = serve(
            "200 OK",
            r#"{"token_type":"Bearer","expires_in":3599,"access_token":"AT2"}"#,
        );
        let http = Http::new(EndpointPolicy::local_test(port));
        let old = Secret::new("RT-old".into());
        let tokens = refresh(&http, &config(port), &old).unwrap();
        assert_eq!(tokens.refresh_token, Some(old));
    }

    #[test]
    fn errors_are_classified() {
        let cases: [(&str, &'static str, TokenError); 5] = [
            (
                "400 Bad Request",
                r#"{"error":"invalid_grant","error_codes":[70008]}"#,
                TokenError::InvalidGrant {
                    aadsts: Some(70008),
                },
            ),
            (
                "400 Bad Request",
                r#"{"error":"invalid_grant","error_codes":[50076]}"#,
                TokenError::InteractionRequired {
                    aadsts: Some(50076),
                },
            ),
            (
                "400 Bad Request",
                r#"{"error":"invalid_grant","error_codes":[53003]}"#,
                TokenError::Denied {
                    aadsts: Some(53003),
                },
            ),
            (
                "400 Bad Request",
                r#"{"error":"invalid_client","error_codes":[65001]}"#,
                TokenError::ConsentRequired {
                    aadsts: Some(65001),
                },
            ),
            (
                "503 Service Unavailable",
                "oops",
                TokenError::Transient("HTTP 503".into()),
            ),
        ];
        for (status, body, want) in cases {
            let (port, _) = serve(status, body);
            let http = Http::new(EndpointPolicy::local_test(port));
            let got = refresh(&http, &config(port), &Secret::new("x".into())).unwrap_err();
            assert_eq!(got, want, "{body}");
        }
    }

    #[test]
    fn rejects_malformed_success() {
        for body in [
            r#"{"token_type":"Bearer"}"#,
            r#"{"access_token":"","token_type":"Bearer"}"#,
            r#"{"access_token":"x","token_type":"pop"}"#,
        ] {
            let (port, _) = serve("200 OK", body);
            let http = Http::new(EndpointPolicy::local_test(port));
            assert_eq!(
                exchange_code(&http, &config(port), &code()).unwrap_err(),
                TokenError::Malformed,
                "{body}"
            );
        }
    }

    #[test]
    fn token_url_follows_authority() {
        assert_eq!(
            token_url(&ClientConfig::public_remote_desktop()).as_str(),
            "https://login.microsoftonline.com/organizations/oauth2/v2.0/token"
        );
    }
}
