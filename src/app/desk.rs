// SPDX-License-Identifier: MIT

//! Everything Deskfeed tracks about one desktop or app, in one place.
//!
//! Before, each fact had its own map keyed by resource (connection state,
//! the starting and running FreeRDP, reconnect, notes, pending password, …)
//! and every handler had to remember to update all of them. A forgotten one
//! was a bug. Now a resource has one [`Desk`], and the moments that touch
//! many facts at once (an attempt ending, sign-out) are methods here.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::model::{Connection, Launching, Reconnect, Running};
use crate::auth::token::Secret;
use crate::sessions::launcher::{self, LogonSetting};
use crate::sessions::rdpfile::ConnectionFile;

/// One desktop or app (by resource key).
// Independent facts about one resource; a bitflag type would only hide them.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
pub(super) struct Desk {
    /// Its row's state; `None`: idle (Connect).
    pub(super) connection: Option<Connection>,
    /// FreeRDP starting in the background.
    pub(super) launching: Option<Launching>,
    pub(super) running: Option<Running>,
    /// Being reconnected after a lost connection.
    pub(super) reconnect: Option<Reconnect>,
    /// Its FreeRDP was stopped because connecting took too long.
    pub(super) timed_out: bool,
    /// Why the running session lacks something the user turned on.
    pub(super) note: Option<String>,
    /// A RemoteApp whose windows another app's connection shows (that
    /// app's key, ADR 0020).
    pub(super) moved_to: Option<String>,
    /// Connected from the tray: a failure opens the window.
    pub(super) from_tray: bool,
    /// Logon setting and connection file of an attempt not yet connected.
    pub(super) pending_logon: Option<(LogonSetting, Arc<ConnectionFile>)>,
    /// A password to remember once the connection works.
    pub(super) pending_password: Option<Secret>,
    /// This attempt uses a remembered password.
    pub(super) used_saved_password: bool,
    /// Why the last attempt failed, for the next password dialog.
    pub(super) last_failure: Option<String>,
    /// Entra sign-in failed here in this run: ask for a password next.
    pub(super) entra_failed: bool,
    /// Entra attempts that failed since it last connected.
    pub(super) entra_failures: u32,
    /// "Open on a new workspace" waiting for its window; set to stop.
    pub(super) mover: Option<Arc<AtomicBool>>,
}

/// What an ended attempt left that the caller decides about.
pub(super) struct EndedAttempt {
    pub(super) used_saved_password: bool,
    pub(super) pending_logon: Option<(LogonSetting, Arc<ConnectionFile>)>,
}

impl Desk {
    /// Nothing to remember: the entry can go.
    fn is_idle(&self) -> bool {
        self.connection.is_none()
            && self.launching.is_none()
            && self.running.is_none()
            && self.reconnect.is_none()
            && !self.timed_out
            && self.note.is_none()
            && self.moved_to.is_none()
            && !self.from_tray
            && self.pending_logon.is_none()
            && self.pending_password.is_none()
            && !self.used_saved_password
            && self.last_failure.is_none()
            && !self.entra_failed
            && self.entra_failures == 0
            && self.mover.is_none()
    }

    /// Stops "open on a new workspace", if it is waiting.
    pub(super) fn stop_mover(&mut self) {
        if let Some(stop) = self.mover.take() {
            stop.store(true, Ordering::SeqCst);
        }
    }

    /// FreeRDP ended: what belonged to that attempt goes. The row's status
    /// and reconnecting are decided by the caller.
    pub(super) fn end_attempt(&mut self) -> EndedAttempt {
        self.running = None;
        self.launching = None;
        self.note = None;
        // Never remembered: this attempt didn't connect, or it did and was
        // remembered already.
        self.pending_password = None;
        self.stop_mover();
        EndedAttempt {
            used_saved_password: std::mem::take(&mut self.used_saved_password),
            pending_logon: self.pending_logon.take(),
        }
    }

    /// The row shows nothing any more: no status, no reconnecting.
    pub(super) fn clear_status(&mut self) {
        self.connection = None;
        self.reconnect = None;
        self.timed_out = false;
    }

    /// Sign-out: everything about the account's resource goes. Returns a
    /// running session to stop; one still starting is stopped as soon as
    /// it has started (`launched`).
    pub(super) fn sign_out(&mut self) -> Option<launcher::Session> {
        if let Some(l) = &mut self.launching {
            l.stop = true;
        }
        self.stop_mover();
        let launching = self.launching.take();
        let running = self.running.take().map(|r| r.session);
        *self = Self {
            launching,
            ..Self::default()
        };
        running
    }
}

/// All desks, by resource key.
#[derive(Default)]
pub(super) struct Desks(HashMap<String, Desk>);

impl Desks {
    pub(super) fn get(&self, key: &str) -> Option<&Desk> {
        self.0.get(key)
    }

    /// The desk, if there is one, for changing.
    pub(super) fn get_mut(&mut self, key: &str) -> Option<&mut Desk> {
        self.0.get_mut(key)
    }

    /// The desk, created if needed. For writing; read with [`Self::get`].
    pub(super) fn at(&mut self, key: &str) -> &mut Desk {
        self.0.entry(key.to_owned()).or_default()
    }

    /// Drops desks with nothing left to remember (after each update).
    pub(super) fn tidy(&mut self) {
        self.0.retain(|_, d| !d.is_idle());
    }

    pub(super) fn conn(&self, key: &str) -> Option<&Connection> {
        self.get(key)?.connection.as_ref()
    }

    pub(super) fn conn_mut(&mut self, key: &str) -> Option<&mut Connection> {
        self.0.get_mut(key)?.connection.as_mut()
    }

    pub(super) fn set_conn(&mut self, key: &str, connection: Connection) {
        self.at(key).connection = Some(connection);
    }

    pub(super) fn clear_conn(&mut self, key: &str) {
        if let Some(d) = self.0.get_mut(key) {
            d.connection = None;
        }
    }

    pub(super) fn running(&self, key: &str) -> Option<&Running> {
        self.get(key)?.running.as_ref()
    }

    pub(super) fn launching(&self, key: &str) -> Option<&Launching> {
        self.get(key)?.launching.as_ref()
    }

    pub(super) fn launching_mut(&mut self, key: &str) -> Option<&mut Launching> {
        self.0.get_mut(key)?.launching.as_mut()
    }

    pub(super) fn reconnect(&self, key: &str) -> Option<&Reconnect> {
        self.get(key)?.reconnect.as_ref()
    }

    /// Takes the reconnect state, if any.
    pub(super) fn take_reconnect(&mut self, key: &str) -> Option<Reconnect> {
        self.0.get_mut(key)?.reconnect.take()
    }

    /// Every running session: key and session.
    pub(super) fn all_running(&self) -> impl Iterator<Item = (&String, &Running)> {
        self.0
            .iter()
            .filter_map(|(k, d)| d.running.as_ref().map(|r| (k, r)))
    }

    /// Every FreeRDP still starting: key and start.
    pub(super) fn all_launching(&self) -> impl Iterator<Item = (&String, &Launching)> {
        self.0
            .iter()
            .filter_map(|(k, d)| d.launching.as_ref().map(|l| (k, l)))
    }

    /// Desks whose row says Connected.
    pub(super) fn connected_count(&self) -> usize {
        self.0
            .values()
            .filter(|d| matches!(d.connection, Some(Connection::Connected)))
            .count()
    }

    /// Sessions running or starting.
    pub(super) fn open_count(&self) -> usize {
        self.all_running().count() + self.all_launching().count()
    }

    /// The account's sessions running or starting.
    pub(super) fn open_count_for(&self, account: &str) -> usize {
        self.all_running()
            .filter(|(_, r)| r.account == account)
            .count()
            + self
                .all_launching()
                .filter(|(_, l)| l.account == account)
                .count()
    }

    /// Takes every running session (quitting).
    pub(super) fn take_all_running(&mut self) -> Vec<launcher::Session> {
        self.0
            .values_mut()
            .filter_map(|d| d.running.take().map(|r| r.session))
            .collect()
    }

    /// Stops every "open on a new workspace" wait.
    pub(super) fn stop_all_movers(&self) {
        for d in self.0.values() {
            if let Some(stop) = &d.mover {
                stop.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Keys of desks connected from the tray.
    pub(super) fn tray_keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().filter(|(_, d)| d.from_tray).map(|(k, _)| k)
    }

    /// Forgets which desks were connected from the tray, except those
    /// `keep` says to keep.
    pub(super) fn retain_from_tray(&mut self, keep: impl Fn(&str, &Desk) -> bool) {
        for (k, d) in &mut self.0 {
            if d.from_tray && !keep(k, d) {
                d.from_tray = false;
            }
        }
    }

    /// The connection `key` showed others' windows has ended: those apps'
    /// windows are gone with it.
    pub(super) fn release_moved_to(&mut self, key: &str) {
        for d in self.0.values_mut() {
            if d.moved_to.as_deref() == Some(key) {
                d.moved_to = None;
            }
        }
    }

    /// Windows shown through `from`'s connection now are in `to`'s.
    pub(super) fn repoint_moved_to(&mut self, from: &str, to: &str) {
        for d in self.0.values_mut() {
            if d.moved_to.as_deref() == Some(from) {
                d.moved_to = Some(to.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launching(account: &str) -> Launching {
        Launching {
            attempt: 1,
            deadline: std::time::Duration::from_secs(60),
            account: account.into(),
            stop: false,
            title: "Desk".into(),
            grabs_shortcuts: false,
            keep_awake: false,
            remote_app: false,
        }
    }

    /// A desk in the middle of an attempt, with something in every field
    /// an attempt or a run can fill.
    fn busy() -> (Desk, Arc<AtomicBool>) {
        let mover = Arc::new(AtomicBool::new(false));
        let desk = Desk {
            connection: Some(Connection::Connecting),
            launching: Some(launching("acc")),
            reconnect: Some(Reconnect {
                attempt: 2,
                timer: 9,
            }),
            timed_out: true,
            note: Some("camera".into()),
            moved_to: Some("other".into()),
            from_tray: true,
            pending_logon: Some((
                LogonSetting::EntraSignIn,
                Arc::new(ConnectionFile::local_test("h")),
            )),
            pending_password: Some(Secret::new("pw".into())),
            used_saved_password: true,
            last_failure: Some("failed".into()),
            entra_failed: true,
            entra_failures: 2,
            mover: Some(mover.clone()),
            ..Desk::default()
        };
        (desk, mover)
    }

    #[test]
    fn an_ended_attempt_leaves_only_what_outlives_it() {
        let (mut d, mover) = busy();
        let ended = d.end_attempt();
        assert!(ended.used_saved_password);
        assert!(ended.pending_logon.is_some());
        // The attempt's own state is gone, and "open on a new workspace"
        // was told to stop.
        assert!(d.launching.is_none() && d.running.is_none());
        assert!(d.note.is_none() && d.pending_password.is_none());
        assert!(!d.used_saved_password && d.pending_logon.is_none());
        assert!(d.mover.is_none() && mover.load(Ordering::SeqCst));
        // What the caller decides about, or what outlives attempts, stays.
        assert!(d.connection.is_some() && d.reconnect.is_some() && d.timed_out);
        assert!(d.last_failure.is_some() && d.entra_failed && d.entra_failures == 2);
        assert!(d.from_tray && d.moved_to.is_some());
    }

    #[test]
    fn clearing_the_status_touches_only_the_row() {
        let (mut d, _) = busy();
        d.clear_status();
        assert!(d.connection.is_none() && d.reconnect.is_none() && !d.timed_out);
        assert!(d.launching.is_some() && d.last_failure.is_some() && d.entra_failed);
    }

    #[test]
    fn sign_out_leaves_only_a_start_to_stop() {
        let (mut d, mover) = busy();
        assert!(d.sign_out().is_none(), "nothing was running");
        // The start stays, marked to stop once it has started.
        assert!(d.launching.as_ref().is_some_and(|l| l.stop));
        assert!(mover.load(Ordering::SeqCst));
        d.launching = None;
        assert!(d.is_idle(), "everything else went");
    }

    #[test]
    fn idle_desks_are_tidied_away() {
        let mut desks = Desks::default();
        desks.at("idle");
        desks.at("busy").last_failure = Some("x".into());
        desks.at("starting").launching = Some(launching("acc"));
        desks.tidy();
        assert!(desks.get("idle").is_none());
        assert!(desks.get("busy").is_some() && desks.get("starting").is_some());
        assert_eq!(desks.open_count(), 1);
        assert_eq!(desks.open_count_for("acc"), 1);
        assert_eq!(desks.open_count_for("other"), 0);
    }

    #[test]
    fn moved_windows_follow_the_connection() {
        let mut desks = Desks::default();
        desks.at("a").moved_to = Some("b".into());
        desks.at("c").moved_to = Some("b".into());
        desks.repoint_moved_to("b", "d");
        assert_eq!(desks.get("a").unwrap().moved_to.as_deref(), Some("d"));
        desks.release_moved_to("d");
        assert!(desks.get("a").unwrap().moved_to.is_none());
        assert!(desks.get("c").unwrap().moved_to.is_none());
    }

    #[test]
    fn tray_marks_can_be_dropped_selectively() {
        let mut desks = Desks::default();
        desks.at("a").from_tray = true;
        desks.at("b").from_tray = true;
        desks.at("b").connection = Some(Connection::Preparing);
        desks.retain_from_tray(|_, d| d.connection.is_some());
        let left: Vec<&String> = desks.tray_keys().collect();
        assert_eq!(left, ["b"]);
    }
}
