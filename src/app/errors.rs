// SPDX-License-Identifier: MIT

//! User-facing texts for sign-in, feed and session errors.

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;
use crate::accounts::{NotSaved, RenewError};
use crate::auth::signin::HelperProblem;
use crate::secrets::SecretsError;
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
        SignInError::HelperFailed(HelperProblem::Tls) => fl!("error-tls"),
        SignInError::HelperFailed(HelperProblem::NoDisplay) => fl!("error-no-display"),
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

/// The text for an ended session, with an explanation for failures the
/// launcher has no category for: FreeRDP's own error name and how far the
/// connection got, from its log lines; a bare "code 131" tells the user
/// nothing. The code stays in the text, for support.
pub(super) fn describe_ended(end: &SessionEnd, log: &[String]) -> String {
    match end {
        SessionEnd::Failed { code } => explain_failure(*code, log),
        other => describe_session_end(other),
    }
}

fn explain_failure(code: i32, log: &[String]) -> String {
    let names = error_names(log);
    // The server's reason first (`ERRINFO_`), then FreeRDP's last error.
    let known = names
        .iter()
        .filter(|n| n.starts_with("ERRINFO_"))
        .chain(names.iter().rev().filter(|n| n.starts_with("ERRCONNECT_")))
        .find_map(|n| cause_of(n).map(|why| (n.clone(), why)));
    let (name, why) = match known {
        Some((name, why)) => (Some(name), why),
        None => (names.last().cloned(), cause_from_stage(log)),
    };
    let code = code.to_string();
    match name {
        Some(name) => fl!("error-failed-named", why = why, code = code, name = name),
        None => fl!("error-failed-code", why = why, code = code),
    }
}

/// FreeRDP's error names in the log lines (`ERRCONNECT_…` from
/// `libfreerdp/core/errconnect.c`, `ERRINFO_…` from `errinfo.c`), in order.
fn error_names(log: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for line in log {
        for word in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if (word.starts_with("ERRCONNECT_") || word.starts_with("ERRINFO_"))
                && word.len() <= 64
                && !names.iter().any(|n| n == word)
            {
                names.push(word.to_owned());
            }
        }
    }
    names
}

/// Plain words for the errors that say something the user can act on.
fn cause_of(name: &str) -> Option<String> {
    let rest = name
        .strip_prefix("ERRCONNECT_")
        .or_else(|| name.strip_prefix("ERRINFO_"))?;
    Some(match rest {
        "DNS_ERROR" | "DNS_NAME_NOT_FOUND" => fl!("cause-dns"),
        "CONNECT_FAILED" | "CONNECT_TRANSPORT_FAILED" | "MCS_CONNECT_INITIAL_ERROR" => {
            fl!("cause-network")
        }
        "TLS_CONNECT_FAILED" => fl!("error-tls"),
        "SECURITY_NEGO_CONNECT_FAILED" | "HYBRID_REQUIRED_BY_SERVER" => fl!("cause-security"),
        "AUTHENTICATION_FAILED"
        | "NO_OR_MISSING_CREDENTIALS"
        | "ACCESS_DENIED"
        | "ACCOUNT_RESTRICTION"
        | "LOGON_TYPE_NOT_GRANTED"
        | "INSUFFICIENT_PRIVILEGES"
        | "SERVER_INSUFFICIENT_PRIVILEGES"
        | "SERVER_DENIED_CONNECTION" => fl!("cause-refused"),
        "ACTIVATION_TIMEOUT" => fl!("error-host-timed-out"),
        "TARGET_BOOTING" => fl!("cause-booting"),
        "CONNECT_CANCELLED" => fl!("cause-cancelled"),
        "PRE_CONNECT_FAILED" | "POST_CONNECT_FAILED" => fl!("cause-client"),
        "IDLE_TIMEOUT" => fl!("cause-idle"),
        "LOGON_TIMEOUT" => fl!("cause-logon-timeout"),
        "RPC_INITIATED_DISCONNECT" | "RPC_INITIATED_DISCONNECT_BYUSER" | "LOGOFF_BY_USER" => {
            fl!("cause-ended-by-server")
        }
        _ if rest.starts_with("LICENSE_") => fl!("cause-license"),
        _ => return None,
    })
}

/// How far the connection got, from FreeRDP's connection steps
/// (`CONNECTION_STATE_…`; a final `--> CONNECTION_STATE_INITIAL` is the
/// teardown, not a step).
fn cause_from_stage(log: &[String]) -> String {
    let last = log
        .iter()
        .rev()
        .filter_map(|l| {
            l.split_once(" --> CONNECTION_STATE_")
                .map(|(_, s)| s.trim())
        })
        .find(|s| *s != "INITIAL");
    match last {
        None | Some("NEGO") => fl!("stage-reaching"),
        Some(s) if s.starts_with("AAD") || s.starts_with("NLA") || s.starts_with("RDSTLS") => {
            fl!("stage-signing-in")
        }
        Some("ACTIVE") => fl!("stage-running"),
        Some(_) => fl!("stage-setting-up"),
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
        SessionEnd::HostTimedOut => fl!("error-host-timed-out"),
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

/// A keyring problem, as the detail of a message.
pub(super) fn keyring_detail(e: &SecretsError) -> String {
    match e {
        SecretsError::Dismissed => fl!("keyring-dismissed"),
        SecretsError::NoAnswer => fl!("keyring-no-answer"),
        SecretsError::Malformed => fl!("keyring-malformed"),
        SecretsError::Unavailable(_) => fl!("keyring-unavailable"),
    }
}

/// The notice when a sign-in couldn't be saved.
pub(super) fn describe_not_saved(e: &NotSaved) -> String {
    let detail = match e {
        NotSaved::NoRefreshToken => "no refresh token".to_owned(),
        NotSaved::Keyring(e) => keyring_detail(e),
    };
    fl!("notice-not-saved", detail = detail)
}

/// Restoring a saved sign-in at start failed. `Show` keeps the saved
/// sign-in for another try.
pub(super) fn describe_restore_error(e: RenewError) -> StepError {
    match e {
        RenewError::Network => StepError::Show(fl!("error-restore-network")),
        RenewError::SignInFrequency => StepError::SignInAgain(fl!("error-sign-in-frequency")),
        RenewError::SignedOut => StepError::SignInAgain(fl!("error-session-expired")),
        RenewError::Revoked | RenewError::Refused => {
            StepError::SignInAgain(fl!("error-saved-sign-in-expired"))
        }
    }
}

/// Renewing a signed-in account's tokens failed (loading desktops,
/// connecting).
impl From<RenewError> for StepError {
    fn from(e: RenewError) -> Self {
        match e {
            RenewError::Network => Self::Show(fl!("error-generic")),
            RenewError::SignInFrequency => Self::SignInAgain(fl!("error-sign-in-frequency")),
            RenewError::SignedOut | RenewError::Revoked | RenewError::Refused => {
                Self::SignInAgain(fl!("error-session-expired"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_problems_get_their_own_words() {
        let text = |p| {
            describe_sign_in_error(SignInError::HelperFailed(p))
                .0
                .unwrap()
        };
        assert_eq!(text(HelperProblem::Tls), fl!("error-tls"));
        assert_eq!(text(HelperProblem::NoDisplay), fl!("error-no-display"));
        // Anything else: the general text; the details go to the log.
        for other in [
            HelperProblem::NoReply,
            HelperProblem::Other("something_new".into()),
        ] {
            assert_eq!(text(other), fl!("error-sign-in-generic"));
        }
        assert_eq!(describe_sign_in_error(SignInError::Cancelled).0, None);
    }

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

    /// The log lines a failed session leaves (as kept by the launcher).
    fn log(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| (*l).to_owned()).collect()
    }

    const STEP: &str = "[DEBUG][com.freerdp.core.rdp] - [rdp_client_transition_to_state][0x1]: ";

    #[test]
    fn a_failure_explains_itself_and_keeps_the_code() {
        // Code 131 with FreeRDP's error name, as seen on the owner's host.
        let text = describe_ended(
            &SessionEnd::Failed { code: 131 },
            &log(&[
                &format!("{STEP}CONNECTION_STATE_NEGO --> CONNECTION_STATE_AAD"),
                "[ERROR][com.freerdp.core] - [rdp_client_wait_for_activation]: ERRCONNECT_ACTIVATION_TIMEOUT [0x0002001C]",
                &format!("{STEP}CONNECTION_STATE_AAD --> CONNECTION_STATE_INITIAL"),
            ]),
        );
        assert!(text.contains("didn't finish signing you in"), "{text}");
        assert!(
            text.contains("131") && text.contains("ERRCONNECT_ACTIVATION_TIMEOUT"),
            "{text}"
        );
        assert!(text.contains("View → Diagnostics"), "{text}");

        // The server's reason (logged at INFO) wins over FreeRDP's own.
        let text = describe_ended(
            &SessionEnd::Failed { code: 131 },
            &log(&[
                "[INFO][com.freerdp.core] - [rdp_print_errinfo]: ERRINFO_SERVER_DENIED_CONNECTION (0x0000000B)",
                "[ERROR][com.freerdp.core] - ERRCONNECT_CONNECT_TRANSPORT_FAILED [0x0002000D]",
            ]),
        );
        assert!(text.contains("refused the sign-in"), "{text}");
        assert!(text.contains("ERRINFO_SERVER_DENIED_CONNECTION"), "{text}");
    }

    #[test]
    fn an_unknown_failure_says_how_far_it_got() {
        let failed = SessionEnd::Failed { code: 147 };
        let at = |state: &str, extra: &[&str]| {
            let mut lines = vec![format!(
                "{STEP}CONNECTION_STATE_INITIAL --> CONNECTION_STATE_NEGO"
            )];
            if !state.is_empty() {
                lines.push(format!(
                    "{STEP}CONNECTION_STATE_NEGO --> CONNECTION_STATE_{state}"
                ));
            }
            lines.extend(extra.iter().map(|l| (*l).to_owned()));
            lines.push(format!(
                "{STEP}CONNECTION_STATE_X --> CONNECTION_STATE_INITIAL"
            ));
            describe_ended(&failed, &lines)
        };
        assert!(at("", &[]).contains("before signing in"));
        assert!(at("AAD", &[]).contains("signing you in"));
        assert!(at("NLA", &[]).contains("signing you in"));
        assert!(at("LICENSING", &[]).contains("setting up your session"));
        assert!(at("ACTIVE", &[]).contains("after the session had started"));
        // An error name with no explanation of its own is still shown.
        let text = at(
            "LICENSING",
            &["[ERROR][x] - ERRCONNECT_SOMETHING_NEW [0x1]"],
        );
        assert!(text.contains("setting up your session"), "{text}");
        assert!(
            text.contains("ERRCONNECT_SOMETHING_NEW") && text.contains("147"),
            "{text}"
        );
        // Nothing in the log: still an explanation and the code.
        let text = describe_ended(&failed, &[]);
        assert!(
            text.contains("before signing in") && text.contains("147"),
            "{text}"
        );
    }

    #[test]
    fn other_ends_keep_their_own_messages() {
        assert_eq!(
            describe_ended(
                &SessionEnd::LogonFailed,
                &log(&["ERRCONNECT_LOGON_FAILURE"])
            ),
            describe_session_end(&SessionEnd::LogonFailed)
        );
    }

    /// The texts restore and renew errors showed before the wording moved
    /// here from `accounts`.
    #[test]
    fn renewal_errors_are_worded_for_where_they_happen() {
        let shown = |e: StepError| match e {
            StepError::Show(t) => ("retry", t),
            StepError::SignInAgain(t) => ("sign in", t),
        };
        // Restoring a saved sign-in at start.
        let restore = |e| shown(describe_restore_error(e));
        assert_eq!(
            restore(RenewError::Network),
            ("retry", fl!("error-restore-network"))
        );
        assert_eq!(
            restore(RenewError::Revoked),
            ("sign in", fl!("error-saved-sign-in-expired"))
        );
        assert_eq!(
            restore(RenewError::Refused),
            ("sign in", fl!("error-saved-sign-in-expired"))
        );
        assert_eq!(
            restore(RenewError::SignInFrequency),
            ("sign in", fl!("error-sign-in-frequency"))
        );
        assert_eq!(
            restore(RenewError::SignedOut),
            ("sign in", fl!("error-session-expired"))
        );
        // Renewing while signed in.
        let renew = |e: RenewError| shown(e.into());
        assert_eq!(renew(RenewError::Network), ("retry", fl!("error-generic")));
        assert_eq!(
            renew(RenewError::SignInFrequency),
            ("sign in", fl!("error-sign-in-frequency"))
        );
        for e in [
            RenewError::Revoked,
            RenewError::Refused,
            RenewError::SignedOut,
        ] {
            assert_eq!(renew(e), ("sign in", fl!("error-session-expired")));
        }
    }

    #[test]
    fn unsaved_sign_ins_are_explained() {
        assert_eq!(
            describe_not_saved(&NotSaved::Keyring(SecretsError::Unavailable("x".into()))),
            fl!("notice-not-saved", detail = fl!("keyring-unavailable"))
        );
        assert_eq!(
            describe_not_saved(&NotSaved::NoRefreshToken),
            fl!("notice-not-saved", detail = "no refresh token")
        );
    }
}
