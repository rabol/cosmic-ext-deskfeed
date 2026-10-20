// SPDX-License-Identifier: MIT

//! The password dialog, remembered Windows passwords, and the logon
//! setting and user name that worked (ADR 0005, ADR 0009).

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;
use crate::connections::SavedConnection;

impl AppModel {
    /// The password dialog, preset to `remembered`.
    pub(super) fn open_password_dialog(
        &mut self,
        owner: &str,
        key: String,
        connection: Arc<ConnectionFile>,
        remembered: &LogonSetting,
    ) {
        let title = self.title_of(&key);
        let (logon, custom_domain) = logon_index(remembered);
        let error = self.desks.get_mut(&key).and_then(|d| d.last_failure.take());
        self.desks.set_conn(&key, Connection::AwaitingUser);
        let dialog = PasswordDialog {
            owner: owner.to_owned(),
            key,
            title,
            connection,
            password: String::new(),
            password_hidden: true,
            logon,
            custom_domain,
            error,
            remember: false,
        };
        // Another resource's dialog is open: this one waits its turn.
        self.dialogs.offer(dialog, |d| &d.key);
    }

    /// Drops the password dialogs `gone` matches and shows the next one.
    pub(super) fn withdraw_dialogs(&mut self, gone: impl Fn(&PasswordDialog) -> bool) {
        self.dialogs.withdraw(gone);
    }

    /// Reads a desktop's remembered password in the background.
    pub(super) fn load_saved_password(
        &mut self,
        owner: &str,
        key: String,
        connection: Arc<ConnectionFile>,
        logon: LogonSetting,
    ) -> Task<cosmic::Action<Message>> {
        let Some(epoch) = self.owner_epoch(owner) else {
            return Task::none();
        };
        // A saved connection's password is its own kind of keyring item
        // (ADR 0029); a desktop's belongs to its account.
        let identity = self.account(owner).map(|a| a.identity.clone());
        let owner = owner.to_owned();
        cosmic::task::future(async move {
            let desktop = key.clone();
            let result = off_ui(
                move || {
                    match &identity {
                        Some(identity) => crate::secrets::load_password(identity, &desktop),
                        None => crate::secrets::load_connection_password(&desktop),
                    }
                    .map_err(|e| e.to_string())
                },
                Err("task".into()),
            )
            .await;
            Message::Connection(ConnectionMessage::SavedPasswordLoaded {
                owner,
                epoch,
                key,
                logon,
                connection,
                result,
            })
        })
    }

    /// A remembered password arrived: connect with it, or ask.
    pub(super) fn saved_password_loaded(
        &mut self,
        owner: &str,
        epoch: u64,
        key: String,
        logon: LogonSetting,
        connection: Arc<ConnectionFile>,
        result: Result<Option<Secret>, String>,
    ) -> Task<cosmic::Action<Message>> {
        if self.owner_epoch(owner) != Some(epoch)
            || !matches!(self.desks.conn(&key), Some(Connection::Preparing))
        {
            return Task::none();
        }
        match result {
            Ok(Some(password)) => {
                self.desks.at(&key).used_saved_password = true;
                return self.launch(owner, &key, &connection, logon, Some(password));
            }
            Ok(None) => {
                crate::app_log!("remembered password not in the keyring any more");
                self.set_saved_password(&key, false);
            }
            Err(e) => crate::app_log!("remembered password not read ({e})"),
        }
        if self.desks.take_reconnect(&key).is_some() {
            self.desks
                .set_conn(&key, Connection::Failed(fl!("error-connection-lost")));
            return Task::none();
        }
        self.open_password_dialog(owner, key, connection, &logon);
        Task::none()
    }

    /// Records whether a desktop has a remembered password.
    pub(super) fn set_saved_password(&mut self, key: &str, saved: bool) {
        let mut keys = self.config.saved_passwords.clone();
        let changed = if saved {
            keys.insert(key.to_owned())
        } else {
            keys.remove(key)
        };
        if !changed {
            return;
        }
        save_config!(self, saved_passwords, set_saved_passwords, keys);
    }

    /// Deletes a desktop's remembered password from the keyring, in the
    /// background.
    pub(super) fn forget_password(&mut self, key: &str) -> Task<cosmic::Action<Message>> {
        self.set_saved_password(key, false);
        if self.config.connections.contains_key(key) {
            return self.forget_connection_password(key);
        }
        let Some((identity, id)) = self
            .account_of_resource(key)
            .map(|a| (a.identity.clone(), a.id.clone()))
        else {
            return Task::none();
        };
        // At once: a save already on its way writes nothing.
        let forget = self.sign_ins.note_forget(&id, key);
        let key = key.to_owned();
        let sign_ins = self.sign_ins.clone();
        Task::future(async move {
            let deleted = off_ui(
                move || sign_ins.forget_password(&identity, &key, forget).is_ok(),
                false,
            )
            .await;
            crate::app_log!("remembered password deleted: {deleted}");
        })
        .discard()
    }

    /// Deletes a saved connection's remembered password in the background;
    /// also when removing the connection, so after it is gone from the
    /// settings.
    pub(super) fn forget_connection_password(&self, id: &str) -> Task<cosmic::Action<Message>> {
        // At once: a save already on its way writes nothing.
        let forget = self.sign_ins.note_forget(id, id);
        let id = id.to_owned();
        let sign_ins = self.sign_ins.clone();
        Task::future(async move {
            let deleted = off_ui(
                move || sign_ins.forget_connection_password(&id, forget).is_ok(),
                false,
            )
            .await;
            crate::app_log!("remembered connection password deleted: {deleted}");
        })
        .discard()
    }

    pub(super) fn dialog_connect(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(d) = self.dialogs.shown_mut() else {
            return Task::none();
        };
        let logon = match d.logon {
            LOGON_EMPTY => LogonSetting::EmptyDomain,
            LOGON_CUSTOM => {
                let domain = d.custom_domain.trim().to_owned();
                if !crate::sessions::launcher::valid_domain(&domain) {
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
        let d = self.dialogs.close().expect("checked above");
        // Kept only until the connection works (`session_event`).
        self.desks.at(&d.key).pending_password = match (&password, d.remember) {
            (Some(p), true) => Some(p.clone()),
            _ => None,
        };
        self.launch(&d.owner, &d.key, &d.connection, logon, password)
    }

    pub(super) fn remember_logon(&mut self, key: &str, logon: LogonSetting) {
        if self.logon_of(key) == Some(&logon) {
            return;
        }
        // A saved connection keeps it with its other fields, where its form
        // shows it (the domain).
        if let Some(c) = self.config.connections.get(key) {
            let mut all = self.config.connections.clone();
            all.insert(key.to_owned(), SavedConnection { logon, ..c.clone() });
            save_config!(self, connections, set_connections, all);
            return;
        }
        let mut map = self.config.logon_settings.clone();
        map.insert(key.to_owned(), logon);
        save_config!(self, logon_settings, set_logon_settings, map);
    }
}
