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
/// toward an account lockout (ADR 0005). The password dialog when the user
/// chose a password setting for this desktop (that choice wins, even if
/// the connection file asks for Entra), or after Entra sign-in failed for
/// it in this run.
pub(super) fn first_step(remembered: Option<&LogonSetting>, entra_failed_here: bool) -> FirstStep {
    let wants_entra = matches!(remembered, None | Some(LogonSetting::EntraSignIn));
    if wants_entra && !entra_failed_here {
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
    /// Entra sign-in isn't working here: offer the password next time.
    OfferPassword,
    /// Windows rejected the typed password: say so, the user retries.
    WrongPassword,
    /// Any other error, shown on the row.
    Failed,
}

/// What a session that ended means.
///
/// - `was_connected`: it got connected before it ended. Then the error is
///   not about how the user logged on.
/// - `remembered`: the logon setting saved for this desktop (the one that
///   last worked).
/// - `tried`: the setting this attempt used (`None` if unknown).
/// - `earlier_entra_failures`: Entra attempts on this desktop that failed
///   since it last connected, in this run.
///
/// If Entra has never worked here, one failure offers the password. If it
/// worked before, one failure is treated as temporary, but a second one in
/// a row offers the password, so the user is never stuck with Entra.
pub(super) fn end_outcome(
    end: &SessionEnd,
    was_connected: bool,
    remembered: Option<&LogonSetting>,
    tried: Option<&LogonSetting>,
    earlier_entra_failures: u32,
) -> EndOutcome {
    if matches!(end, SessionEnd::Closed | SessionEnd::Cancelled) {
        return EndOutcome::Clear;
    }
    if was_connected {
        return EndOutcome::Failed;
    }
    match tried {
        Some(LogonSetting::EntraSignIn) => {
            let worked_before = remembered == Some(&LogonSetting::EntraSignIn);
            if !worked_before || earlier_entra_failures >= 1 {
                EndOutcome::OfferPassword
            } else {
                EndOutcome::Failed
            }
        }
        Some(password) if password.needs_password() && *end == SessionEnd::LogonFailed => {
            EndOutcome::WrongPassword
        }
        _ => EndOutcome::Failed,
    }
}

/// Whether an ended session is reconnected by the app (ADR 0014).
///
/// - A connected session that lost its connection starts reconnecting.
/// - During reconnecting, an attempt that failed for a reason a later try
///   may not have (network, timeout, FreeRDP failure) is tried again.
/// - Never after a normal end, a cancel, or anything about credentials:
///   repeating those could lock the account (ADR 0005).
///
/// Only Entra sign-in reconnects by itself; the password isn't kept.
pub(super) fn reconnects(
    end: &SessionEnd,
    was_connected: bool,
    reconnecting: bool,
    tried: Option<&LogonSetting>,
) -> bool {
    if tried != Some(&LogonSetting::EntraSignIn) {
        return false;
    }
    match end {
        SessionEnd::ConnectionLost => was_connected || reconnecting,
        SessionEnd::DnsFailed
        | SessionEnd::TlsFailed
        | SessionEnd::Failed { .. }
        | SessionEnd::Crashed => reconnecting,
        SessionEnd::Closed
        | SessionEnd::Cancelled
        | SessionEnd::LogonFailed
        | SessionEnd::AccountLockedOut
        | SessionEnd::PasswordExpired
        | SessionEnd::AccountDisabled
        | SessionEnd::AuthenticationFailed => false,
    }
}

/// How long to wait before reconnect attempt `attempt` (1-based): 5 s,
/// doubling, at most a minute.
pub(super) fn reconnect_delay(attempt: u32) -> std::time::Duration {
    let secs = 5u64.saturating_mul(1 << attempt.saturating_sub(1).min(6));
    std::time::Duration::from_secs(secs.min(60))
}

/// What a finished sign-in changes besides the signed-in account itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AfterSignIn {
    /// A new account: file the fresh cookie profile under its ID.
    AdoptProfile,
    /// Account `0`'s sign-in window was used to sign in to another account.
    /// That one stays signed out; the profile now holds the other account's
    /// cookies, so it is filed under the other account (account `0` gets a
    /// fresh profile next time).
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

    const ENTRA: LogonSetting = LogonSetting::EntraSignIn;
    const PASSWORD: LogonSetting = LogonSetting::Automatic;

    #[test]
    fn connect_tries_entra_first() {
        assert_eq!(first_step(None, false), FirstStep::Entra);
        assert_eq!(first_step(Some(&ENTRA), false), FirstStep::Entra);
    }

    #[test]
    fn a_chosen_password_setting_sticks() {
        // Review finding 1: the saved choice wins, every time.
        assert_eq!(
            first_step(Some(&LogonSetting::EmptyDomain), false),
            FirstStep::AskPassword(LogonSetting::EmptyDomain)
        );
        let custom = LogonSetting::CustomDomain("CORP".into());
        assert_eq!(
            first_step(Some(&custom), false),
            FirstStep::AskPassword(custom)
        );
    }

    #[test]
    fn after_entra_failed_the_dialog_starts_at_the_standard_password() {
        assert_eq!(first_step(None, true), FirstStep::AskPassword(PASSWORD));
        assert_eq!(
            first_step(Some(&ENTRA), true),
            FirstStep::AskPassword(PASSWORD)
        );
    }

    #[test]
    fn normal_ends_clear_the_row() {
        for end in [SessionEnd::Closed, SessionEnd::Cancelled] {
            for connected in [false, true] {
                for tried in [None, Some(&ENTRA), Some(&PASSWORD)] {
                    assert_eq!(
                        end_outcome(&end, connected, None, tried, 0),
                        EndOutcome::Clear
                    );
                }
            }
        }
    }

    #[test]
    fn errors_after_connecting_are_ordinary() {
        for tried in [Some(&ENTRA), Some(&PASSWORD), None] {
            assert_eq!(
                end_outcome(&SessionEnd::LogonFailed, true, None, tried, 3),
                EndOutcome::Failed
            );
        }
    }

    #[test]
    fn entra_that_never_worked_offers_the_password_at_once() {
        for end in [
            SessionEnd::Failed { code: 131 },
            SessionEnd::LogonFailed,
            SessionEnd::AuthenticationFailed,
        ] {
            assert_eq!(
                end_outcome(&end, false, None, Some(&ENTRA), 0),
                EndOutcome::OfferPassword
            );
        }
    }

    #[test]
    fn entra_that_worked_before_gets_one_more_chance_then_the_password() {
        // Review finding 2: never stuck with Entra.
        let timeout = SessionEnd::Failed { code: 131 };
        assert_eq!(
            end_outcome(&timeout, false, Some(&ENTRA), Some(&ENTRA), 0),
            EndOutcome::Failed
        );
        assert_eq!(
            end_outcome(&timeout, false, Some(&ENTRA), Some(&ENTRA), 1),
            EndOutcome::OfferPassword
        );
    }

    #[test]
    fn wrong_password_only_when_a_password_was_used() {
        // Review findings 3 and 5.
        assert_eq!(
            end_outcome(&SessionEnd::LogonFailed, false, None, Some(&PASSWORD), 0),
            EndOutcome::WrongPassword
        );
        assert_eq!(
            end_outcome(
                &SessionEnd::LogonFailed,
                false,
                Some(&ENTRA),
                Some(&ENTRA),
                0
            ),
            EndOutcome::Failed
        );
        assert_eq!(
            end_outcome(&SessionEnd::LogonFailed, false, None, None, 0),
            EndOutcome::Failed
        );
        assert_eq!(
            end_outcome(
                &SessionEnd::Failed { code: 131 },
                false,
                None,
                Some(&PASSWORD),
                0
            ),
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

    #[test]
    fn reconnects_only_lost_entra_sessions() {
        let lost = SessionEnd::ConnectionLost;
        assert!(reconnects(&lost, true, false, Some(&ENTRA)));
        // Not connected yet and not reconnecting: a failed connect, shown.
        assert!(!reconnects(&lost, false, false, Some(&ENTRA)));
        // Password logons never retry by themselves.
        assert!(!reconnects(&lost, true, false, Some(&PASSWORD)));
        assert!(!reconnects(&lost, true, false, None));
        // While reconnecting, network-type failures are tried again ...
        for end in [
            SessionEnd::DnsFailed,
            SessionEnd::Failed { code: 147 },
            SessionEnd::Crashed,
            SessionEnd::ConnectionLost,
        ] {
            assert!(reconnects(&end, false, true, Some(&ENTRA)), "{end:?}");
        }
        // ... but never credentials, a normal end or a cancel.
        for end in [
            SessionEnd::Closed,
            SessionEnd::Cancelled,
            SessionEnd::LogonFailed,
            SessionEnd::AuthenticationFailed,
            SessionEnd::AccountLockedOut,
        ] {
            assert!(!reconnects(&end, true, true, Some(&ENTRA)), "{end:?}");
        }
    }

    #[test]
    fn reconnect_waits_grow_to_a_minute() {
        let secs: Vec<u64> = (1..=8).map(|a| reconnect_delay(a).as_secs()).collect();
        assert_eq!(secs, [5, 10, 20, 40, 60, 60, 60, 60]);
        assert_eq!(reconnect_delay(0).as_secs(), 5);
        assert_eq!(reconnect_delay(u32::MAX).as_secs(), 60);
    }
}
