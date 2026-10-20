// SPDX-License-Identifier: MIT

//! Connecting a desktop: the message router, and step 1 (the connection
//! file). The other steps are in `password.rs`, `launch.rs` and `session.rs`.

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;

impl AppModel {
    /// Connection messages: connecting, the password dialog, sessions, the workspace move. Routed here by `route`.
    // One `match` over this handler's messages; splitting it would only
    // spread the routing out.
    #[allow(clippy::too_many_lines)]
    pub(super) fn connection_message(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::Connect(key) => return self.prepare_connection(&key),
            Message::ConnectionFileReady(id, epoch, key, result) => {
                if self.account(&id).is_none_or(|a| a.epoch != epoch) {
                    return Task::none();
                }
                // Cancelled while it was being prepared.
                if !matches!(self.desks.conn(&key), Some(Connection::Preparing)) {
                    return Task::none();
                }
                let result = match result {
                    Ok((connection, renewed)) => {
                        self.adopt_tokens(&id, epoch, &renewed);
                        Ok(connection)
                    }
                    Err(StepError::Show(e)) => Err(e),
                    Err(StepError::SignInAgain(e)) => {
                        self.desks.clear_conn(&key);
                        self.desks.take_reconnect(&key);
                        self.needs_sign_in(&id, e);
                        return Task::none();
                    }
                };
                return self.connection_file_ready(&id, key, result);
            }
            Message::PasswordInput(PasswordText(s)) => {
                if let Some(d) = self.dialogs.shown_mut() {
                    d.password = s;
                }
            }
            Message::TogglePasswordVisible => {
                if let Some(d) = self.dialogs.shown_mut() {
                    d.password_hidden = !d.password_hidden;
                }
            }
            Message::LogonSelected(i) => {
                if let Some(d) = self.dialogs.shown_mut() {
                    d.logon = i;
                    d.error = None;
                }
            }
            Message::CustomDomainInput(s) => {
                if let Some(d) = self.dialogs.shown_mut() {
                    d.custom_domain = s;
                }
            }
            // Cancelled: back to Connect.
            Message::DialogCancel => {
                if let Some(d) = self.dialogs.close() {
                    self.desks.clear_conn(&d.key);
                }
            }
            Message::DialogConnect => return self.dialog_connect(),
            Message::Launched(id, epoch, key, attempt, result) => {
                return self.launched(&id, epoch, key, attempt, result);
            }
            Message::ConnectDeadline(key, attempt) => self.connect_deadline(&key, attempt),
            Message::ReconnectDue(key, timer) => return self.reconnect_due(&key, timer),
            Message::ShowSession(key) => return self.show_session(&key),
            Message::ToggleRememberPassword(on) => {
                if let Some(d) = self.dialogs.shown_mut() {
                    d.remember = on;
                }
            }
            Message::SavedPasswordLoaded(account, epoch, key, logon, connection, result) => {
                return self.saved_password_loaded(&account, epoch, key, logon, connection, result);
            }
            Message::PasswordSaved(key, id, ticket, result) => match result {
                // Only while nothing changed since: a sign-out or Forget
                // meanwhile wins.
                Ok(true)
                    if self.account(&id).is_some()
                        && crate::accounts::password_ticket(&id, &key) == ticket =>
                {
                    crate::app_log!("Windows password remembered for a desktop");
                    self.set_saved_password(&key, true);
                }
                Ok(_) => {}
                Err(e) => {
                    crate::app_log!("Windows password not remembered ({e})");
                    self.desks.at(&key).note = Some(fl!("password-not-saved", detail = e));
                }
            },
            Message::ForgetPassword(key) => return self.forget_password(&key),
            Message::TrustCertificate => {
                let Some(d) = self.certificate_dialog.take() else {
                    return Task::none();
                };
                // A changed certificate can't be trusted from the dialog.
                if d.changed_from.is_some() {
                    return Task::none();
                }
                crate::app_log!("session host certificate approved for a desktop");
                let mut pins = self.config.certificate_pins.clone();
                pins.insert(d.key.clone(), d.certificate.fingerprint);
                self.save_certificate_pins(pins);
                self.desks.clear_conn(&d.key);
                return self.prepare_connection(&d.key);
            }
            Message::CloseCertificateDialog => self.certificate_dialog = None,
            Message::ForgetCertificate(key) => {
                let mut pins = self.config.certificate_pins.clone();
                if pins.remove(&key).is_some() {
                    crate::app_log!("approved session host certificate forgotten");
                    self.save_certificate_pins(pins);
                }
            }
            Message::SessionShown(result) => {
                // The desktop is open either way; only the log hears of it.
                if let Err(e) = result {
                    crate::app_log!("show desktop: not brought to the front ({e})");
                }
            }
            Message::QuitRequested
            | Message::QuitChosen
            | Message::QuitToBackground
            | Message::QuitConfirm
            | Message::QuitCancel
            | Message::ReadyToQuit => return self.quit_message(&message),
            Message::Session(id, epoch, key, attempt, event) => {
                // Only the current attempt's events: a reader may still
                // report after its session ended.
                if self.account(&id).is_some_and(|a| a.epoch == epoch)
                    && self
                        .desks
                        .running(&key)
                        .is_some_and(|r| r.start.attempt == attempt)
                {
                    return self.session_event(&key, event);
                }
            }
            Message::Disconnect(key) => {
                // Not started yet: preparing, or waiting for its password
                //. A connection file still on its way is
                // dropped when it arrives.
                if matches!(
                    self.desks.conn(&key),
                    Some(Connection::Preparing | Connection::AwaitingUser)
                ) {
                    self.desks.clear_conn(&key);
                    self.desks.take_reconnect(&key);
                    self.withdraw_dialogs(|d| d.key == key);
                    return Task::none();
                }
                // Stop also ends reconnecting.
                if self.desks.take_reconnect(&key).is_some()
                    && matches!(self.desks.conn(&key), Some(Connection::Reconnecting { .. }))
                {
                    self.desks.clear_conn(&key);
                }
                if let Some(r) = self.desks.running(&key) {
                    r.session.disconnect();
                }
                if self.desks.running(&key).is_some() {
                    self.desks.set_conn(&key, Connection::Disconnecting);
                } else if let Some(l) = self.desks.launching_mut(&key) {
                    l.stop = true;
                }
            }
            Message::WorkspaceMoved(key, done, result) => {
                // Only this wait's: a newer one keeps its handle.
                if let Some(d) = self.desks.get_mut(&key)
                    && d.mover.as_ref().is_some_and(|m| Arc::ptr_eq(m, &done))
                {
                    d.mover = None;
                }
                // Not worth interrupting the user: the desktop opened, only
                // not on a new workspace.
                if let Err(e) = result {
                    crate::app_log!("open on a new workspace: not moved ({e})");
                }
            }
            // `update` routes only this handler's messages here.
            other => misrouted(&other),
        }
        Task::none()
    }

    /// Step 1 of connecting: download and filter the connection file.
    pub(super) fn prepare_connection(&mut self, key: &str) -> Task<cosmic::Action<Message>> {
        // A required dependency is missing or too old (spec §8: check the
        // backend before launch). Check again, so a fixed one counts on
        // the next try.
        if let Some(checks) = &self.deps
            && !deps::required_ok(checks)
        {
            self.desks
                .set_conn(key, Connection::Failed(fl!("error-deps-missing")));
            return self.check_dependencies();
        }
        // Connecting takes its session back (ADR 0020).
        if let Some(d) = self.desks.get_mut(key) {
            d.moved_to = None;
        }
        if self.quit != Quit::No
            || self.desks.running(key).is_some()
            || self.desks.launching(key).is_some()
            || self
                .desks
                .conn(key)
                .is_some_and(|c| c.is_starting() || *c == Connection::Disconnecting)
        {
            return Task::none(); // no accidental duplicate launches
        }
        let Some(account) = self.account_of_resource(key) else {
            return Task::none();
        };
        let (Some(signed_in), Some(resource)) =
            (account.signed_in().cloned(), account.resource(key))
        else {
            return Task::none();
        };
        let (id, epoch) = (account.id.clone(), account.epoch);
        let Some(url) = resource.connection_file.clone() else {
            self.desks
                .set_conn(key, Connection::Failed(fl!("error-no-connection-file")));
            return Task::none();
        };
        self.desks.set_conn(key, Connection::Preparing);
        let http = self.http.clone();
        let key = key.to_owned();
        // Read now: a sign-out before this runs stops it.
        let started = accounts::sign_out_count(&id);
        cosmic::task::future(async move {
            let result = off_ui(
                move || {
                    // The access token also goes to FreeRDP, so it must stay
                    // valid through the connection.
                    let signed_in = accounts::renew_if_needed(&http, signed_in, started)?;
                    let get = |s: &SignedIn| {
                        http.get(&url, Role::ConnectionFile, Some(&s.tokens.access_token))
                    };
                    let (resp, signed_in) = match get(&signed_in) {
                        // Refused although it looked valid: renew once and retry.
                        Err(HttpError::Status { status: 401, .. }) => {
                            crate::app_log!("connection file: access token refused; renewing");
                            let renewed = accounts::renew_now(&http, signed_in, started)?;
                            (get(&renewed).map_err(refused_or_shown_http)?, renewed)
                        }
                        other => (
                            other.map_err(|e| {
                                StepError::Show(describe_fetch_error(&FetchError::Http(e)))
                            })?,
                            signed_in,
                        ),
                    };
                    ConnectionFile::parse(&resp.body)
                        .map(|c| {
                            // Names only: they come from Deskfeed's own list.
                            if !c.refused.is_empty() {
                                crate::app_log!(
                                    "connection file: dropped for their form: {}",
                                    c.refused.join(", ")
                                );
                            }
                            (Arc::new(c), signed_in)
                        })
                        .map_err(|e| {
                            StepError::Show(match e {
                                RdpError::GatewayNotAllowed => fl!("error-gateway-not-allowed"),
                                e => {
                                    crate::app_log!("connection file refused: {e:?}");
                                    fl!("error-bad-connection-file")
                                }
                            })
                        })
                },
                Err(StepError::Show(fl!("error-generic"))),
            )
            .await;
            Message::ConnectionFileReady(id, epoch, key, result)
        })
    }

    /// Step 2: Entra sign-in first; the Windows password only when chosen or
    /// after Entra sign-in failed.
    pub(super) fn connection_file_ready(
        &mut self,
        account: &str,
        key: String,
        result: Result<Arc<ConnectionFile>, String>,
    ) -> Task<cosmic::Action<Message>> {
        let connection = match result {
            Ok(c) => c,
            // Offline or a network error while reconnecting: try later.
            Err(_) if self.desks.reconnect(&key).is_some() => return self.schedule_reconnect(&key),
            Err(e) => {
                self.desks.set_conn(&key, Connection::Failed(e));
                return Task::none();
            }
        };
        let remembered = match decide::first_step(
            self.config.logon_settings.get(&key),
            self.desks.get(&key).is_some_and(|d| d.entra_failed),
        ) {
            decide::FirstStep::Entra => {
                return self.launch(account, &key, &connection, LogonSetting::EntraSignIn, None);
            }
            // A remembered password: no dialog (also when reconnecting).
            decide::FirstStep::AskPassword(setting)
                if setting.needs_password() && self.config.saved_passwords.contains(&key) =>
            {
                return self.load_saved_password(account, key, connection, setting);
            }
            // Reconnecting never opens the password dialog by itself.
            decide::FirstStep::AskPassword(_) if self.desks.take_reconnect(&key).is_some() => {
                self.desks
                    .set_conn(&key, Connection::Failed(fl!("error-connection-lost")));
                return Task::none();
            }
            decide::FirstStep::AskPassword(setting) => setting,
        };
        self.open_password_dialog(account, key, connection, &remembered);
        Task::none()
    }
}
