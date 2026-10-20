// SPDX-License-Identifier: MIT

//! The password dialog, remembered Windows passwords, and the logon
//! setting and user name that worked (ADR 0005, ADR 0009).

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;

impl AppModel {
    /// The password dialog, preset to `remembered`.
    pub(super) fn open_password_dialog(
        &mut self,
        account: &str,
        key: String,
        connection: Arc<ConnectionFile>,
        remembered: &LogonSetting,
    ) {
        let title = self
            .account(account)
            .and_then(|a| a.resource(&key))
            .map(|r| r.title.clone())
            .unwrap_or_default();
        let (logon, custom_domain) = logon_index(remembered);
        let error = self.desks.get_mut(&key).and_then(|d| d.last_failure.take());
        self.desks.set_conn(&key, Connection::AwaitingUser);
        let dialog = PasswordDialog {
            account: account.to_owned(),
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
        account: &str,
        key: String,
        connection: Arc<ConnectionFile>,
        logon: LogonSetting,
    ) -> Task<cosmic::Action<Message>> {
        let Some((identity, epoch)) = self.account(account).map(|a| (a.identity.clone(), a.epoch))
        else {
            return Task::none();
        };
        let account = account.to_owned();
        cosmic::task::future(async move {
            let owner = key.clone();
            let result = off_ui(
                move || crate::secrets::load_password(&identity, &owner).map_err(|e| e.to_string()),
                Err("task".into()),
            )
            .await;
            Message::SavedPasswordLoaded(account, epoch, key, logon, connection, result)
        })
    }

    /// A remembered password arrived: connect with it, or ask.
    pub(super) fn saved_password_loaded(
        &mut self,
        account: &str,
        epoch: u64,
        key: String,
        logon: LogonSetting,
        connection: Arc<ConnectionFile>,
        result: Result<Option<Secret>, String>,
    ) -> Task<cosmic::Action<Message>> {
        if self.account(account).is_none_or(|a| a.epoch != epoch)
            || !matches!(self.desks.conn(&key), Some(Connection::Preparing))
        {
            return Task::none();
        }
        match result {
            Ok(Some(password)) => {
                self.desks.at(&key).used_saved_password = true;
                return self.launch(account, &key, &connection, logon, Some(password));
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
        self.open_password_dialog(account, key, connection, &logon);
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
        match &self.config_handler {
            Some(handler) => {
                let _ = self.config.set_saved_passwords(handler, keys);
            }
            None => self.config.saved_passwords = keys,
        }
    }

    /// Deletes a desktop's remembered password from the keyring, in the
    /// background.
    pub(super) fn forget_password(&mut self, key: &str) -> Task<cosmic::Action<Message>> {
        self.set_saved_password(key, false);
        let Some((identity, id)) = self
            .account_of_resource(key)
            .map(|a| (a.identity.clone(), a.id.clone()))
        else {
            return Task::none();
        };
        // At once: a save already on its way writes nothing.
        let forget = crate::accounts::note_forget(&id, key);
        let key = key.to_owned();
        Task::future(async move {
            let deleted = off_ui(
                move || crate::accounts::forget_password(&identity, &key, forget).is_ok(),
                false,
            )
            .await;
            crate::app_log!("remembered password deleted: {deleted}");
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
        let d = self.dialogs.close().expect("checked above");
        // Kept only until the connection works (`session_event`).
        self.desks.at(&d.key).pending_password = match (&password, d.remember) {
            (Some(p), true) => Some(p.clone()),
            _ => None,
        };
        self.launch(&d.account, &d.key, &d.connection, logon, password)
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
