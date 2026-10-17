// SPDX-License-Identifier: MIT

//! Connecting a desktop: connection file, Entra sign-in or password, FreeRDP, session events.

use super::*;

impl AppModel {
    /// Step 1 of connecting: download and filter the connection file.
    pub(super) fn prepare_connection(&mut self, key: &str) -> Task<cosmic::Action<Message>> {
        if self.running.contains_key(key)
            || matches!(
                self.connections.get(key),
                Some(Connection::Preparing | Connection::Connecting)
            )
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
            self.connections.insert(
                key.to_owned(),
                Connection::Failed(fl!("error-no-connection-file")),
            );
            return Task::none();
        };
        self.connections
            .insert(key.to_owned(), Connection::Preparing);
        let http = self.http.clone();
        let key = key.to_owned();
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                // The access token also goes to FreeRDP, so it must stay
                // valid through the connection.
                let signed_in = accounts::renew_if_needed(&http, signed_in)?;
                let resp = http
                    .get(
                        &url,
                        Role::ConnectionFile,
                        Some(&signed_in.tokens.access_token),
                    )
                    .map_err(|e| describe_fetch_error(&FetchError::Http(e)))?;
                ConnectionFile::parse(&resp.body)
                    .map(|c| (Arc::new(c), signed_in))
                    .map_err(|e| fl!("error-bad-connection-file", detail = format!("{e:?}")))
            })
            .await
            .unwrap_or_else(|_| Err(fl!("error-generic")));
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
            Err(e) => {
                self.connections.insert(key, Connection::Failed(e));
                return Task::none();
            }
        };
        let remembered = match decide::first_step(
            connection.entra_sso(),
            self.config.logon_settings.get(&key),
            self.entra_failed.contains(&key),
        ) {
            decide::FirstStep::Entra => {
                return self.launch(account, &key, &connection, LogonSetting::EntraSignIn, None);
            }
            decide::FirstStep::AskPassword(setting) => setting,
        };
        let title = self
            .account(account)
            .and_then(|a| a.resource(&key))
            .map(|r| r.title.clone())
            .unwrap_or_default();
        let (logon, custom_domain) = logon_index(&remembered);
        let error = self.last_failure.remove(&key);
        self.dialog = Some(PasswordDialog {
            account: account.to_owned(),
            key,
            title,
            connection,
            password: String::new(),
            password_hidden: true,
            logon,
            custom_domain,
            error,
        });
        Task::none()
    }

    pub(super) fn dialog_connect(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(d) = &mut self.dialog else {
            return Task::none();
        };
        let logon = match d.logon {
            LOGON_EMPTY => LogonSetting::EmptyDomain,
            LOGON_CUSTOM => {
                let domain = d.custom_domain.trim().to_owned();
                if !valid_domain(&domain) {
                    d.error = Some(fl!("error-bad-domain"));
                    return Task::none();
                }
                LogonSetting::CustomDomain(domain)
            }
            LOGON_ENTRA => LogonSetting::EntraSignIn,
            _ => LogonSetting::Automatic,
        };
        let password = if logon.needs_password() {
            if d.password.is_empty() {
                d.error = Some(fl!("error-password-empty"));
                return Task::none();
            }
            if d.password.contains(['\n', '\r', '\0']) {
                d.error = Some(fl!("error-password-chars"));
                return Task::none();
            }
            Some(Secret::new(std::mem::take(&mut d.password)))
        } else {
            None
        };
        let d = self.dialog.take().expect("checked above");
        self.launch(&d.account, &d.key, &d.connection, logon, password)
    }

    /// Step 3: start FreeRDP and forward its events to the app.
    pub(super) fn launch(
        &mut self,
        account: &str,
        key: &str,
        connection: &Arc<ConnectionFile>,
        logon: LogonSetting,
        password: Option<Secret>,
    ) -> Task<cosmic::Action<Message>> {
        let Some((signed_in, epoch)) = self
            .account(account)
            .and_then(|a| Some((a.signed_in()?.clone(), a.epoch)))
        else {
            return Task::none();
        };
        let (Some(helper), Some(profile)) = (signin::find_helper(), accounts::profile_dir(account))
        else {
            self.connections.insert(
                key.to_owned(),
                Connection::Failed(fl!("error-helper-missing")),
            );
            return Task::none();
        };
        let settings = self.effective_settings(account, key);
        let window_title = self
            .account(account)
            .and_then(|a| Some(format!("{} – {}", a.resource(key)?.title, a.label())))
            .unwrap_or_default();
        let spec = LaunchSpec {
            connection: (**connection).clone(),
            username: signed_in.identity.username.clone(),
            password,
            logon: logon.clone(),
            helper,
            profile,
            gateway_token: (!signed_in.tokens.needs_refresh(Instant::now()))
                .then(|| signed_in.tokens.access_token.clone()),
            settings,
            window_title: window_title.clone(),
            app_id: <Self as cosmic::Application>::APP_ID.to_owned(),
        };
        // No secrets here: the setting and, for a custom domain, its name.
        crate::app_log!("Starting FreeRDP with logon setting {logon:?}");
        let (tx, rx) = mpsc::unbounded();
        let started = launcher::launch(&spec, move |e| {
            let _ = tx.unbounded_send(e);
        });
        match started {
            Ok(session) => {
                self.running.insert(key.to_owned(), session);
                self.connections
                    .insert(key.to_owned(), Connection::Connecting);
                self.pending_logon
                    .insert(key.to_owned(), (logon, connection.clone()));
                let events = {
                    let (account, key) = (account.to_owned(), key.to_owned());
                    cosmic::task::stream(
                        rx.map(move |e| Message::Session(account.clone(), epoch, key.clone(), e)),
                    )
                };
                if !settings.new_workspace {
                    return events;
                }
                let stop = Arc::new(AtomicBool::new(false));
                self.movers.insert(key.to_owned(), stop.clone());
                let title = launcher::safe_title(&window_title);
                // The window appears after authentication: wait as long as
                // the connection may take, plus the sign-in page.
                let wait = std::time::Duration::from_secs(u64::from(settings.timeout_secs) + 180);
                let key = key.to_owned();
                let mover = cosmic::task::future(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        crate::workspace::move_to_new_workspace(
                            <AppModel as cosmic::Application>::APP_ID,
                            &title,
                            wait,
                            &stop,
                        )
                    })
                    .await;
                    let result = match result {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(e)) => Err(e.to_string()),
                        Err(_) => Err("task".to_owned()),
                    };
                    Message::WorkspaceMoved(key, result)
                });
                Task::batch([events, mover])
            }
            Err(e) => {
                let text = match e {
                    launcher::LaunchError::FreeRdpMissing => fl!("error-freerdp-missing"),
                    launcher::LaunchError::BadPassword => fl!("error-password-chars"),
                    other => fl!("error-launch", detail = format!("{other:?}")),
                };
                self.connections
                    .insert(key.to_owned(), Connection::Failed(text));
                Task::none()
            }
        }
    }

    pub(super) fn session_event(&mut self, key: &str, event: SessionEvent) {
        match event {
            SessionEvent::Connected => {
                self.connections
                    .insert(key.to_owned(), Connection::Connected);
                self.entra_failed.remove(key);
                // Remember the logon setting that worked.
                if let Some((logon, _)) = self.pending_logon.get(key).cloned() {
                    self.remember_logon(key, logon);
                }
            }
            SessionEvent::Ended { end, log } => {
                self.running.remove(key);
                if let Some(stop) = self.movers.remove(key) {
                    stop.store(true, Ordering::SeqCst);
                }
                let pending = self.pending_logon.remove(key);
                if !log.is_empty() && end != SessionEnd::Closed {
                    crate::app_log!("FreeRDP ended ({end:?}); last log lines:");
                    for line in &log {
                        crate::app_log!("  {line}");
                    }
                }
                let outcome = decide::end_outcome(
                    &end,
                    matches!(self.connections.get(key), Some(Connection::Connected)),
                    self.config.logon_settings.get(key) == Some(&LogonSetting::EntraSignIn),
                    pending.as_ref().map(|(logon, _)| logon),
                );
                match outcome {
                    decide::EndOutcome::Clear => {
                        self.connections.remove(key);
                    }
                    decide::EndOutcome::OfferPassword => {
                        // Offer password logon next time, and say why.
                        self.entra_failed.insert(key.to_owned());
                        self.connections.insert(
                            key.to_owned(),
                            Connection::Failed(fl!("status-entra-failed")),
                        );
                        self.last_failure.insert(
                            key.to_owned(),
                            fl!("error-entra-failed", detail = describe_session_end(&end)),
                        );
                    }
                    decide::EndOutcome::WrongPassword => {
                        // Stop and explain; the user decides whether to try
                        // again (ADR 0005). No automatic retry or re-prompt.
                        let tried = pending
                            .map(|(logon, _)| self.logon_labels[logon_index(&logon).0].clone())
                            .unwrap_or_default();
                        let message = fl!("error-logon-failed", setting = tried);
                        self.connections.insert(
                            key.to_owned(),
                            Connection::Failed(fl!("status-logon-failed")),
                        );
                        self.last_failure.insert(key.to_owned(), message);
                    }
                    decide::EndOutcome::Failed => {
                        self.connections.insert(
                            key.to_owned(),
                            Connection::Failed(describe_session_end(&end)),
                        );
                    }
                }
            }
        }
    }

    pub(super) fn remember_logon(&mut self, key: &str, logon: LogonSetting) {
        if self.config.logon_settings.get(key) == Some(&logon) {
            return;
        }
        let mut map = self.config.logon_settings.clone();
        map.insert(key.to_owned(), logon);
        if let Some(handler) = &self.config_handler {
            let _ = self.config.set_logon_settings(handler, map);
        }
    }
}
