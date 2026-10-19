// SPDX-License-Identifier: MIT

//! User-facing texts for sign-in, feed and session errors.

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;
use crate::sessions::launcher::AppRefusal;

pub(super) fn describe_sign_in_error(e: SignInError) -> SignInFailure {
    let text = match e {
        SignInError::Cancelled => return SignInFailure(None),
        SignInError::HelperMissing => fl!("error-helper-missing"),
        SignInError::Timeout => fl!("error-sign-in-timeout"),
        SignInError::Provider { error, aadsts } => match error.as_str() {
            "access_denied" => access_denied(aadsts),
            "consent_required" => fl!("error-consent-required"),
            _ => fl!("error-provider", code = error),
        },
        SignInError::Token(TokenError::Denied { aadsts }) => {
            access_denied(aadsts.map(|c| c.to_string()))
        }
        SignInError::Token(TokenError::ConsentRequired { .. }) => fl!("error-consent-required"),
        SignInError::Token(TokenError::Transient(_)) => fl!("error-network"),
        SignInError::HelperFailed(reason) if reason == "tls_error" => fl!("error-tls"),
        SignInError::HelperFailed(reason) if reason == "no_display" => fl!("error-no-display"),
        // Spec §12: technical details go to the details area, which is
        // the log (View → Diagnostics shows it).
        other => {
            crate::app_log!("sign-in failed: {other:?}");
            fl!("error-sign-in-generic")
        }
    };
    SignInFailure(Some(text))
}

pub(super) fn access_denied(code: Option<String>) -> String {
    match code.filter(|c| !c.is_empty()) {
        // Conditional Access wants a compliant (53000) or domain-joined
        // (53001) device (spec §6: explain device compliance).
        Some(code) if code == "53000" || code == "53001" => {
            fl!("error-device-not-compliant", code = code)
        }
        Some(code) => fl!("error-access-denied-code", code = code),
        None => fl!("error-access-denied"),
    }
}

pub(super) fn describe_fetch_error(e: &FetchError) -> String {
    match e {
        FetchError::Http(HttpError::Status { status: 401, .. }) => fl!("error-session-expired"),
        FetchError::Http(HttpError::Status { status: 403, .. }) => fl!("error-forbidden"),
        FetchError::Http(
            HttpError::HostNotFound | HttpError::ConnectionFailed | HttpError::Timeout,
        ) => {
            fl!("error-network")
        }
        FetchError::Http(HttpError::Tls) => fl!("error-tls"),
        other => {
            crate::app_log!("loading desktops failed: {other}");
            fl!("error-feed")
        }
    }
}

pub(super) fn describe_session_end(end: &SessionEnd) -> String {
    match end {
        SessionEnd::AccountLockedOut => fl!("error-account-locked"),
        SessionEnd::PasswordExpired => fl!("error-password-expired"),
        SessionEnd::AccountDisabled => fl!("error-account-disabled"),
        SessionEnd::AuthenticationFailed => fl!("error-auth-failed"),
        SessionEnd::DnsFailed => fl!("error-network"),
        SessionEnd::ConnectionLost => fl!("error-connection-lost"),
        SessionEnd::CertificateNotTrusted(_) => fl!("error-certificate-untrusted"),
        SessionEnd::TlsFailed => fl!("error-tls"),
        SessionEnd::Crashed => fl!("error-freerdp-crashed"),
        SessionEnd::Failed { code } => fl!("error-session-failed", code = code.to_string()),
        SessionEnd::LogonFailed => fl!("error-sign-in-rejected"),
        SessionEnd::Replaced => fl!("error-session-replaced"),
        SessionEnd::ProxyAuthRefused => fl!("error-proxy-password"),
        SessionEnd::WrongAccount => fl!("error-wrong-account"),
        SessionEnd::AppRefused(why) => match why {
            AppRefusal::NotAllowed => fl!("error-app-not-allowed"),
            AppRefusal::NotFound => fl!("error-app-not-found"),
            AppRefusal::SessionLocked => fl!("error-app-session-locked"),
            AppRefusal::Other => fl!("error-app-refused"),
        },
        SessionEnd::Closed | SessionEnd::Cancelled | SessionEnd::DisconnectedByServer => {
            String::new()
        }
    }
}

/// A fetch failed right after a fresh renewal: a refusal (401) now means
/// the account must sign in again; anything else is shown.
pub(super) fn refused_or_shown(e: FetchError) -> StepError {
    match e {
        FetchError::Http(HttpError::Status { status: 401, .. }) => {
            StepError::SignInAgain(fl!("error-session-expired"))
        }
        e => StepError::Show(describe_fetch_error(&e)),
    }
}

pub(super) fn refused_or_shown_http(e: HttpError) -> StepError {
    refused_or_shown(FetchError::Http(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_compliance_is_explained() {
        for code in ["53000", "53001"] {
            let text = access_denied(Some(code.into()));
            assert!(text.contains("devices it manages"), "{text}");
            assert!(text.contains(code), "{text}");
        }
        assert!(!access_denied(Some("53003".into())).contains("devices it manages"));
        assert!(!access_denied(None).contains("AADSTS"));
    }
}
