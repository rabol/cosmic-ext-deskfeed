// SPDX-License-Identifier: MIT

//! Starting FreeRDP: the launch itself, "open on a new workspace", the
//! connect deadline, and Show.

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;

impl AppModel {
    /// Step 3: start FreeRDP and forward its events to the app.
    // One sequence of steps in the order they happen; reads best in one
    // place.
    #[allow(clippy::too_many_lines)]
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
            self.desks
                .set_conn(key, Connection::Failed(fl!("error-helper-missing")));
            return Task::none();
        };
        let mut settings = self.effective_settings(account, key);
        // A RemoteApp's windows are ordinary windows (ADR 0020): no full
        // screen to keep awake, no single window to move to a workspace.
        let remote_app = connection.remote_app();
        if remote_app {
            settings.keep_awake = false;
            settings.new_workspace = false;
            settings.system_shortcuts = false;
        }
        // The administrator's device policy wins over the user's settings
        // (spec §Settings: respect server policy, show effective behaviour).
        let mut notes = Vec::new();
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
                notes.push(note);
            }
        }
        crate::app_log!(
            "Device policy from the connection file: {:?}",
            connection.devices
        );
        notes.extend(settings.limit_to(connection.devices).into_iter().map(
            |limited| match limited {
                Limited::Clipboard => fl!("clipboard-not-allowed"),
                Limited::Microphone => fl!("microphone-not-allowed"),
                Limited::Sound => fl!("sound-not-allowed"),
            },
        ));
        self.desks.at(key).note = (!notes.is_empty()).then(|| notes.join(" · "));
        let window_title = self
            .account(account)
            .and_then(|a| Some(format!("{} – {}", a.resource(key)?.title, a.label())))
            .unwrap_or_default();
        let spec = LaunchSpec {
            connection: (**connection).clone(),
            // Always the account's name. For a password logon the AVD
            // service's `redirectedUsername` replaces it anyway (FreeRDP
            // 3.32.2 `gateway/arm.c`, `arm_fill_rdstls`); only the domain
            // is the user's choice (logon setting, ADR 0005).
            username: signed_in.identity.username.clone(),
            password,
            logon: logon.clone(),
            helper,
            profile,
            gateway_token: (!signed_in.tokens.needs_refresh(Instant::now()))
                .then(|| signed_in.tokens.access_token.clone()),
            pinned_certificate: self.config.certificate_pins.get(key).cloned(),
            settings,
            window_title: window_title.clone(),
            app_id: <Self as cosmic::Application>::APP_ID.to_owned(),
            account: Some(crate::sessions::launcher::HelperAccount {
                object_id: signed_in.identity.object_id.clone(),
                tenant_id: signed_in.identity.tenant_id.clone(),
                username: signed_in.identity.username.clone(),
            }),
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
        let desk = self.desks.at(key);
        desk.connection = Some(Connection::Connecting);
        desk.pending_logon = Some((logon, connection.clone()));
        let title = launcher::safe_title(&window_title);
        let attempt = self.next();
        // FreeRDP's own limits: `/timeout` for the session host, and 180 s
        // for each sign-in page (`client/common/aad_helper.c`), of which a
        // connection needs at most two; plus a minute for the network.
        let deadline =
            std::time::Duration::from_secs(u64::from(settings.timeout_secs) + 2 * 180 + 60);
        self.desks.at(key).launching = Some(Launching {
            start: Start {
                attempt,
                account: account.to_owned(),
                title: title.clone(),
                grabs_shortcuts: settings.system_shortcuts,
                keep_awake: settings.keep_awake,
                remote_app,
            },
            deadline,
            stop: false,
        });
        // Starting FreeRDP writes files and hands over the arguments: not on
        // the UI thread. Its events follow once it has started.
        let (tx, rx) = mpsc::unbounded();
        let started = {
            let (account, key) = (account.to_owned(), key.to_owned());
            cosmic::task::future(async move {
                let result = tokio::task::spawn_blocking(move || {
                    let mut spec = spec;
                    spec.settings.scale = crate::display::resolve_percent(spec.settings.scale);
                    launcher::launch(&spec, move |e| {
                        let _ = tx.unbounded_send(e);
                    })
                    .map_err(|e| match e {
                        launcher::LaunchError::FreeRdpMissing => fl!("error-freerdp-missing"),
                        launcher::LaunchError::UnsupportedProxy(scheme) => {
                            fl!("error-proxy-unsupported", scheme = scheme)
                        }
                        launcher::LaunchError::BadPassword => fl!("error-password-chars"),
                        other => {
                            crate::app_log!("FreeRDP didn't start: {other:?}");
                            fl!("error-launch")
                        }
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
    pub(super) fn mover(
        &mut self,
        key: &str,
        title: String,
        settings: &crate::settings::Effective,
    ) -> Task<cosmic::Action<Message>> {
        let stop = Arc::new(AtomicBool::new(false));
        self.desks.at(key).mover = Some(stop.clone());
        // The window appears after authentication: wait as long as the
        // connection may take, plus the sign-in page.
        let wait = std::time::Duration::from_secs(u64::from(settings.timeout_secs) + 180);
        let key = key.to_owned();
        let mine = stop.clone();
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
            Message::WorkspaceMoved(key, mine, result)
        })
    }

    /// The time to connect is up: end the attempt if it hasn't connected.
    pub(super) fn connect_deadline(&mut self, key: &str, attempt: u64) {
        if let Some(r) = self
            .desks
            .running(key)
            .filter(|r| r.start.attempt == attempt)
            && !matches!(self.desks.conn(key), Some(Connection::Connected))
        {
            crate::app_log!("connecting took too long; ending FreeRDP");
            r.session.disconnect();
            self.desks.at(key).timed_out = true;
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
            .desks
            .launching(&key)
            .is_none_or(|l| l.start.attempt != attempt)
        {
            // Nobody expects it any more.
            if let Ok(session) = result {
                session.disconnect();
            }
            return Task::none();
        }
        let Launching {
            start,
            deadline,
            stop,
        } = self.desks.at(&key).launching.take().expect("checked above");
        let owner = start.account.clone();
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
                self.desks.at(&key).running = Some(Running { start, session });
                if wanted {
                    return cosmic::task::future(async move {
                        tokio::time::sleep(deadline).await;
                        Message::ConnectDeadline(key, attempt)
                    });
                }
            }
            Err(text) => {
                let desk = self.desks.at(&key);
                desk.stop_mover();
                // A local problem (FreeRDP missing, …): retrying won't help.
                desk.reconnect = None;
                desk.pending_logon = None;
                if current {
                    self.desks.set_conn(&key, Connection::Failed(text));
                } else {
                    self.desks.clear_conn(&key);
                }
            }
        }
        if self.quit == Quit::Stopping {
            return self.continue_quit();
        }
        Task::none()
    }

    /// Brings a running session's window to the front (COSMIC switches to
    /// its workspace), in the background.
    pub(super) fn show_session(&self, key: &str) -> Task<cosmic::Action<Message>> {
        let Some(title) = self.desks.running(key).map(|r| r.start.title.clone()) else {
            return Task::none();
        };
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                crate::workspace::focus(<Self as cosmic::Application>::APP_ID, &title)
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|_| Err("task".into()));
            Message::SessionShown(result)
        })
    }
}
