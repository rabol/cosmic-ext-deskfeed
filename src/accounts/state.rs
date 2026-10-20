// SPDX-License-Identifier: MIT

//! The accounts' shared state, and saving and deleting their sign-ins,
//! remembered passwords and offline lists.

use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};

use super::account_id;
use super::profiles::{jars_root, profile_dir};
use crate::auth::attempt::ClientConfig;
use crate::auth::id_token::Identity;
use crate::auth::signin::SignedIn;
use crate::auth::token::Secret;
use crate::secrets::{self, SecretsError, Stored};

/// Signed-in accounts' shared state and the work on their saved sign-ins:
/// saving, deleting and renewing tokens, remembered passwords and the
/// offline list. Per account, one renewal or keyring write at a time
/// (spec §6), and work scheduled before a sign-out never saves afterwards
/// (spec §6: late callbacks must not restore the account).
pub struct Accounts {
    /// The client identity (its token endpoint and keyring attributes).
    pub(super) config: ClientConfig,
    /// Per account ID.
    state: Mutex<HashMap<String, AccountState>>,
}

#[derive(Default)]
struct AccountState {
    /// Held during a renewal or keyring write for this account. Per
    /// account, so one account waiting (for example on a keyring prompt)
    /// doesn't hold up another's.
    busy: Arc<Mutex<()>>,
    /// Sign-outs so far; work records the count it was scheduled under.
    sign_outs: u64,
    /// The newest tokens (spec §6: a renewal must not reuse a refresh
    /// token another renewal already replaced). Connections and refreshes
    /// each hold their own copy; renewals look here first.
    latest: Option<Arc<SignedIn>>,
    /// Per desktop: "Forget password" so far, like `sign_outs`.
    forgets: HashMap<String, u64>,
    /// Per desktop: the forget count the stored password was saved under.
    /// A Forget whose deletion runs after a newer save (one scheduled after
    /// that Forget) leaves the newer password alone.
    saved_after: HashMap<String, u64>,
}

/// What a remembered-password save was scheduled under: the account's
/// sign-outs and the desktop's forgets at that moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordTicket {
    sign_outs: u64,
    forgets: u64,
}

/// Why a refresh token wasn't saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotSaved {
    /// The token response had none.
    NoRefreshToken,
    Keyring(SecretsError),
}

impl Accounts {
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            state: Mutex::default(),
        }
    }

    /// Runs `f` on the account's state.
    fn with<R>(&self, id: &str, f: impl FnOnce(&mut AccountState) -> R) -> R {
        let mut all = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(all.entry(id.to_owned()).or_default())
    }

    /// The account's lock for renewals and keyring writes.
    pub(super) fn busy(&self, id: &str) -> Arc<Mutex<()>> {
        self.with(id, |a| a.busy.clone())
    }

    pub fn sign_out_count(&self, id: &str) -> u64 {
        self.with(id, |a| a.sign_outs)
    }

    /// From now on, work that started earlier for this account saves nothing
    /// and its newest tokens are forgotten.
    pub fn note_sign_out(&self, id: &str) {
        self.with(id, |a| {
            a.sign_outs += 1;
            a.latest = None;
        });
    }

    /// Records `account`'s tokens as the newest, unless newer ones are known
    /// or the account was signed out since `started`.
    pub(super) fn note_tokens(&self, account: &Arc<SignedIn>, started: u64) {
        self.with(&account_id(&account.identity), |a| {
            let newer = a
                .latest
                .as_ref()
                .is_none_or(|known| account.tokens.expires_at >= known.tokens.expires_at);
            if a.sign_outs == started && newer {
                a.latest = Some(account.clone());
            }
        });
    }

    /// The newest tokens known for the account, if newer than `account`'s.
    pub(super) fn newest(&self, account: &Arc<SignedIn>) -> Option<Arc<SignedIn>> {
        self.with(&account_id(&account.identity), |a| {
            a.latest
                .clone()
                .filter(|known| known.tokens.expires_at > account.tokens.expires_at)
        })
    }

    /// An interactive sign-in finished: its tokens are the newest.
    pub fn signed_in(&self, account: &Arc<SignedIn>, started: u64) {
        self.note_tokens(account, started);
    }

    /// Saves the account's refresh token, unless it was signed out since
    /// `started` (a [`Self::sign_out_count`]).
    pub fn save(&self, account: &SignedIn, started: u64) -> Result<(), NotSaved> {
        let busy = self.busy(&account_id(&account.identity));
        let _busy = busy.lock();
        self.save_while_busy(account, started)
    }

    /// [`Self::save`], for a caller that holds the account's lock.
    pub(super) fn save_while_busy(&self, account: &SignedIn, started: u64) -> Result<(), NotSaved> {
        if self.sign_out_count(&account_id(&account.identity)) != started {
            crate::app_log!("saved sign-in: not stored (signed out meanwhile)");
            return Ok(());
        }
        let Some(refresh_token) = &account.tokens.refresh_token else {
            crate::app_log!("saved sign-in: not stored (no refresh token in the response)");
            return Err(NotSaved::NoRefreshToken);
        };
        let result = secrets::save(&self.config, &account.identity, refresh_token);
        // Diagnostics: outcome and error kind only, never tokens or IDs.
        match &result {
            Ok(()) => crate::app_log!("saved sign-in: stored"),
            Err(e) => crate::app_log!("saved sign-in: not stored ({e})"),
        }
        result.map_err(NotSaved::Keyring)
    }

    /// Signs the account out: from now on its tokens aren't saved, and its
    /// keyring items (sign-in and remembered passwords) are deleted.
    pub fn delete(&self, identity: &Identity) -> Result<(), SecretsError> {
        let id = account_id(identity);
        self.note_sign_out(&id);
        let busy = self.busy(&id);
        let _busy = busy.lock();
        let deleted = secrets::delete(&self.config, identity);
        crate::app_log!("saved sign-in: deleted (sign-out): {deleted:?}");
        // Under the same lock as their saves (`save_password`).
        let passwords = secrets::delete_passwords(identity, None);
        crate::app_log!(
            "remembered passwords deleted (sign-out): {:?}",
            passwords.is_ok()
        );
        deleted.and(passwords)
    }

    /// Removes an account's cookies, RAM cookie folder and offline desktop
    /// list (sign-out, spec §6: credentials and account caches). Under the
    /// account's lock, like the offline list's saves.
    pub fn remove_files(&self, id: &str) -> Result<(), ()> {
        let busy = self.busy(id);
        let _busy = busy.lock();
        let mut ok = crate::cache::Store::user().is_none_or(|s| s.remove(id).is_ok());
        for dir in [profile_dir(id), jars_root().map(|j| j.join(id))]
            .into_iter()
            .flatten()
        {
            if dir.exists() && fs::remove_dir_all(&dir).is_err() {
                ok = false;
            }
        }
        if ok { Ok(()) } else { Err(()) }
    }

    /// Saves the account's offline list, unless it was signed out since
    /// `started`. Under the account's lock, like [`Self::remove_files`], so
    /// a save queued before a sign-out can't bring the list back after it.
    pub fn save_cache(
        &self,
        store: &crate::cache::Store,
        id: &str,
        list: &[crate::cache::CachedWorkspace],
        started: u64,
    ) -> Result<(), String> {
        let busy = self.busy(id);
        let _busy = busy.lock();
        if self.sign_out_count(id) != started {
            crate::app_log!("offline desktop list: not saved (signed out meanwhile)");
            return Ok(());
        }
        store.save(id, list)
    }

    pub fn password_ticket(&self, id: &str, desktop: &str) -> PasswordTicket {
        self.with(id, |a| PasswordTicket {
            sign_outs: a.sign_outs,
            forgets: a.forgets.get(desktop).copied().unwrap_or(0),
        })
    }

    /// The user forgot this desktop's password: saves scheduled earlier
    /// write nothing. Call at once, before deleting; give the result to
    /// [`Self::forget_password`].
    pub fn note_forget(&self, id: &str, desktop: &str) -> u64 {
        self.with(id, |a| {
            let n = a.forgets.entry(desktop.to_owned()).or_insert(0);
            *n += 1;
            *n
        })
    }

    /// Remembers a Windows password, unless the account signed out or the
    /// password was forgotten since `ticket`. `Ok(false)`: not saved for
    /// that reason. Under the account's lock, like the deletions.
    pub fn save_password(
        &self,
        identity: &Identity,
        desktop: &str,
        title: &str,
        password: &Secret,
        ticket: PasswordTicket,
    ) -> Result<bool, SecretsError> {
        self.save_guarded(&account_id(identity), desktop, ticket, || {
            secrets::save_password(identity, desktop, title, password)
        })
    }

    /// Deletes one desktop's remembered password. `forget`: what
    /// [`Self::note_forget`] returned for this Forget.
    pub fn forget_password(
        &self,
        identity: &Identity,
        desktop: &str,
        forget: u64,
    ) -> Result<(), SecretsError> {
        self.forget_guarded(&account_id(identity), desktop, forget, || {
            secrets::delete_passwords(identity, Some(desktop))
        })
    }

    /// A saved plain RDP connection's password (ADR 0029), with the same
    /// guards as a desktop's: the connection's ID stands for both the
    /// account and the desktop.
    pub fn save_connection_password(
        &self,
        connection: &str,
        title: &str,
        password: &Secret,
        ticket: PasswordTicket,
    ) -> Result<bool, SecretsError> {
        self.save_guarded(connection, connection, ticket, || {
            secrets::save_connection_password(connection, title, password)
        })
    }

    /// Deletes a saved connection's password (Forget, or removing it).
    pub fn forget_connection_password(
        &self,
        connection: &str,
        forget: u64,
    ) -> Result<(), SecretsError> {
        self.forget_guarded(connection, connection, forget, || {
            secrets::delete_connection_password(connection)
        })
    }

    /// Runs `save` under `id`'s lock, unless `id` signed out or `desktop`'s
    /// password was forgotten since `ticket`.
    fn save_guarded(
        &self,
        id: &str,
        desktop: &str,
        ticket: PasswordTicket,
        save: impl FnOnce() -> Result<(), SecretsError>,
    ) -> Result<bool, SecretsError> {
        let busy = self.busy(id);
        let _busy = busy.lock();
        if self.password_ticket(id, desktop) != ticket {
            crate::app_log!("remembered password: not stored (signed out or forgotten meanwhile)");
            return Ok(false);
        }
        save()?;
        self.with(id, |a| {
            a.saved_after.insert(desktop.to_owned(), ticket.forgets);
        });
        Ok(true)
    }

    /// Runs `delete` under `id`'s lock, unless a password was saved after
    /// this Forget was asked for: that is the user's newer choice.
    fn forget_guarded(
        &self,
        id: &str,
        desktop: &str,
        forget: u64,
        delete: impl FnOnce() -> Result<(), SecretsError>,
    ) -> Result<(), SecretsError> {
        let busy = self.busy(id);
        let _busy = busy.lock();
        let newer = self.with(id, |a| {
            a.saved_after
                .get(desktop)
                .is_some_and(|&saved| saved >= forget)
        });
        if newer {
            return Ok(());
        }
        delete()
    }

    /// All saved sign-ins.
    pub fn load_saved(&self) -> Result<Vec<Stored>, SecretsError> {
        match secrets::load_all(&self.config) {
            Ok(all) => {
                crate::app_log!("saved sign-in: {} found", all.len());
                Ok(all)
            }
            Err(e) => {
                crate::app_log!("saved sign-in: can't load ({e})");
                Err(e)
            }
        }
    }
}
