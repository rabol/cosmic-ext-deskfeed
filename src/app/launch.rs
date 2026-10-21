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
        owner: &str,
        key: &str,
        connection: &Arc<ConnectionFile>,
        logon: LogonSetting,
        password: Option<Secret>,
    ) -> Task<cosmic::Action<Message>> {
        let Some(epoch) = self.owner_epoch(owner) else {
            return Task::none();
        };
        let Some((target, username, window_title)) = self.launch_target(owner, key) else {
            return Task::none();
        };
        // A saved connection has no account level: its ID matches no
        // account's settings, so the app's apply (ADR 0029).
        let mut settings = self.effective_settings(owner, key);
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
        let folder = self.shared_folder_of(key, connection, &mut notes);
        self.desks.at(key).note = (!notes.is_empty()).then(|| notes.join(" · "));
        let spec = LaunchSpec {
            connection: (**connection).clone(),
            username,
            password,
            logon: logon.clone(),
            target,
            pinned_certificate: self.config.certificate_pins.get(key).cloned(),
            settings,
            // Checked on the disk with the launch, off the UI thread.
            shared_folder: None,
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
                owner: owner.to_owned(),
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
            let (owner, key) = (owner.to_owned(), key.to_owned());
            cosmic::task::future(async move {
                let (result, folder_missing) = off_ui(
                    move || {
                        let mut spec = spec;
                        spec.settings.scale = crate::display::resolve_percent(spec.settings.scale);
                        // A folder that isn't there (deleted, a disk not
                        // plugged in) doesn't stop the connection.
                        let share = folder.map(|path| crate::shared_folder::share(&path));
                        let folder_missing = matches!(share, Some(Err(_)));
                        spec.shared_folder = share.and_then(Result::ok);
                        let result = launcher::launch(&spec, move |e| {
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
                        });
                        (result, folder_missing)
                    },
                    (Err(fl!("error-generic")), false),
                )
                .await;
                Message::Connection(ConnectionMessage::Launched {
                    owner,
                    epoch,
                    key,
                    attempt,
                    result,
                    folder_missing,
                })
            })
        };
        let events = {
            let (owner, key) = (owner.to_owned(), key.to_owned());
            cosmic::task::stream(rx.map(move |e| {
                Message::Connection(ConnectionMessage::Session {
                    owner: owner.clone(),
                    epoch,
                    key: key.clone(),
                    attempt,
                    event: e,
                })
            }))
        };
        let session = started.chain(events);
        if !settings.new_workspace {
            return session;
        }
        Task::batch([session, self.mover(key, title, &settings)])
    }

    /// Where FreeRDP connects, as whom, and its window's title: a saved
    /// plain RDP connection directly (ADR 0029), a desktop through Azure.
    /// `None` when it can't start (the row says why, if there is a reason
    /// to give).
    pub(super) fn launch_target(
        &mut self,
        owner: &str,
        key: &str,
    ) -> Option<(launcher::Target, Option<String>, String)> {
        if let Some(c) = self.config.connections.get(key) {
            // Its own user name; no gateway, no Azure sign-in.
            return Some((
                launcher::Target::Direct {
                    restricted_admin: c.restricted_admin,
                },
                Some(c.username.clone()),
                c.title().to_owned(),
            ));
        }
        let a = self.account(owner)?;
        let signed_in = a.signed_in().cloned()?;
        let window_title = a
            .resource(key)
            .map(|r| format!("{} – {}", r.title, a.label()))
            .unwrap_or_default();
        let (Some(helper), Some(profile)) =
            (signin::find_helper(), accounts::prepared_profile(owner))
        else {
            self.desks
                .set_conn(key, Connection::Failed(fl!("error-helper-missing")));
            return None;
        };
        let target = launcher::Target::Azure {
            helper,
            profile,
            gateway_token: (!signed_in.tokens.needs_refresh(Instant::now()))
                .then(|| signed_in.tokens.access_token.clone()),
            login_hint: signed_in.identity.username.clone(),
        };
        // Always the account's name. For a password logon the AVD service's
        // `redirectedUsername` replaces it anyway (FreeRDP 3.32.2
        // `gateway/arm.c`, `arm_fill_rdstls`); only the domain is the
        // user's choice (logon setting, ADR 0005).
        Some((target, signed_in.identity.username.clone(), window_title))
    }

    /// The folder chosen to share with row `key`, if the connection file
    /// lets drives through and FreeRDP can take its path; otherwise a note
    /// says why it isn't shared. Whether it is there is checked at launch.
    pub(super) fn shared_folder_of(
        &self,
        key: &str,
        connection: &ConnectionFile,
        notes: &mut Vec<String>,
    ) -> Option<String> {
        let path = self.config.shared_folders.get(key)?;
        if !connection.devices.drives {
            crate::app_log!("shared folder: not allowed by the connection file");
            notes.push(fl!("folder-not-allowed"));
            return None;
        }
        if crate::shared_folder::check_form(path).is_err() {
            notes.push(fl!("folder-unusable"));
            return None;
        }
        Some(path.clone())
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
            let result = off_ui(
                move || {
                    crate::workspace::move_to_new_workspace(
                        <AppModel as cosmic::Application>::APP_ID,
                        &title,
                        wait,
                        &stop,
                    )
                    .map_err(|e| e.to_string())
                },
                Err("task".to_owned()),
            )
            .await;
            Message::Connection(ConnectionMessage::WorkspaceMoved {
                key,
                stop: mine,
                result,
            })
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
        owner: &str,
        epoch: u64,
        key: String,
        attempt: u64,
        result: Result<launcher::Session, String>,
        folder_missing: bool,
    ) -> Task<cosmic::Action<Message>> {
        if folder_missing
            && result.is_ok()
            && self
                .desks
                .launching(&key)
                .is_some_and(|l| l.start.attempt == attempt)
        {
            crate::app_log!("shared folder: not found, connecting without it");
            let desk = self.desks.at(&key);
            let missing = fl!("folder-missing");
            desk.note = Some(match desk.note.take() {
                Some(n) => format!("{n} · {missing}"),
                None => missing,
            });
        }
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
        let started_by = start.owner.clone();
        let current = self.owner_epoch(owner) == Some(epoch);
        match result {
            Ok(session)
                if self.quit != Quit::Stopping && self.signing_out.contains(&started_by) =>
            {
                // Its account is being signed out: stop it, then delete the
                // files once more in case its sign-in helper wrote any.
                let sign_ins = self.sign_ins.clone();
                return cosmic::task::future(async move {
                    let id = started_by.clone();
                    let result = off_ui(
                        move || {
                            launcher::stop_all(&[session], launcher::STOP_GRACE);
                            sign_ins.remove_files(&started_by)
                        },
                        Err(()),
                    )
                    .await;
                    Message::Account(AccountMessage::SignOutFilesRemoved {
                        account: id,
                        result,
                    })
                });
            }
            // Its saved connection was removed meanwhile (ADR 0029): no row
            // shows it and its events would be ignored, so it isn't kept.
            Ok(session)
                if self.quit != Quit::Stopping && self.owner_epoch(&started_by).is_none() =>
            {
                return Task::future(async move {
                    off_ui(
                        move || launcher::stop_all(&[session], launcher::STOP_GRACE),
                        (),
                    )
                    .await;
                })
                .discard();
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
                        Message::Connection(ConnectionMessage::ConnectDeadline { key, attempt })
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
            let result = off_ui(
                move || {
                    crate::workspace::focus(<Self as cosmic::Application>::APP_ID, &title)
                        .map_err(|e| e.to_string())
                },
                Err("task".into()),
            )
            .await;
            Message::Connection(ConnectionMessage::SessionShown(result))
        })
    }
}
