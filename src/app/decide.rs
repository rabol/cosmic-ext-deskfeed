// SPDX-License-Identifier: MIT

//! The app's decisions as pure functions, so they're tested without a UI:
//! what Connect does first, what an ended session means, and what a
//! finished sign-in changes.

use crate::sessions::launcher::{LogonSetting, SessionEnd};

/// What Connect does once the connection file has arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FirstStep {
    /// Entra sign-in, no password.
    Entra,
    /// The password dialog, preset to this logon setting.
    AskPassword(LogonSetting),
}

/// Entra sign-in first: it needs no password, so trying it can't count
/// toward an account lockout (ADR 0005). The password dialog only when the
/// user chose a password setting for this desktop, or after Entra sign-in
/// failed for it in this run.
pub(super) fn first_step(
    file_requests_entra: bool,
    remembered: Option<&LogonSetting>,
    entra_failed_here: bool,
) -> FirstStep {
    let entra_first =
        file_requests_entra || matches!(remembered, None | Some(LogonSetting::EntraSignIn));
    if entra_first && !entra_failed_here {
        return FirstStep::Entra;
    }
    FirstStep::AskPassword(match remembered {
        Some(LogonSetting::EntraSignIn) | None => LogonSetting::Automatic,
        Some(other) => other.clone(),
    })
}

/// What a finished FreeRDP session means for the desktop's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EndOutcome {
    /// Normal end: clear the status.
    Clear,
    /// Entra sign-in never worked here: offer the password next time.
    OfferPassword,
    /// Windows rejected the password: say so, the user retries.
    WrongPassword,
    /// Any other error, shown on the row.
    Failed,
}

/// `was_connected`: the session got connected before it ended.
/// `entra_worked_before`: Entra sign-in is the remembered setting.
/// `tried`: the logon setting this attempt used.
pub(super) fn end_outcome(
    end: &SessionEnd,
    was_connected: bool,
    entra_worked_before: bool,
    tried: Option<&LogonSetting>,
) -> EndOutcome {
    // Only a session that never got connected says anything about the
    // logon method; once Entra sign-in has worked for a desktop, a failure
    // is more likely temporary.
    let entra_never_worked =
        !was_connected && !entra_worked_before && matches!(tried, Some(LogonSetting::EntraSignIn));
    match end {
        SessionEnd::Closed | SessionEnd::Cancelled => EndOutcome::Clear,
        _ if entra_never_worked => EndOutcome::OfferPassword,
        SessionEnd::LogonFailed => EndOutcome::WrongPassword,
        _ => EndOutcome::Failed,
    }
}

/// What a finished sign-in changes besides the signed-in account itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AfterSignIn {
    /// A new account: file the fresh cookie profile under its ID.
    AdoptProfile,
    /// The window of account `0` was used to sign in to another account:
    /// that one stays signed out, the other is added.
    ResetOther(String),
    /// The same account signed in again.
    Nothing,
}

/// `origin`: the account whose "Sign in" was used (`None`: Add account).
pub(super) fn after_sign_in(origin: Option<&str>, signed_in_id: &str) -> AfterSignIn {
    match origin {
        None => AfterSignIn::AdoptProfile,
        Some(old) if old != signed_in_id => AfterSignIn::ResetOther(old.to_owned()),
        Some(_) => AfterSignIn::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_tries_entra_first() {
        // Nothing remembered: Entra, whatever the connection file says.
        assert_eq!(first_step(false, None, false), FirstStep::Entra);
        assert_eq!(first_step(true, None, false), FirstStep::Entra);
        // Remembered Entra.
        assert_eq!(
            first_step(false, Some(&LogonSetting::EntraSignIn), false),
            FirstStep::Entra
        );
    }

    #[test]
    fn password_when_chosen_or_after_entra_failed() {
        assert_eq!(
            first_step(false, Some(&LogonSetting::EmptyDomain), false),
            FirstStep::AskPassword(LogonSetting::EmptyDomain)
        );
        let custom = LogonSetting::CustomDomain("CORP".into());
        assert_eq!(
            first_step(false, Some(&custom), false),
            FirstStep::AskPassword(custom)
        );
        // Entra failed here: the dialog, preset to the standard password
        // setting, not to Entra again.
        assert_eq!(
            first_step(false, None, true),
            FirstStep::AskPassword(LogonSetting::Automatic)
        );
        assert_eq!(
            first_step(true, Some(&LogonSetting::EntraSignIn), true),
            FirstStep::AskPassword(LogonSetting::Automatic)
        );
        // A file asking for Entra wins over a remembered password setting.
        assert_eq!(
            first_step(true, Some(&LogonSetting::EmptyDomain), false),
            FirstStep::Entra
        );
    }

    #[test]
    fn normal_ends_clear_the_row() {
        for end in [SessionEnd::Closed, SessionEnd::Cancelled] {
            for connected in [false, true] {
                assert_eq!(
                    end_outcome(&end, connected, false, Some(&LogonSetting::EntraSignIn)),
                    EndOutcome::Clear
                );
            }
        }
    }

    #[test]
    fn entra_that_never_worked_offers_the_password() {
        let entra = Some(&LogonSetting::EntraSignIn);
        for end in [
            SessionEnd::Failed { code: 131 },
            SessionEnd::LogonFailed,
            SessionEnd::AuthenticationFailed,
        ] {
            assert_eq!(
                end_outcome(&end, false, false, entra),
                EndOutcome::OfferPassword
            );
        }
    }

    #[test]
    fn entra_that_worked_before_or_connected_is_an_ordinary_error() {
        let entra = Some(&LogonSetting::EntraSignIn);
        let timeout = SessionEnd::Failed { code: 131 };
        // Worked on this desktop before: probably temporary.
        assert_eq!(
            end_outcome(&timeout, false, true, entra),
            EndOutcome::Failed
        );
        // Connected, then failed: not about the logon method.
        assert_eq!(
            end_outcome(&timeout, true, false, entra),
            EndOutcome::Failed
        );
    }

    #[test]
    fn rejected_password_says_so() {
        let pw = Some(&LogonSetting::Automatic);
        assert_eq!(
            end_outcome(&SessionEnd::LogonFailed, false, false, pw),
            EndOutcome::WrongPassword
        );
        assert_eq!(
            end_outcome(&SessionEnd::Failed { code: 131 }, false, false, pw),
            EndOutcome::Failed
        );
    }

    #[test]
    fn sign_in_outcomes() {
        assert_eq!(after_sign_in(None, "a"), AfterSignIn::AdoptProfile);
        assert_eq!(after_sign_in(Some("a"), "a"), AfterSignIn::Nothing);
        assert_eq!(
            after_sign_in(Some("a"), "b"),
            AfterSignIn::ResetOther("a".into())
        );
    }
}
