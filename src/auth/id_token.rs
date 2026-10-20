// SPDX-License-Identifier: MIT

//! ID token validation (spec §6): RS256 signature against Microsoft's
//! published keys, then issuer, audience, time and nonce checks. Only a
//! validated token's claims are used for the account's identity.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use serde::Deserialize;

use crate::service::endpoint::Role;
use crate::service::http::{Http, HttpError};

/// Allowed clock difference.
const SKEW_SECS: u64 = 5 * 60;
const MAX_TOKEN_LEN: usize = 16 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    #[serde(default)]
    kty: String,
    #[serde(default)]
    kid: String,
    #[serde(default, rename = "use")]
    use_: Option<String>,
    #[serde(default)]
    n: String,
    #[serde(default)]
    e: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

/// The identity of a signed-in account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Tenant ID.
    pub tenant_id: String,
    /// Object ID of the user in that tenant: the stable account key.
    pub object_id: String,
    /// Sign-in name for display (and the password dialog).
    pub username: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdTokenError {
    Malformed,
    UnsupportedAlgorithm,
    UnknownKey,
    BadSignature,
    WrongAudience,
    WrongIssuer,
    Expired,
    NotYetValid,
    NonceMismatch,
    /// A renewed ID token names another tenant or account.
    AccountMismatch,
    MissingClaims,
    KeysUnavailable(String),
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    aud: String,
    iss: String,
    exp: u64,
    #[serde(default)]
    nbf: Option<u64>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    tid: Option<String>,
    #[serde(default)]
    oid: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// Downloads Microsoft's current signing keys.
pub fn fetch_keys(http: &Http) -> Result<Vec<Jwk>, IdTokenError> {
    let url = http.policy().signing_keys_url();
    let resp = http
        .get(&url, Role::SigningKeys, None)
        .map_err(|e: HttpError| IdTokenError::KeysUnavailable(e.to_string()))?;
    let jwks: Jwks = serde_json::from_slice(&resp.body)
        .map_err(|_| IdTokenError::KeysUnavailable("malformed".into()))?;
    Ok(jwks.keys)
}

fn b64(s: &str) -> Result<Vec<u8>, IdTokenError> {
    URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| IdTokenError::Malformed)
}

/// Validates `token` and returns the account identity.
pub fn validate(
    token: &str,
    keys: &[Jwk],
    client_id: &str,
    nonce: &str,
    now_unix: u64,
) -> Result<Identity, IdTokenError> {
    verify(token, keys, client_id, Some(nonce), now_unix)
}

/// Validates the ID token of a token renewal. It comes straight from the
/// token endpoint, without a sign-in attempt, so there is no nonce to
/// compare; instead it must name the same tenant and account as `expected`.
pub fn validate_renewed(
    token: &str,
    keys: &[Jwk],
    client_id: &str,
    now_unix: u64,
    expected: &Identity,
) -> Result<Identity, IdTokenError> {
    let identity = verify(token, keys, client_id, None, now_unix)?;
    if identity.tenant_id != expected.tenant_id || identity.object_id != expected.object_id {
        return Err(IdTokenError::AccountMismatch);
    }
    Ok(identity)
}

fn verify(
    token: &str,
    keys: &[Jwk],
    client_id: &str,
    nonce: Option<&str>,
    now_unix: u64,
) -> Result<Identity, IdTokenError> {
    if token.len() > MAX_TOKEN_LEN {
        return Err(IdTokenError::Malformed);
    }
    let mut parts = token.split('.');
    let (Some(h), Some(p), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(IdTokenError::Malformed);
    };
    let header: Header = serde_json::from_slice(&b64(h)?).map_err(|_| IdTokenError::Malformed)?;
    if header.alg != "RS256" {
        // Rejects `none`, HMAC and anything else.
        return Err(IdTokenError::UnsupportedAlgorithm);
    }
    let kid = header.kid.ok_or(IdTokenError::UnknownKey)?;
    let key = keys
        .iter()
        .find(|k| k.kid == kid && k.kty == "RSA" && k.use_.as_deref().is_none_or(|u| u == "sig"))
        .ok_or(IdTokenError::UnknownKey)?;
    let public = RsaPublicKeyComponents {
        n: b64(&key.n)?,
        e: b64(&key.e)?,
    };
    let signed = &token[..h.len() + 1 + p.len()];
    public
        .verify(&RSA_PKCS1_2048_8192_SHA256, signed.as_bytes(), &b64(s)?)
        .map_err(|_| IdTokenError::BadSignature)?;

    // Signature is good; now the claims.
    let claims: Claims = serde_json::from_slice(&b64(p)?).map_err(|_| IdTokenError::Malformed)?;
    if claims.aud != client_id {
        return Err(IdTokenError::WrongAudience);
    }
    let tenant_id = claims
        .tid
        .filter(|t| is_guid(t))
        .ok_or(IdTokenError::MissingClaims)?;
    if claims.iss != format!("https://login.microsoftonline.com/{tenant_id}/v2.0") {
        return Err(IdTokenError::WrongIssuer);
    }
    in_time(claims.exp, claims.nbf, now_unix)?;
    if let Some(nonce) = nonce
        && claims.nonce.as_deref() != Some(nonce)
    {
        return Err(IdTokenError::NonceMismatch);
    }
    let object_id = claims
        .oid
        .filter(|o| is_guid(o))
        .ok_or(IdTokenError::MissingClaims)?;
    Ok(Identity {
        tenant_id,
        object_id,
        username: claims.preferred_username.filter(|u| u.len() <= 256),
        display_name: claims
            .name
            .map(|n| n.chars().filter(|c| !c.is_control()).take(128).collect()),
    })
}

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` in hex: tenant and object IDs
/// (also how the log spots them).
pub(crate) fn is_guid(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Whether `now` is within the token's lifetime, allowing [`SKEW_SECS`]
/// of clock difference either way. Saturating: values near the end of the
/// range don't overflow.
fn in_time(exp: u64, nbf: Option<u64>, now: u64) -> Result<(), IdTokenError> {
    if exp.saturating_add(SKEW_SECS) < now {
        return Err(IdTokenError::Expired);
    }
    if nbf.is_some_and(|nbf| nbf > now.saturating_add(SKEW_SECS)) {
        return Err(IdTokenError::NotYetValid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Throwaway 2048-bit test key (generated with openssl for these tests).
    const N: &str = "vTqP6d9P4mQwdopSUClRuzefRfrF2e8TFVEn8NwGYyqKbJesN_SKvy1aYXukCjnBzQdM0yJTX3kTJ1DtMsI9vtS7kvNyTBbQ0z33rlchLphuhs1GZ_pdOhuAUPILJy13fbnrAQWVi0t0eJZJL52CQPAu6VGyVDPPYbRJwXhcWQH14XMJv7KvaIe9Eq2nYi56Lw0CsWjO0O2pFeubAsZltlqen3P3Hyaz5WDRMmzhNJ3ZW9LGABzcGgbLnYSpgwZmJgkT13MDG1SYbRewJWcinDsPFqIWk7_a2eqepUbXpCJBgo_YBaVXSAbnPYGS7c5sgQ8N9vUlHxWPKHiikHAZlw";
    // Signed with that key: aud = public client ID, tenant 1111…, exp 1760003600,
    // nonce "test-nonce", oid aaaa…, preferred_username anna@example.com.
    const JWT: &str = concat!(
        "eyJhbGciOiJSUzI1NiIsImtpZCI6InRlc3RraWQiLCJ0eXAiOiJKV1QifQ",
        ".",
        "eyJhdWQiOiJhODVjZjE3My00MTkyLTQyZjgtODFmYS03NzdhNzYzZTZlMmMiLCJpc3MiOiJodHRwczovL2xvZ2luLm1pY3Jvc29mdG9ubGluZS5jb20vMTExMTExMTEtMjIyMi0zMzMzLTQ0NDQtNTU1NTU1NTU1NTU1L3YyLjAiLCJpYXQiOjE3NjAwMDAwMDAsIm5iZiI6MTc2MDAwMDAwMCwiZXhwIjoxNzYwMDAzNjAwLCJub25jZSI6InRlc3Qtbm9uY2UiLCJvaWQiOiJhYWFhYWFhYS1iYmJiLWNjY2MtZGRkZC1lZWVlZWVlZWVlZWUiLCJ0aWQiOiIxMTExMTExMS0yMjIyLTMzMzMtNDQ0NC01NTU1NTU1NTU1NTUiLCJwcmVmZXJyZWRfdXNlcm5hbWUiOiJhbm5hQGV4YW1wbGUuY29tIiwibmFtZSI6IkFubmEgRXhhbXBsZSIsInZlciI6IjIuMCJ9",
        ".",
        "hhxWQprSbMMTHFWilqK65jM159xlFhOoFInGVV5EYQnAPsUoA4EavKnrAiVRAJH82OjkYtfhixA2cRys3gIUIuvF8j5P6rWq6DrQftdBAzXVarzA5mktJ4-lzHAtx2Hn41uWisJbCuvbdpS7FKoosHKvcffEpO1asZXDleZslPBWLSq4En-_QpC48W-a9WZNHySRI3NpLAFcB7y2N8nbx5HjVF9CecxZckvEw63IlBDUqUcQOErymG1BsUDDQXo3fnulXSJMgZMyYrlSfDty_l15_Cqmrz84oWmQ6BHacOElG8VVH5NHzUuqJc_ma6pJ5d4fHVeh9OEpt4IAZtV90Q"
    );
    const CLIENT: &str = "a85cf173-4192-42f8-81fa-777a763e6e2c";
    const NOW: u64 = 1_760_001_000;

    fn keys() -> Vec<Jwk> {
        vec![Jwk {
            kty: "RSA".into(),
            kid: "testkid".into(),
            use_: Some("sig".into()),
            n: N.into(),
            e: "AQAB".into(),
        }]
    }

    #[test]
    fn valid_token_gives_identity() {
        let id = validate(JWT, &keys(), CLIENT, "test-nonce", NOW).unwrap();
        assert_eq!(id.tenant_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(id.object_id, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        assert_eq!(id.username.as_deref(), Some("anna@example.com"));
        assert_eq!(id.display_name.as_deref(), Some("Anna Example"));
    }

    #[test]
    fn renewed_token_must_match_the_account() {
        let expected = validate(JWT, &keys(), CLIENT, "test-nonce", NOW).unwrap();
        assert_eq!(
            validate_renewed(JWT, &keys(), CLIENT, NOW, &expected),
            Ok(expected.clone())
        );
        let mut other = expected.clone();
        other.object_id = "ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee".into();
        assert_eq!(
            validate_renewed(JWT, &keys(), CLIENT, NOW, &other),
            Err(IdTokenError::AccountMismatch)
        );
        // Signature and claims are still checked.
        assert_eq!(
            validate_renewed(JWT, &keys(), "other-client", NOW, &expected),
            Err(IdTokenError::WrongAudience)
        );
    }

    #[test]
    fn claim_checks() {
        assert_eq!(
            validate(JWT, &keys(), "other-client", "test-nonce", NOW),
            Err(IdTokenError::WrongAudience)
        );
        assert_eq!(
            validate(JWT, &keys(), CLIENT, "other-nonce", NOW),
            Err(IdTokenError::NonceMismatch)
        );
        assert_eq!(
            validate(
                JWT,
                &keys(),
                CLIENT,
                "test-nonce",
                1_760_003_600 + SKEW_SECS + 1
            ),
            Err(IdTokenError::Expired)
        );
        assert_eq!(
            validate(
                JWT,
                &keys(),
                CLIENT,
                "test-nonce",
                1_760_000_000 - SKEW_SECS - 1
            ),
            Err(IdTokenError::NotYetValid)
        );
        // Within the allowed clock skew.
        assert!(
            validate(
                JWT,
                &keys(),
                CLIENT,
                "test-nonce",
                1_760_003_600 + SKEW_SECS
            )
            .is_ok()
        );
    }

    #[test]
    fn lifetime_at_the_ends_of_the_range() {
        assert_eq!(in_time(u64::MAX, None, NOW), Ok(()));
        assert_eq!(in_time(u64::MAX, Some(u64::MAX), u64::MAX), Ok(()));
        assert_eq!(
            in_time(u64::MAX, Some(u64::MAX), u64::MAX - SKEW_SECS - 1),
            Err(IdTokenError::NotYetValid)
        );
        assert_eq!(in_time(0, None, u64::MAX), Err(IdTokenError::Expired));
    }

    #[test]
    fn tampered_payload_fails_signature() {
        let mut parts: Vec<&str> = JWT.split('.').collect();
        let payload = String::from_utf8(URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        let evil = URL_SAFE_NO_PAD.encode(payload.replace("anna@example.com", "boss@example.com"));
        parts[1] = &evil;
        assert_eq!(
            validate(&parts.join("."), &keys(), CLIENT, "test-nonce", NOW),
            Err(IdTokenError::BadSignature)
        );
    }

    #[test]
    fn rejects_unsigned_and_hmac_tokens() {
        let parts: Vec<&str> = JWT.split('.').collect();
        for alg in [
            r#"{"alg":"none","kid":"testkid"}"#,
            r#"{"alg":"HS256","kid":"testkid"}"#,
        ] {
            let h = URL_SAFE_NO_PAD.encode(alg);
            let token = format!("{h}.{}.{}", parts[1], parts[2]);
            assert_eq!(
                validate(&token, &keys(), CLIENT, "test-nonce", NOW),
                Err(IdTokenError::UnsupportedAlgorithm)
            );
        }
        let unsigned = format!("{}.{}.", parts[0], parts[1]);
        assert!(validate(&unsigned, &keys(), CLIENT, "test-nonce", NOW).is_err());
    }

    #[test]
    fn unknown_or_wrong_type_key() {
        assert_eq!(
            validate(JWT, &[], CLIENT, "test-nonce", NOW),
            Err(IdTokenError::UnknownKey)
        );
        let mut k = keys();
        k[0].use_ = Some("enc".into());
        assert_eq!(
            validate(JWT, &k, CLIENT, "test-nonce", NOW),
            Err(IdTokenError::UnknownKey)
        );
        let mut k = keys();
        k[0].n = URL_SAFE_NO_PAD.encode([0xab; 256]);
        assert_eq!(
            validate(JWT, &k, CLIENT, "test-nonce", NOW),
            Err(IdTokenError::BadSignature)
        );
    }

    #[test]
    fn malformed_tokens() {
        for t in ["", "a.b", "a.b.c.d", "!!.!!.!!"] {
            assert_eq!(
                validate(t, &keys(), CLIENT, "n", NOW),
                Err(IdTokenError::Malformed),
                "{t}"
            );
        }
        assert_eq!(
            validate(&"a".repeat(MAX_TOKEN_LEN + 1), &keys(), CLIENT, "n", NOW),
            Err(IdTokenError::Malformed)
        );
    }

    #[test]
    fn guid_check() {
        assert!(is_guid("11111111-2222-3333-4444-555555555555"));
        assert!(!is_guid("11111111-2222-3333-4444-55555555555"));
        assert!(!is_guid("11111111x2222-3333-4444-555555555555"));
        assert!(!is_guid("../../../etc/passwd/xxxxxxxxxxxxxxxxx"));
    }

    /// Fetches Microsoft's real signing keys. `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs the internet"]
    fn live_keys_fetch() {
        let http = Http::new(crate::service::endpoint::EndpointPolicy::azure_public());
        let keys = fetch_keys(&http).unwrap();
        assert!(keys.iter().any(|k| k.kty == "RSA" && !k.n.is_empty()));
    }

    #[test]
    fn random_id_tokens_never_validate() {
        crate::test_util::random_inputs("id token", &[JWT.as_bytes()], |input| {
            let token = String::from_utf8_lossy(input);
            let result = validate(&token, &keys(), CLIENT, "test-nonce", NOW);
            // Only the signed token itself is valid.
            assert!(result.is_err() || token == JWT, "{token}");
        });
    }
}
