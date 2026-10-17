// SPDX-License-Identifier: MIT

//! Account life cycle: restore from the keyring, add, sign in again, load desktops, sign out.

use super::*;

impl AppModel {
    /// Start: one entry per saved sign-in, each restored in the background.
    pub(super) fn saved_loaded(
        &mut self,
        result: Result<Vec<Stored>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.loading_saved = false;
        let saved = match result {
            Ok(saved) => saved,
            Err(e) => {
                self.notice = Some(e);
                Vec::new()
            }
        };
        let mut tasks = Vec::new();
        for stored in saved {
            let id = accounts::account_id(&stored.identity);
            if self.account(&id).is_some() {
                continue;
            }
            accounts::adopt_legacy_profile(&id);
            let epoch = self.next();
            self.accounts.push(Account {
                id: id.clone(),
                identity: stored.identity.clone(),
                state: AccountState::Restoring,
                stored: Some(stored),
                notice: None,
                can_retry: false,
                generation: 0,
                epoch,
            });
            tasks.push(self.start_restore(&id));
        }
        let select = self
            .accounts
            .first()
            .map_or(Page::Add, |a| Page::Account(a.id.clone()));
        self.rebuild_nav(&select);
        Task::batch(tasks)
    }

    /// Signs in with the account's saved refresh token.
    pub(super) fn start_restore(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let generation = self.next();
        let Some(account) = self.account_mut(id) else {
            return Task::none();
        };
        let Some(stored) = account.stored.clone() else {
            account.state = AccountState::NeedsSignIn { error: None };
            return Task::none();
        };
        account.generation = generation;
        account.state = AccountState::Restoring;
        account.can_retry = false;
        let http = self.http.clone();
        let id = id.to_owned();
        cosmic::task::future(async move {
            let outcome = tokio::task::spawn_blocking(move || accounts::restore(&http, &stored))
                .await
                .unwrap_or_else(|_| Restore::Retry(fl!("error-generic")));
            Message::Restored(id, generation, outcome)
        })
    }

    pub(super) fn restored(
        &mut self,
        id: &str,
        generation: u64,
        outcome: Restore,
    ) -> Task<cosmic::Action<Message>> {
        let Some(account) = self.account_mut(id).filter(|a| a.generation == generation) else {
            return Task::none();
        };
        match outcome {
            Restore::Ready(signed_in, notice) => {
                account.identity = signed_in.identity.clone();
                account.notice = notice;
                account.state = AccountState::Ready {
                    signed_in,
                    loading: true,
                    workspaces: Vec::new(),
                    error: None,
                };
                self.load_resources(id)
            }
            Restore::SignInAgain(error) => {
                account.stored = None;
                account.state = AccountState::NeedsSignIn { error: Some(error) };
                Task::none()
            }
            Restore::Retry(error) => {
                account.can_retry = true;
                account.state = AccountState::NeedsSignIn { error: Some(error) };
                Task::none()
            }
        }
    }

    /// Opens the sign-in window for an account not in the list yet, with a
    /// fresh cookie profile.
    pub(super) fn start_add(&mut self) -> Task<cosmic::Action<Message>> {
        if matches!(
            self.adding,
            Some(Adding {
                cancel: Some(_),
                ..
            })
        ) {
            return Task::none();
        }
        self.rebuild_nav(&Page::Add);
        let generation = self.next();
        let profile = accounts::new_profile_dir();
        let error = |e| Adding {
            generation,
            cancel: None,
            error: Some(e),
        };
        let (Some(helper), Some(profile)) = (signin::find_helper(), profile) else {
            self.adding = Some(error(fl!("error-helper-missing")));
            return Task::none();
        };
        let cancel = Cancel::default();
        self.adding = Some(Adding {
            generation,
            cancel: Some(cancel.clone()),
            error: None,
        });
        self.run_sign_in(None, generation, helper, profile, None, cancel)
    }

    /// Signs in again to an account in the list, with its own cookies and
    /// its user name preselected.
    pub(super) fn start_account_sign_in(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let generation = self.next();
        let (Some(helper), Some(profile)) = (signin::find_helper(), accounts::profile_dir(id))
        else {
            if let Some(a) = self.account_mut(id) {
                a.state = AccountState::NeedsSignIn {
                    error: Some(fl!("error-helper-missing")),
                };
            }
            return Task::none();
        };
        let Some(account) = self.account_mut(id) else {
            return Task::none();
        };
        let cancel = Cancel::default();
        account.generation = generation;
        account.state = AccountState::SigningIn {
            cancel: cancel.clone(),
        };
        let hint = account.identity.username.clone();
        self.run_sign_in(
            Some(id.to_owned()),
            generation,
            helper,
            profile,
            hint,
            cancel,
        )
    }

    pub(super) fn run_sign_in(
        &self,
        origin: Option<String>,
        generation: u64,
        helper: PathBuf,
        profile: PathBuf,
        login_hint: Option<String>,
        cancel: Cancel,
    ) -> Task<cosmic::Action<Message>> {
        let http = self.http.clone();
        cosmic::task::future(async move {
            let used = profile.clone();
            let result = tokio::task::spawn_blocking(move || {
                signin::sign_in(
                    &http,
                    &ClientConfig::public_remote_desktop(),
                    &helper,
                    &profile,
                    login_hint.as_deref(),
                    &cancel,
                )
            })
            .await
            .unwrap_or(Err(SignInError::HelperFailed("task".into())));
            Message::SignInFinished(
                origin,
                generation,
                used,
                result.map(Arc::new).map_err(describe_sign_in_error),
            )
        })
    }

    pub(super) fn sign_in_finished(
        &mut self,
        origin: Option<String>,
        generation: u64,
        profile: PathBuf,
        result: Result<Arc<SignedIn>, SignInFailure>,
    ) -> Task<cosmic::Action<Message>> {
        let current = match &origin {
            None => self
                .adding
                .as_ref()
                .is_some_and(|a| a.generation == generation),
            Some(id) => self.account(id).is_some_and(|a| a.generation == generation),
        };
        if !current {
            return Task::none();
        }
        let signed_in = match result {
            Ok(s) => s,
            Err(SignInFailure(error)) => {
                match &origin {
                    None => {
                        self.adding = error.map(|e| Adding {
                            generation,
                            cancel: None,
                            error: Some(e),
                        });
                    }
                    Some(id) => {
                        if let Some(a) = self.account_mut(id) {
                            a.state = AccountState::NeedsSignIn { error };
                        }
                    }
                }
                return Task::none();
            }
        };
        let id = accounts::account_id(&signed_in.identity);
        match decide::after_sign_in(origin.as_deref(), &id) {
            decide::AfterSignIn::AdoptProfile => {
                self.adding = None;
                accounts::adopt_profile(&profile, &id);
            }
            decide::AfterSignIn::ResetOther(old) => {
                // The profile now holds this account's cookies.
                accounts::adopt_profile(&profile, &id);
                if let Some(a) = self.account_mut(&old) {
                    a.state = AccountState::NeedsSignIn { error: None };
                }
            }
            decide::AfterSignIn::Nothing => {}
        }
        let generation = self.next();
        let epoch = self.next();
        let state = AccountState::Ready {
            signed_in: signed_in.clone(),
            loading: true,
            workspaces: Vec::new(),
            error: None,
        };
        match self.account_mut(&id) {
            Some(a) => {
                a.identity = signed_in.identity.clone();
                a.state = state;
                a.notice = None;
                a.can_retry = false;
                a.generation = generation;
            }
            None => self.accounts.push(Account {
                id: id.clone(),
                identity: signed_in.identity.clone(),
                state,
                stored: None,
                notice: None,
                can_retry: false,
                generation,
                epoch,
            }),
        }
        self.rebuild_nav(&Page::Account(id.clone()));
        let started = accounts::sign_out_count(&id);
        let save_id = id.clone();
        let save = cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || accounts::save(&signed_in, started))
                .await
                .unwrap_or_else(|_| Err(fl!("error-generic")));
            Message::Saved(save_id, generation, result)
        });
        Task::batch([save, self.load_resources(&id)])
    }

    pub(super) fn load_resources(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let generation = self.next();
        let Some(account) = self.account_mut(id) else {
            return Task::none();
        };
        let AccountState::Ready {
            signed_in, loading, ..
        } = &mut account.state
        else {
            return Task::none();
        };
        *loading = true;
        account.generation = generation;
        let signed_in = signed_in.clone();
        let http = self.http.clone();
        let id = id.to_owned();
        cosmic::task::future(async move {
            let acc = signed_in.clone();
            let result = tokio::task::spawn_blocking(move || {
                discovery::discover(&http, &acc.tokens.access_token)
            })
            .await;
            let result = match result {
                Ok(Ok(feeds)) => Ok(feeds
                    .into_iter()
                    .map(|f| workspace_view(&signed_in, f))
                    .collect()),
                Ok(Err(e)) => Err(describe_fetch_error(&e)),
                Err(_) => Err(fl!("error-generic")),
            };
            Message::Discovered(id, generation, result)
        })
    }

    pub(super) fn discovered(
        &mut self,
        id: &str,
        generation: u64,
        result: Result<Vec<WorkspaceView>, String>,
    ) {
        let Some(account) = self.account_mut(id).filter(|a| a.generation == generation) else {
            return;
        };
        if let AccountState::Ready {
            loading,
            workspaces,
            error,
            ..
        } = &mut account.state
        {
            *loading = false;
            match result {
                Ok(list) => {
                    *workspaces = list;
                    *error = None;
                }
                Err(e) => *error = Some(e),
            }
        }
    }

    /// Signs one account out: ends its sessions, removes it from the list,
    /// deletes its cookies and keyring item (spec §6).
    pub(super) fn sign_out(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let Some(pos) = self.accounts.iter().position(|a| a.id == id) else {
            return Task::none();
        };
        let account = self.accounts.remove(pos);
        if let AccountState::SigningIn { cancel } = &account.state {
            cancel.cancel();
        }
        for key in account.resource_keys() {
            if let Some(s) = self.running.remove(&key) {
                s.disconnect();
            }
            self.connections.remove(&key);
            self.pending_logon.remove(&key);
            self.last_failure.remove(&key);
            self.entra_failed.remove(&key);
            self.entra_failures.remove(&key);
        }
        if self.dialog.as_ref().is_some_and(|d| d.account == id) {
            self.dialog = None;
        }
        if accounts::remove_files(id).is_err() {
            self.notice = Some(fl!("error-sign-out-incomplete"));
        }
        let select = self
            .accounts
            .first()
            .map_or(Page::Add, |a| Page::Account(a.id.clone()));
        self.rebuild_nav(&select);
        // The keyring may block (for example on an unlock prompt), so not on
        // the UI thread.
        let identity = account.identity;
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || accounts::delete(&identity))
                .await
                .unwrap_or_else(|_| Err(fl!("error-sign-out-incomplete")));
            Message::SignOutFinished(result)
        })
    }
}
