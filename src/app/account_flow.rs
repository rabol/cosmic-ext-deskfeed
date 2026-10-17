// SPDX-License-Identifier: MIT

//! Account life cycle: restore from the keyring, add, sign in again, load desktops, sign out.

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;

impl AppModel {
    /// Account messages: saved sign-ins, sign-in, desktop lists, sign-out. Routed here by `update`.
    pub(super) fn account_message(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::SavedLoaded(result) => return self.saved_loaded(result),
            Message::Restored(id, generation, outcome) => {
                return self.restored(&id, generation, outcome);
            }
            Message::RetryRestore(id) => return self.start_restore(&id),
            Message::AddAccount => return self.start_add(),
            Message::SignInAccount(id) => return self.start_account_sign_in(&id),
            Message::CancelSignIn(None) => {
                if let Some(Adding {
                    cancel: Some(c), ..
                }) = &self.adding
                {
                    c.cancel();
                }
                self.adding = None;
            }
            Message::CancelSignIn(Some(id)) => {
                let generation = self.next();
                if let Some(a) = self.account_mut(&id) {
                    if let AccountState::SigningIn { cancel } = &a.state {
                        cancel.cancel();
                    }
                    a.generation = generation;
                    a.state = AccountState::NeedsSignIn { error: None };
                }
            }
            Message::SignInFinished(origin, generation, profile, result) => {
                return self.sign_in_finished(origin.as_deref(), generation, &profile, result);
            }
            Message::Saved(id, sign_in, result) => {
                if let Some(a) = self.account_mut(&id)
                    && a.sign_in == sign_in
                {
                    a.notice = result.err();
                }
            }
            Message::Refresh(id) => return self.load_resources(&id),
            Message::RefreshDue => return self.refresh_due(),
            Message::Discovered(id, epoch, generation, result) => {
                self.discovered(&id, epoch, generation, result);
            }
            Message::SignOut(id) => return self.sign_out(&id),
            Message::SignOutSelected => {
                if let Some(Page::Account(id)) = self.selected_page() {
                    return self.sign_out(&id);
                }
            }
            Message::SignOutFinished(result) => {
                if let Err(e) = result {
                    self.notice = Some(e);
                }
            }
            // `update` routes only this handler's messages here.
            other => debug_assert!(false, "message routed to the wrong handler: {other:?}"),
        }
        Task::none()
    }

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
                sign_in: 0,
                last_load: None,
                epoch,
            });
            tasks.push(self.start_restore(&id));
        }
        self.select_first_account();
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
        origin: Option<&str>,
        generation: u64,
        profile: &Path,
        result: Result<Arc<SignedIn>, SignInFailure>,
    ) -> Task<cosmic::Action<Message>> {
        let current = match origin {
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
                match origin {
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
        match decide::after_sign_in(origin, &id) {
            decide::AfterSignIn::AdoptProfile => {
                self.adding = None;
                accounts::adopt_profile(profile, &id);
            }
            decide::AfterSignIn::ResetOther(old) => {
                // The profile now holds this account's cookies.
                accounts::adopt_profile(profile, &id);
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
                a.sign_in = generation;
            }
            None => self.accounts.push(Account {
                id: id.clone(),
                identity: signed_in.identity.clone(),
                state,
                stored: None,
                notice: None,
                can_retry: false,
                generation,
                sign_in: generation,
                last_load: None,
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
        account.last_load = Some(SystemTime::now());
        let signed_in = signed_in.clone();
        let epoch = account.epoch;
        let http = self.http.clone();
        let id = id.to_owned();
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                // An expired access token would only get "session expired".
                let signed_in = accounts::renew_if_needed(&http, signed_in)?;
                let feeds = discovery::discover(&http, &signed_in.tokens.access_token)
                    .map_err(|e| StepError::Show(describe_fetch_error(&e)))?;
                let list = feeds
                    .into_iter()
                    .map(|f| workspace_view(&signed_in, f))
                    .collect();
                Ok((list, signed_in))
            })
            .await
            .unwrap_or_else(|_| Err(StepError::Show(fl!("error-generic"))));
            Message::Discovered(id, epoch, generation, result)
        })
    }

    /// Refreshes the desktop lists not refreshed for [`REFRESH_EVERY`].
    pub(super) fn refresh_due(&mut self) -> Task<cosmic::Action<Message>> {
        let due: Vec<String> = self
            .accounts
            .iter()
            .filter(|a| {
                matches!(a.state, AccountState::Ready { loading: false, .. })
                    && a.last_load
                        .is_none_or(|t| t.elapsed().map_or(true, |d| d >= REFRESH_EVERY))
            })
            .map(|a| a.id.clone())
            .collect();
        Task::batch(due.iter().map(|id| self.load_resources(id)))
    }

    pub(super) fn discovered(
        &mut self,
        id: &str,
        epoch: u64,
        generation: u64,
        result: Result<(Vec<WorkspaceView>, Arc<SignedIn>), StepError>,
    ) {
        // Renewed tokens are kept even if a newer refresh started meanwhile.
        if let Ok((_, renewed)) = &result {
            self.adopt_tokens(id, epoch, renewed);
        }
        let Some(account) = self.account_mut(id).filter(|a| a.generation == generation) else {
            return;
        };
        let result = match result {
            Ok((list, _)) => Ok(list),
            Err(StepError::Show(e)) => Err(e),
            Err(StepError::SignInAgain(e)) => {
                self.needs_sign_in(id, e);
                return;
            }
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

    /// Takes renewed tokens for the account entry `epoch`, when they are
    /// newer than the ones it has.
    pub(super) fn adopt_tokens(&mut self, id: &str, epoch: u64, renewed: &Arc<SignedIn>) {
        if let Some(account) = self.account_mut(id).filter(|a| a.epoch == epoch)
            && let AccountState::Ready { signed_in, .. } = &mut account.state
            && signed_in.identity == renewed.identity
            && renewed.tokens.expires_at > signed_in.tokens.expires_at
        {
            *signed_in = renewed.clone();
        }
    }

    /// The account's sign-in can't be renewed: it asks for an interactive
    /// sign-in, with the reason. Its sessions keep running.
    pub(super) fn needs_sign_in(&mut self, id: &str, error: String) {
        let generation = self.next();
        if let Some(account) = self.account_mut(id)
            && matches!(account.state, AccountState::Ready { .. })
        {
            account.generation = generation;
            account.state = AccountState::NeedsSignIn { error: Some(error) };
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
        // Its desktops by the sessions' owner, not only the current list: a
        // refresh may have removed a desktop that is still connected.
        let mut keys: HashSet<String> = account.resource_keys().into_iter().collect();
        keys.extend(
            self.running
                .iter()
                .filter(|(_, r)| r.account == id)
                .map(|(k, _)| k.clone()),
        );
        keys.extend(
            self.launching
                .iter()
                .filter(|(_, (owner, _))| owner == id)
                .map(|(k, _)| k.clone()),
        );
        for key in keys {
            if let Some(r) = self.running.remove(&key) {
                r.session.disconnect();
            }
            // Stopped as soon as it has started (`launched`).
            if let Some((_, stop)) = self.launching.get_mut(&key) {
                *stop = true;
            }
            if let Some(stop) = self.movers.remove(&key) {
                stop.store(true, Ordering::SeqCst);
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
        self.select_first_account();
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
