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
        other => fl!("error-sign-in-generic", detail = format!("{other:?}")),
    };
    SignInFailure(Some(text))
}

pub(super) fn access_denied(code: Option<String>) -> String {
    match code.filter(|c| !c.is_empty()) {
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
        other => fl!("error-feed", detail = other.to_string()),
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
