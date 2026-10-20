// SPDX-License-Identifier: MIT

//! A running session: its events, how it ended, and reconnecting
//! (ADR 0014, ADR 0020).

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;

impl AppModel {
    pub(super) fn session_event(
        &mut self,
        key: &str,
        event: SessionEvent,
    ) -> Task<cosmic::Action<Message>> {
        // Remembering a password once its connection worked.
        let mut save_task = Task::none();
        match event {
            SessionEvent::Stage(stage) => {
                // Only while starting; Disconnect wins.
                if let Some(c @ (Connection::Connecting | Connection::Authenticating)) =
                    self.desks.conn_mut(key)
                {
                    *c = match stage {
                        launcher::Stage::Connecting => Connection::Connecting,
                        launcher::Stage::Authenticating => Connection::Authenticating,
                    };
                }
            }
            SessionEvent::Connected => {
                if matches!(self.desks.conn(key), Some(Connection::Disconnecting)) {
                    return Task::none();
                }
                if let Some(r) = self.desks.take_reconnect(key) {
                    crate::app_log!("reconnected (attempt {})", r.attempt);
                }
                if let Some(d) = self.desks.get_mut(key) {
                    d.used_saved_password = false;
                }
                if let Some(password) = self
                    .desks
                    .get_mut(key)
                    .and_then(|d| d.pending_password.take())
                {
                    save_task = self.save_password(key, password);
                }
                let desk = self.desks.at(key);
                desk.connection = Some(Connection::Connected);
                desk.entra_failed = false;
                desk.entra_failures = 0;
                // Remember the logon setting that worked.
                if let Some((logon, _)) = desk.pending_logon.clone() {
                    self.remember_logon(key, logon);
                }
            }
            SessionEvent::Ended { end, log } => return self.session_ended(key, &end, &log),
        }
        save_task
    }

    /// Remembers the password a connection just worked with ("Remember
    /// this password"), in the background: a desktop's under its account,
    /// a saved connection's as its own item (ADR 0029).
    fn save_password(&self, key: &str, password: Secret) -> Task<cosmic::Action<Message>> {
        let Some(owner) = self.owner_of(key) else {
            return Task::none();
        };
        let identity = self.account(&owner).map(|a| a.identity.clone());
        let title = self.title_of(key);
        let key = key.to_owned();
        // Read now: a sign-out, Forget or removal before the save runs
        // stops it.
        let ticket = self.sign_ins.password_ticket(&owner, &key);
        let sign_ins = self.sign_ins.clone();
        cosmic::task::future(async move {
            let desktop = key.clone();
            let result =
                off_ui(
                    move || {
                        match &identity {
                            Some(identity) => sign_ins
                                .save_password(identity, &desktop, &title, &password, ticket),
                            None => sign_ins
                                .save_connection_password(&desktop, &title, &password, ticket),
                        }
                        .map_err(|e| keyring_detail(&e))
                    },
                    Err("task".into()),
                )
                .await;
            Message::Connection(ConnectionMessage::PasswordSaved {
                key,
                owner,
                ticket,
                result,
            })
        })
    }

    /// A session's FreeRDP ended: clear its state, then reconnect, explain,
    /// or ask, depending on how it ended.
    // One sequence of steps in the order they happen; reads best in one
    // place.
    #[allow(clippy::too_many_lines)]
    pub(super) fn session_ended(
        &mut self,
        key: &str,
        end: &SessionEnd,
        log: &[String],
    ) -> Task<cosmic::Action<Message>> {
        let mut save_task = Task::none();
        let ended = self
            .desks
            .running(key)
            .map(|r| (r.start.attempt, r.start.remote_app, r.start.owner.clone()));
        // What belonged to this attempt goes (`Desk::end_attempt`).
        let crate::app::desk::EndedAttempt {
            used_saved_password: used_saved,
            pending_logon: pending,
        } = self.desks.at(key).end_attempt();
        if !log.is_empty() && *end != SessionEnd::Closed {
            crate::app_log!("FreeRDP ended ({end:?}); last log lines:");
            for line in log {
                crate::app_log!("  {line}");
            }
        }
        // Ended because the user disconnected: however FreeRDP
        // ended, that is a normal end, never a reason to reconnect.
        if matches!(self.desks.conn(key), Some(Connection::Disconnecting)) {
            // Windows shown through it are gone with it.
            self.desks.release_moved_to(key);
            self.desks.at(key).clear_status();
            return Task::none();
        }
        let was_connected = matches!(self.desks.conn(key), Some(Connection::Connected));
        if let Some(to) =
            ended
                .filter(|(_, remote_app, _)| *remote_app)
                .and_then(|(attempt, _, account)| {
                    self.newer_app_connection(key, &account, end, was_connected, attempt)
                })
        {
            crate::app_log!("RemoteApp's session taken over by another app's connection");
            // Windows that had moved here move on with them.
            self.desks.repoint_moved_to(key, &to);
            let desk = self.desks.at(key);
            desk.clear_status();
            desk.moved_to = Some(to);
            return save_task;
        }
        self.desks.release_moved_to(key);
        let tried = pending.as_ref().map(|(logon, _)| logon);
        let reconnecting = self.desks.reconnect(key).is_some();
        let wanted = self
            .owner_of(key)
            .is_some_and(|owner| self.effective_settings(&owner, key).reconnect);
        let timed_out = self.desks.get(key).is_some_and(|d| d.timed_out);
        if wanted
            && self.quit == Quit::No
            && ((timed_out && reconnecting)
                || (!timed_out
                    && decide::reconnects(
                        end,
                        was_connected,
                        reconnecting,
                        tried,
                        self.config.saved_passwords.contains(key),
                    )))
        {
            self.desks.at(key).timed_out = false;
            return self.schedule_reconnect(key);
        }
        let desk = self.desks.at(key);
        desk.reconnect = None;
        if std::mem::take(&mut desk.timed_out) {
            // Ended by the app: not about how the user logged on.
            desk.connection = Some(Connection::Failed(fl!("error-connect-too-long")));
            return Task::none();
        }
        let outcome = decide::end_outcome(
            end,
            was_connected,
            self.logon_of(key),
            tried,
            self.desks.get(key).map_or(0, |d| d.entra_failures),
        );
        if outcome != decide::EndOutcome::Clear
            && !was_connected
            && tried == Some(&LogonSetting::EntraSignIn)
            // Picking another account is a slip, not Entra sign-in failing.
            && *end != SessionEnd::HostTimedOut
        {
            self.desks.at(key).entra_failures += 1;
        }
        if let SessionEnd::CertificateNotTrusted(certificate) = end {
            let pinned = self.config.certificate_pins.get(key).cloned();
            let step = decide::certificate_step(pinned.as_deref(), &certificate.fingerprint);
            crate::app_log!("session host certificate not trusted: {step:?}");
            let title = self.title_of(key);
            self.desks.set_conn(
                key,
                Connection::Failed(match step {
                    decide::CertificateStep::Ask => fl!("error-certificate-untrusted"),
                    decide::CertificateStep::Changed => fl!("error-certificate-changed"),
                }),
            );
            self.certificate_dialog = Some(CertificateDialog {
                key: key.to_owned(),
                title,
                direct: self.config.connections.contains_key(key),
                certificate: certificate.clone(),
                changed_from: (step == decide::CertificateStep::Changed)
                    .then_some(pinned)
                    .flatten(),
            });
            return Task::none();
        }
        match outcome {
            decide::EndOutcome::Clear => {
                self.desks.clear_conn(key);
            }
            decide::EndOutcome::OfferPassword => {
                // Offer password logon next time, and say why.
                let desk = self.desks.at(key);
                desk.entra_failed = true;
                desk.connection = Some(Connection::Failed(fl!("status-entra-failed")));
                desk.last_failure =
                    Some(fl!("error-entra-failed", detail = describe_ended(end, log)));
            }
            decide::EndOutcome::WrongPassword => {
                // Stop and explain; the user decides whether to try
                // again (ADR 0005). No automatic retry or re-prompt.
                let tried = pending
                    .map(|(logon, _)| self.logon_labels[logon_index(&logon).0].clone())
                    .unwrap_or_default();
                let message = if used_saved {
                    // Never tried again (ADR 0005): it goes, and
                    // the dialog asks next time.
                    save_task = self.forget_password(key);
                    fl!("password-rejected-removed")
                } else {
                    fl!("error-logon-failed", setting = tried)
                };
                let desk = self.desks.at(key);
                desk.connection = Some(Connection::Failed(fl!("status-logon-failed")));
                desk.last_failure = Some(message);
            }
            decide::EndOutcome::Failed => {
                self.desks
                    .set_conn(key, Connection::Failed(describe_ended(end, log)));
                // Windows refused the remembered password for another
                // reason (locked, expired, …): it goes too, so the next
                // Connect asks instead of sending it again (ADR 0005,
                // ADR 0009 addendum).
                if used_saved && decide::refuses_password(end) {
                    save_task = self.forget_password(key);
                }
            }
        }
        save_task
    }

    /// The RemoteApp connection that took over this one's Windows session,
    /// if any (`decide::moved_to`): the same account's, starting or
    /// connected, not being disconnected.
    pub(super) fn newer_app_connection(
        &self,
        key: &str,
        account: &str,
        end: &SessionEnd,
        was_connected: bool,
        attempt: u64,
    ) -> Option<String> {
        let leaving = |k: &str| matches!(self.desks.conn(k), Some(Connection::Disconnecting));
        let running = self
            .desks
            .all_running()
            .filter(|(k, r)| {
                r.start.remote_app && r.start.owner == account && *k != key && !leaving(k)
            })
            .map(|(k, r)| (k.as_str(), r.start.attempt));
        let launching = self
            .desks
            .all_launching()
            .filter(|(k, l)| {
                l.start.remote_app && l.start.owner == account && *k != key && !leaving(k)
            })
            .map(|(k, l)| (k.as_str(), l.start.attempt));
        decide::moved_to(end, was_connected, attempt, running.chain(launching)).map(str::to_owned)
    }

    /// Waits, then starts the next reconnect attempt; gives up after the
    /// desktop's "Reconnect attempts".
    pub(super) fn schedule_reconnect(&mut self, key: &str) -> Task<cosmic::Action<Message>> {
        let Some(of) = self
            .owner_of(key)
            .map(|owner| self.effective_settings(&owner, key).reconnect_attempts)
        else {
            self.desks.take_reconnect(key);
            return Task::none();
        };
        let timer = self.next();
        let r = self
            .desks
            .at(key)
            .reconnect
            .get_or_insert(Reconnect { attempt: 0, timer });
        r.attempt += 1;
        r.timer = timer;
        let attempt = r.attempt;
        if attempt > of {
            self.desks.take_reconnect(key);
            crate::app_log!("connection lost; gave up after {of} reconnect attempts");
            self.desks
                .set_conn(key, Connection::Failed(fl!("error-reconnect-gave-up")));
            return Task::none();
        }
        let wait = decide::reconnect_delay(attempt);
        crate::app_log!(
            "connection lost; reconnect attempt {attempt} of {of} in {} s",
            wait.as_secs()
        );
        self.desks
            .set_conn(key, Connection::Reconnecting { attempt, of });
        let key = key.to_owned();
        cosmic::task::future(async move {
            tokio::time::sleep(wait).await;
            Message::Connection(ConnectionMessage::ReconnectDue { key, wait: timer })
        })
    }

    /// The wait is over: connect again, unless stopped or replaced
    /// meanwhile (Stop, Connect, sign-out).
    pub(super) fn reconnect_due(&mut self, key: &str, timer: u64) -> Task<cosmic::Action<Message>> {
        if self.desks.reconnect(key).is_none_or(|r| r.timer != timer)
            || !matches!(self.desks.conn(key), Some(Connection::Reconnecting { .. }))
        {
            return Task::none();
        }
        self.desks.clear_conn(key);
        let task = self.prepare_connection(key);
        // Nothing started (signed out, quitting, …): stop reconnecting.
        if self.desks.conn(key).is_none() {
            self.desks.take_reconnect(key);
        }
        task
    }
}
