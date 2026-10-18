// SPDX-License-Identifier: MIT

//! Connecting a desktop: connection file, Entra sign-in or password, FreeRDP, session events.

use super::*;

impl AppModel {
    /// Connection messages: connecting, the password dialog, sessions, the workspace move. Routed here by `update`.
    pub(super) fn connection_message(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::Connect(key) => return self.prepare_connection(&key),
            Message::ConnectionFileReady(id, epoch, key, result) => {
                if self.account(&id).is_none_or(|a| a.epoch != epoch) {
                    return Task::none();
                }
                let result = match result {
                    Ok((connection, renewed)) => {
                        self.adopt_tokens(&id, epoch, &renewed);
                        Ok(connection)
                    }
                    Err(StepError::Show(e)) => Err(e),
                    Err(StepError::SignInAgain(e)) => {
                        self.connections.remove(&key);
                        self.needs_sign_in(&id, e);
                        return Task::none();
                    }
                };
                return self.connection_file_ready(&id, key, result);
            }
            Message::PasswordInput(PasswordText(s)) => {
                if let Some(d) = &mut self.dialog {
                    d.password = s;
                }
            }
            Message::TogglePasswordVisible => {
                if let Some(d) = &mut self.dialog {
                    d.password_hidden = !d.password_hidden;
                }
            }
            Message::LogonSelected(i) => {
                if let Some(d) = &mut self.dialog {
                    d.logon = i;
                    d.error = None;
                }
            }
            Message::CustomDomainInput(s) => {
                if let Some(d) = &mut self.dialog {
                    d.custom_domain = s;
                }
            }
            Message::DialogCancel => {
                if let Some(d) = self.dialog.take() {
                    self.connections.remove(&d.key);
                }
            }
            Message::DialogConnect => return self.dialog_connect(),
            Message::Launched(id, epoch, key, attempt, result) => {
                return self.launched(&id, epoch, key, attempt, result);
            }
            Message::ConnectDeadline(key, attempt) => self.connect_deadline(key, attempt),
            Message::QuitRequested
            | Message::QuitConfirm
            | Message::QuitCancel
            | Message::ReadyToQuit => return self.quit_message(&message),
            Message::Session(id, epoch, key, attempt, event) => {
                // Only the current attempt's events: a reader may still
                // report after its session ended.
                if self.account(&id).is_some_and(|a| a.epoch == epoch)
                    && self.running.get(&key).is_some_and(|r| r.attempt == attempt)
                {
                    self.session_event(&key, event);
                }
            }
            Message::Disconnect(key) => {
                if let Some(r) = self.running.get(&key) {
                    r.session.disconnect();
                } else if let Some(l) = self.launching.get_mut(&key) {
                    l.stop = true;
                }
            }
            Message::WorkspaceMoved(key, result) => {
                self.movers.remove(&key);
                // Not worth interrupting the user: the desktop opened, only
                // not on a new workspace.
                if let Err(e) = result {
                    crate::app_log!("open on a new workspace: not moved ({e})");
                }
            }
            // `update` routes only this handler's messages here.
            other => debug_assert!(false, "message routed to the wrong handler: {other:?}"),
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
            self.connections.insert(
                key.to_owned(),
                Connection::Failed(fl!("error-deps-missing")),
            );
            return self.check_dependencies();
        }
        if self.quit != Quit::No
            || self.running.contains_key(key)
            || self.launching.contains_key(key)
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
                    .map_err(|e| StepError::Show(describe_fetch_error(&FetchError::Http(e))))?;
                ConnectionFile::parse(&resp.body)
                    .map(|c| (Arc::new(c), signed_in))
                    .map_err(|e| {
                        StepError::Show(match e {
                            RdpError::GatewayNotAllowed => fl!("error-gateway-not-allowed"),
                            e => fl!("error-bad-connection-file", detail = format!("{e:?}")),
                        })
                    })
            })
            .await
            .unwrap_or_else(|_| Err(StepError::Show(fl!("error-generic"))));
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
        let (Some(helper), Some(profile)) =
            (signin::find_helper(), accounts::prepared_profile(account))
        else {
            self.connections.insert(
                key.to_owned(),
                Connection::Failed(fl!("error-helper-missing")),
            );
            return Task::none();
        };
        let mut settings = self.effective_settings(account, key);
        // The administrator's camera policy wins over the user's setting
        // (spec §Settings: respect server policy, show effective behaviour).
        self.session_notes.remove(key);
        if settings.camera {
            let note = match connection.cameras {
                CameraPolicy::All => None,
                CameraPolicy::NotAllowed => Some(fl!("camera-not-allowed")),
                CameraPolicy::Specific => Some(fl!("camera-only-some")),
            };
            crate::app_log!(
                "Camera: {:?} allowed by the connection file",
                connection.cameras
            );
            if let Some(note) = note {
                settings.camera = false;
                self.session_notes.insert(key.to_owned(), note);
            }
        }
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
        // The kind of setting only; a custom domain's name is tenant data.
        crate::app_log!(
            "Starting FreeRDP with logon setting {}",
            match &logon {
                LogonSetting::Automatic => "Automatic",
                LogonSetting::EmptyDomain => "EmptyDomain",
                LogonSetting::CustomDomain(_) => "CustomDomain",
                LogonSetting::EntraSignIn => "EntraSignIn",
            }
        );
        self.connections
            .insert(key.to_owned(), Connection::Connecting);
        self.pending_logon
            .insert(key.to_owned(), (logon, connection.clone()));
        let title = launcher::safe_title(&window_title);
        let attempt = self.next();
        // FreeRDP's own limits: `/timeout` for the session host, and 180 s
        // for each sign-in page (`client/common/aad_helper.c`), of which a
        // connection needs at most two; plus a minute for the network.
        let deadline =
            std::time::Duration::from_secs(u64::from(settings.timeout_secs) + 2 * 180 + 60);
        self.launching.insert(
            key.to_owned(),
            Launching {
                attempt,
                deadline,
                account: account.to_owned(),
                stop: false,
            },
        );
        // Starting FreeRDP writes files and hands over the arguments: not on
        // the UI thread. Its events follow once it has started.
        let (tx, rx) = mpsc::unbounded();
        let started = {
            let (account, key) = (account.to_owned(), key.to_owned());
            cosmic::task::future(async move {
                let result = tokio::task::spawn_blocking(move || {
                    launcher::launch(&spec, move |e| {
                        let _ = tx.unbounded_send(e);
                    })
                    .map_err(|e| match e {
                        launcher::LaunchError::FreeRdpMissing => fl!("error-freerdp-missing"),
                        launcher::LaunchError::UnsupportedProxy(scheme) => {
                            fl!("error-proxy-unsupported", scheme = scheme)
                        }
                        launcher::LaunchError::BadPassword => fl!("error-password-chars"),
                        other => fl!("error-launch", detail = format!("{other:?}")),
                    })
                })
                .await
                .unwrap_or_else(|_| Err(fl!("error-generic")));
                Message::Launched(account, epoch, key, attempt, result)
            })
        };
        let events = {
            let (account, key) = (account.to_owned(), key.to_owned());
            cosmic::task::stream(
                rx.map(move |e| Message::Session(account.clone(), epoch, key.clone(), attempt, e)),
            )
        };
        let session = started.chain(events);
        if !settings.new_workspace {
            return session;
        }
        Task::batch([session, self.mover(key, title, &settings)])
    }

    /// "Open on a new workspace": waits for the session's window and moves
    /// it.
    fn mover(
        &mut self,
        key: &str,
        title: String,
        settings: &crate::settings::Effective,
    ) -> Task<cosmic::Action<Message>> {
        let stop = Arc::new(AtomicBool::new(false));
        self.movers.insert(key.to_owned(), stop.clone());
        // The window appears after authentication: wait as long as the
        // connection may take, plus the sign-in page.
        let wait = std::time::Duration::from_secs(u64::from(settings.timeout_secs) + 180);
        let key = key.to_owned();
        cosmic::task::future(async move {
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
        })
    }

    /// Quitting: asked, confirmed, cancelled, done.
    fn quit_message(&mut self, message: &Message) -> Task<cosmic::Action<Message>> {
        match (message, self.quit) {
            (Message::QuitRequested, Quit::No) => {
                if self.running.is_empty() && self.launching.is_empty() {
                    self.quit = Quit::Stopping;
                    return self.continue_quit();
                }
                self.quit = Quit::Asking;
            }
            (Message::QuitCancel, Quit::Asking) => self.quit = Quit::No,
            (Message::QuitConfirm, Quit::Asking) => {
                self.quit = Quit::Stopping;
                return self.continue_quit();
            }
            (Message::ReadyToQuit, _) => return cosmic::iced::exit(),
            _ => {}
        }
        Task::none()
    }

    /// The time to connect is up: end the attempt if it hasn't connected.
    fn connect_deadline(&mut self, key: String, attempt: u64) {
        if let Some(r) = self.running.get(&key).filter(|r| r.attempt == attempt)
            && !matches!(self.connections.get(&key), Some(Connection::Connected))
        {
            crate::app_log!("connecting took too long; ending FreeRDP");
            r.session.disconnect();
            self.timed_out.insert(key);
        }
    }

    /// FreeRDP has started (or couldn't). A session nobody wants any more
    /// (disconnected, signed out or quitting meanwhile) is stopped at once.
    pub(super) fn launched(
        &mut self,
        account: &str,
        epoch: u64,
        key: String,
        attempt: u64,
        result: Result<launcher::Session, String>,
    ) -> Task<cosmic::Action<Message>> {
        if self
            .launching
            .get(&key)
            .is_none_or(|l| l.attempt != attempt)
        {
            // Nobody expects it any more.
            if let Ok(session) = result {
                session.disconnect();
            }
            return Task::none();
        }
        let Launching {
            deadline,
            account: owner,
            stop,
            ..
        } = self.launching.remove(&key).expect("checked above");
        let current = self.account(account).is_some_and(|a| a.epoch == epoch);
        match result {
            Ok(session) if self.quit != Quit::Stopping && self.signing_out.contains(&owner) => {
                // Its account is being signed out: stop it, then delete the
                // files once more in case its sign-in helper wrote any.
                return cosmic::task::future(async move {
                    let id = owner.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        launcher::stop_all(&[session], launcher::STOP_GRACE);
                        accounts::remove_files(&owner)
                    })
                    .await
                    .unwrap_or(Err(()));
                    Message::SignOutFilesRemoved(id, result)
                });
            }
            Ok(session) => {
                let wanted = self.quit == Quit::No && !stop && current;
                if self.quit == Quit::Stopping {
                    // `continue_quit` ends it with the others.
                } else if !wanted {
                    session.disconnect();
                }
                self.running.insert(
                    key.clone(),
                    Running {
                        attempt,
                        account: owner,
                        session,
                    },
                );
                if wanted {
                    return cosmic::task::future(async move {
                        tokio::time::sleep(deadline).await;
                        Message::ConnectDeadline(key, attempt)
                    });
                }
            }
            Err(text) => {
                if let Some(stop) = self.movers.remove(&key) {
                    stop.store(true, Ordering::SeqCst);
                }
                self.pending_logon.remove(&key);
                if current {
                    self.connections.insert(key, Connection::Failed(text));
                } else {
                    self.connections.remove(&key);
                }
            }
        }
        if self.quit == Quit::Stopping {
            return self.continue_quit();
        }
        Task::none()
    }

    /// Quitting: once nothing is starting any more, ends every session and
    /// sign-in, waits for FreeRDP to be gone, then exits.
    pub(super) fn continue_quit(&mut self) -> Task<cosmic::Action<Message>> {
        if !self.launching.is_empty() || !self.signing_out.is_empty() {
            // `launched` or the end of the sign-out calls this again.
            return Task::none();
        }
        for stop in self.movers.values() {
            stop.store(true, Ordering::SeqCst);
        }
        if let Some(Adding {
            cancel: Some(c), ..
        }) = &self.adding
        {
            c.cancel();
        }
        for a in &self.accounts {
            if let AccountState::SigningIn { cancel } = &a.state {
                cancel.cancel();
            }
        }
        let sessions: Vec<launcher::Session> =
            self.running.drain().map(|(_, r)| r.session).collect();
        if !sessions.is_empty() {
            crate::app_log!("quitting: ending {} session(s)", sessions.len());
        }
        cosmic::task::future(async move {
            let _ = tokio::task::spawn_blocking(move || {
                launcher::stop_all(&sessions, launcher::STOP_GRACE);
                launcher::clean_stale_runtime_dirs();
            })
            .await;
            Message::ReadyToQuit
        })
    }

    /// Asks before quitting while desktops are open.
    pub(super) fn quit_dialog(&self) -> Element<'_, Message> {
        let open = u32::try_from(self.running.len() + self.launching.len()).unwrap_or(u32::MAX);
        widget::dialog()
            .title(fl!("quit-title"))
            .body(fl!("quit-body", count = open))
            .primary_action(
                widget::button::destructive(fl!("quit-disconnect")).on_press(Message::QuitConfirm),
            )
            .secondary_action(widget::button::standard(fl!("cancel")).on_press(Message::QuitCancel))
            .into()
    }

    pub(super) fn session_event(&mut self, key: &str, event: SessionEvent) {
        match event {
            SessionEvent::Connected => {
                self.connections
                    .insert(key.to_owned(), Connection::Connected);
                self.entra_failed.remove(key);
                self.entra_failures.remove(key);
                // Remember the logon setting that worked.
                if let Some((logon, _)) = self.pending_logon.get(key).cloned() {
                    self.remember_logon(key, logon);
                }
            }
            SessionEvent::Ended { end, log } => {
                self.running.remove(key);
                self.session_notes.remove(key);
                self.launching.remove(key);
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
                if self.timed_out.remove(key) {
                    // Ended by the app: not about how the user logged on.
                    self.connections.insert(
                        key.to_owned(),
                        Connection::Failed(fl!("error-connect-too-long")),
                    );
                    return;
                }
                let was_connected =
                    matches!(self.connections.get(key), Some(Connection::Connected));
                let tried = pending.as_ref().map(|(logon, _)| logon);
                let outcome = decide::end_outcome(
                    &end,
                    was_connected,
                    self.config.logon_settings.get(key),
                    tried,
                    self.entra_failures.get(key).copied().unwrap_or(0),
                );
                if outcome != decide::EndOutcome::Clear
                    && !was_connected
                    && tried == Some(&LogonSetting::EntraSignIn)
                {
                    *self.entra_failures.entry(key.to_owned()).or_insert(0) += 1;
                }
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
