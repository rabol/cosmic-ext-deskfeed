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
    if matches!(
        end,
        SessionEnd::Closed | SessionEnd::Cancelled | SessionEnd::DisconnectedByServer
    ) {
        return EndOutcome::Clear;
    }
    // Not about how the user logged on: the session was taken over, or
    // Windows wouldn't start the app (that comes after logon).
    if was_connected || matches!(end, SessionEnd::Replaced | SessionEnd::AppRefused(_)) {
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
/// Entra sign-in reconnects by itself, a password logon only with a
/// remembered password (`password_saved`); otherwise there's no password
/// to use.
pub(super) fn reconnects(
    end: &SessionEnd,
    was_connected: bool,
    reconnecting: bool,
    tried: Option<&LogonSetting>,
    password_saved: bool,
) -> bool {
    let can = match tried {
        Some(LogonSetting::EntraSignIn) => true,
        Some(other) => other.needs_password() && password_saved,
        None => false,
    };
    if !can {
        return false;
    }
    match end {
        SessionEnd::ConnectionLost => was_connected || reconnecting,
        SessionEnd::DnsFailed
        | SessionEnd::TlsFailed
        | SessionEnd::Failed { .. }
        | SessionEnd::Crashed => reconnecting,
        // Taken over by another connection: reconnecting would take it
        // back, and the two would take turns (ADR 0020). A refused app
        // stays refused.
        SessionEnd::Replaced | SessionEnd::AppRefused(_) => false,
        SessionEnd::Closed
        | SessionEnd::DisconnectedByServer
        | SessionEnd::Cancelled
        | SessionEnd::CertificateNotTrusted(_)
        | SessionEnd::LogonFailed
        | SessionEnd::AccountLockedOut
        | SessionEnd::PasswordExpired
        | SessionEnd::AccountDisabled
        | SessionEnd::AuthenticationFailed => false,
    }
}

/// Where a RemoteApp's windows went when its connection ended (ADR 0020).
///
/// A newer RemoteApp connection of the same account takes over the Windows
/// session, and Windows ends the older one (`Replaced`, or
/// `DisconnectedByServer` on the Windows host tested), but the session and
/// its apps live on in the newer connection. So: the ended connection was
/// connected, ended that way, and `others` (key and attempt of the same
/// account's other RemoteApp connections that are starting or connected)
/// has one started after it (a higher attempt). The newest one wins.
pub(super) fn moved_to<'a>(
    end: &SessionEnd,
    was_connected: bool,
    attempt: u64,
    others: impl IntoIterator<Item = (&'a str, u64)>,
) -> Option<&'a str> {
    if !was_connected || !matches!(end, SessionEnd::Replaced | SessionEnd::DisconnectedByServer) {
        return None;
    }
    others
        .into_iter()
        .filter(|&(_, a)| a > attempt)
        .max_by_key(|&(_, a)| a)
        .map(|(key, _)| key)
}

/// How long to wait before reconnect attempt `attempt` (1-based): 5 s,
/// doubling, at most a minute.
pub(super) fn reconnect_delay(attempt: u32) -> std::time::Duration {
    let secs = 5u64.saturating_mul(1 << attempt.saturating_sub(1).min(6));
    std::time::Duration::from_secs(secs.min(60))
}

/// What an untrusted session host certificate means (ADR 0016).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CertificateStep {
    /// Nothing approved for this desktop yet: ask.
    Ask,
    /// A different certificate than the approved one: block.
    Changed,
}

/// `pinned`: the fingerprint approved for this desktop, if any. FreeRDP
/// was given it, so a refusal with a pin means the certificate changed.
pub(super) fn certificate_step(pinned: Option<&str>, presented: &str) -> CertificateStep {
    match pinned {
        Some(p) if !p.eq_ignore_ascii_case(presented) => CertificateStep::Changed,
        // The same one refused can't happen with a well-formed pin; asking
        // again is safe.
        _ => CertificateStep::Ask,
    }
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
        assert!(reconnects(&lost, true, false, Some(&ENTRA), false));
        // Not connected yet and not reconnecting: a failed connect, shown.
        assert!(!reconnects(&lost, false, false, Some(&ENTRA), false));
        // Password logons only with a remembered password.
        assert!(!reconnects(&lost, true, false, Some(&PASSWORD), false));
        assert!(reconnects(&lost, true, false, Some(&PASSWORD), true));
        // ... and never after Windows rejected it.
        assert!(!reconnects(
            &SessionEnd::LogonFailed,
            false,
            true,
            Some(&PASSWORD),
            true
        ));
        assert!(!reconnects(&lost, true, false, None, false));
        // While reconnecting, network-type failures are tried again ...
        for end in [
            SessionEnd::DnsFailed,
            SessionEnd::Failed { code: 147 },
            SessionEnd::Crashed,
            SessionEnd::ConnectionLost,
        ] {
            assert!(
                reconnects(&end, false, true, Some(&ENTRA), false),
                "{end:?}"
            );
        }
        // ... but never credentials, a normal end or a cancel.
        for end in [
            SessionEnd::Closed,
            SessionEnd::Cancelled,
            SessionEnd::LogonFailed,
            SessionEnd::AuthenticationFailed,
            SessionEnd::AccountLockedOut,
        ] {
            assert!(
                !reconnects(&end, true, true, Some(&ENTRA), false),
                "{end:?}"
            );
        }
    }

    #[test]
    fn an_apps_windows_move_to_the_newer_connection() {
        let others = [("b", 7), ("c", 9), ("old", 2)];
        for end in [SessionEnd::Replaced, SessionEnd::DisconnectedByServer] {
            assert_eq!(moved_to(&end, true, 5, others), Some("c"));
            // Only connections started after it took over.
            assert_eq!(moved_to(&end, true, 9, others), None);
            // Not connected: nothing was open to move.
            assert_eq!(moved_to(&end, false, 5, others), None);
        }
        // Other ends: the windows are gone, not moved.
        for end in [
            SessionEnd::Closed,
            SessionEnd::ConnectionLost,
            SessionEnd::Crashed,
        ] {
            assert_eq!(moved_to(&end, true, 5, others), None, "{end:?}");
        }
        assert_eq!(moved_to(&SessionEnd::Replaced, true, 5, []), None);
    }

    #[test]
    fn a_taken_over_or_refused_session_stays_ended() {
        use crate::sessions::launcher::AppRefusal;
        let entra = Some(&LogonSetting::EntraSignIn);
        for end in [
            SessionEnd::Replaced,
            SessionEnd::AppRefused(AppRefusal::NotFound),
        ] {
            for (was_connected, reconnecting) in [(true, false), (false, true), (true, true)] {
                assert!(
                    !reconnects(&end, was_connected, reconnecting, entra, true),
                    "{end:?}"
                );
            }
            // An error on the row; never "Entra sign-in failed", since it
            // came after logon.
            assert_eq!(
                end_outcome(&end, false, None, entra, 0),
                EndOutcome::Failed,
                "{end:?}"
            );
        }
    }

    #[test]
    fn reconnect_waits_grow_to_a_minute() {
        let secs: Vec<u64> = (1..=8).map(|a| reconnect_delay(a).as_secs()).collect();
        assert_eq!(secs, [5, 10, 20, 40, 60, 60, 60, 60]);
        assert_eq!(reconnect_delay(0).as_secs(), 5);
        assert_eq!(reconnect_delay(u32::MAX).as_secs(), 60);
    }

    #[test]
    fn certificate_changes_block() {
        let a = "aa:".repeat(31) + "aa";
        let b = "bb:".repeat(31) + "bb";
        assert_eq!(certificate_step(None, &a), CertificateStep::Ask);
        assert_eq!(certificate_step(Some(&a), &b), CertificateStep::Changed);
        assert_eq!(
            certificate_step(Some(&a.to_uppercase()), &a),
            CertificateStep::Ask
        );
        let cert = SessionEnd::CertificateNotTrusted(crate::sessions::launcher::CertificateInfo {
            host: String::new(),
            common_name: String::new(),
            subject: String::new(),
            issuer: String::new(),
            valid_from: String::new(),
            valid_to: String::new(),
            fingerprint: a,
        });
        // Never retried by itself.
        assert!(!reconnects(&cert, false, true, Some(&ENTRA), false));
    }
}
