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
                return self.discovered(&id, epoch, generation, result);
            }
            Message::IconsLoaded(id, epoch, icons) => self.icons_loaded(&id, epoch, icons),
            Message::CacheLoaded(id, epoch, cached) => self.cache_loaded(&id, epoch, cached),
            Message::SignOut(id) => return self.ask_sign_out(&id),
            Message::SignOutSelected => {
                if let Some(Page::Account(id)) = self.selected_page() {
                    return self.ask_sign_out(&id);
                }
            }
            Message::SignOutConfirm => {
                if let Some(id) = self.sign_out_asking.take() {
                    return self.sign_out(&id);
                }
            }
            Message::SignOutCancel => self.sign_out_asking = None,
            Message::SignOutFinished(id, identity, result) => {
                if let Err(e) = result {
                    crate::app_log!("sign-out: keyring not cleaned ({e})");
                    self.cleanup_retry(&id).keyring = Some(*identity);
                }
            }
            Message::RetrySignOutCleanup => return self.retry_sign_out_cleanup(),
            Message::DismissSignOutCleanup => self.cleanup_retry = None,
            Message::SignOutFilesRemoved(id, result) => {
                if result.is_err() {
                    crate::app_log!("sign-out: sign-in files not removed");
                    self.cleanup_retry(&id).files = true;
                }
                // A session still starting removes the files once more
                // when it has been stopped (`launched`).
                if !self.launching.values().any(|l| l.account == id) {
                    self.signing_out.remove(&id);
                }
                if self.quit == Quit::Stopping {
                    return self.continue_quit();
                }
            }
            // `update` routes only this handler's messages here.
            other => misrouted(&other),
        }
        Task::none()
    }

    /// Start: one entry per saved sign-in, each restored in the background.
    pub(super) fn saved_loaded(
        &mut self,
        result: Result<Vec<Stored>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.loading_saved = false;
        let loaded_ok = result.is_ok();
        let saved = match result {
            Ok(saved) => saved,
            Err(e) => {
                self.notice = Some(e);
                Vec::new()
            }
        };
        // Offline lists of accounts that are no longer saved go; only when
        // the saved list could be read, so a locked keyring deletes nothing.
        let known: Option<Vec<String>> = loaded_ok.then(|| {
            saved
                .iter()
                .map(|s| accounts::account_id(&s.identity))
                .collect()
        });
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
                last_refreshed: None,
                cached: None,
                epoch,
            });
            tasks.push(self.load_cached(&id, epoch));
            tasks.push(self.start_restore(&id));
        }
        if let (Some(store), Some(known)) = (self.cache.clone(), known) {
            tasks.push(
                Task::future(async move {
                    let _ = tokio::task::spawn_blocking(move || store.remove_others(&known)).await;
                })
                .discard(),
            );
        }
        self.select_first_account();
        Task::batch(tasks)
    }

    /// Reads an account's offline list in the background.
    fn load_cached(&self, id: &str, epoch: u64) -> Task<cosmic::Action<Message>> {
        let Some(store) = self.cache.clone() else {
            return Task::none();
        };
        let keep =
            Duration::from_secs(u64::from(self.account_settings(Some(id)).offline_days) * 86_400);
        let id = id.to_owned();
        cosmic::task::future(async move {
            let owner = id.clone();
            let cached = tokio::task::spawn_blocking(move || store.load(&owner, keep))
                .await
                .ok()
                .flatten();
            Message::CacheLoaded(id, epoch, cached)
        })
    }

    pub(super) fn cache_loaded(
        &mut self,
        id: &str,
        epoch: u64,
        cached: Option<crate::cache::Cached>,
    ) {
        if let Some(a) = self.account_mut(id).filter(|a| a.epoch == epoch)
            // A live list that loaded first wins.
            && a.last_refreshed.is_none()
        {
            a.cached = cached.map(from_cached);
        }
    }

    /// Saves the live list as the account's offline list, in the
    /// background.
    fn save_cached(&self, id: &str, list: &[WorkspaceView]) -> Task<cosmic::Action<Message>> {
        let Some(store) = self.cache.clone() else {
            return Task::none();
        };
        let (id, list) = (id.to_owned(), to_cached(list));
        Task::future(async move {
            let result = tokio::task::spawn_blocking(move || store.save(&id, &list)).await;
            if let Ok(Err(e)) = result {
                crate::app_log!("offline desktop list not saved: {e}");
            }
        })
        .discard()
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
        let (Some(helper), Some(profile)) = (signin::find_helper(), accounts::prepared_profile(id))
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
        let time = Duration::from_secs(
            u64::from(self.account_settings(origin.as_deref()).sign_in_mins) * 60,
        );
        cosmic::task::future(async move {
            let used = profile.clone();
            let result = tokio::task::spawn_blocking(move || {
                signin::sign_in(
                    &http,
                    &ClientConfig::public_remote_desktop(),
                    &helper,
                    &profile,
                    login_hint.as_deref(),
                    time,
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
            // Cancelled meanwhile: a new profile that won't be used.
            accounts::remove_unused_profile(profile);
            return Task::none();
        }
        let signed_in = match result {
            Ok(s) => s,
            Err(SignInFailure(error)) => {
                match origin {
                    None => {
                        accounts::remove_unused_profile(profile);
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
        if self.signing_out.contains(&id) {
            self.refuse_while_signing_out(origin, generation, profile);
            return Task::none();
        }
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
                last_refreshed: None,
                cached: None,
                epoch,
            }),
        }
        self.rebuild_nav(&Page::Account(id.clone()));
        let started = accounts::sign_out_count(&id);
        accounts::signed_in(&signed_in, started);
        let save_id = id.clone();
        let save = cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || accounts::save(&signed_in, started))
                .await
                .unwrap_or_else(|_| Err(fl!("error-generic")));
            Message::Saved(save_id, generation, result)
        });
        Task::batch([save, self.load_resources(&id)])
    }

    /// A sign-in finished for an account whose sign-out is still deleting
    /// its files: adopting this profile now would lose it.
    fn refuse_while_signing_out(&mut self, origin: Option<&str>, generation: u64, profile: &Path) {
        accounts::remove_unused_profile(profile);
        let error = Some(fl!("error-still-signing-out"));
        match origin {
            None => {
                self.adding = Some(Adding {
                    generation,
                    cancel: None,
                    error,
                });
            }
            Some(other) => {
                if let Some(a) = self.account_mut(other) {
                    a.state = AccountState::NeedsSignIn { error };
                }
            }
        }
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
                let (feeds, signed_in) =
                    match discovery::discover(&http, &signed_in.tokens.access_token) {
                        // Refused although it looked valid: renew once and retry.
                        Err(FetchError::Http(HttpError::Status { status: 401, .. })) => {
                            crate::app_log!("desktop list: access token refused; renewing");
                            let renewed = accounts::renew_now(&http, signed_in)?;
                            let feeds = discovery::discover(&http, &renewed.tokens.access_token)
                                .map_err(refused_or_shown)?;
                            (feeds, renewed)
                        }
                        other => (
                            other.map_err(|e| StepError::Show(describe_fetch_error(&e)))?,
                            signed_in,
                        ),
                    };
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

    /// Refreshes the desktop lists not refreshed for their account's
    /// refresh interval.
    pub(super) fn refresh_due(&mut self) -> Task<cosmic::Action<Message>> {
        let due: Vec<String> = self
            .accounts
            .iter()
            .filter(|a| {
                let every = Duration::from_secs(
                    u64::from(self.account_settings(Some(&a.id)).refresh_mins) * 60,
                );
                matches!(a.state, AccountState::Ready { loading: false, .. })
                    && a.last_load
                        .is_none_or(|t| t.elapsed().map_or(true, |d| d >= every))
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
    ) -> Task<cosmic::Action<Message>> {
        // Renewed tokens are kept even if a newer refresh started meanwhile.
        if let Ok((_, renewed)) = &result {
            self.adopt_tokens(id, epoch, renewed);
        }
        let Some(account) = self.account_mut(id).filter(|a| a.generation == generation) else {
            return Task::none();
        };
        let result = match result {
            Ok((list, _)) => Ok(list),
            Err(StepError::Show(e)) => Err(e),
            Err(StepError::SignInAgain(e)) => {
                self.needs_sign_in(id, e);
                return Task::none();
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
                    let pairs: Vec<(String, String)> = list
                        .iter()
                        .flat_map(|w| &w.resources)
                        .map(|r| (r.legacy_key.clone(), r.key.clone()))
                        .collect();
                    let save = list.clone();
                    *workspaces = list;
                    *error = None;
                    account.last_refreshed = Some(SystemTime::now());
                    account.cached = None;
                    self.migrate_resource_keys(&pairs);
                    return Task::batch([self.fetch_icons(id), self.save_cached(id, &save)]);
                }
                Err(e) => *error = Some(e),
            }
        }
        Task::none()
    }

    /// Fetches the icons of `id`'s desktops that aren't cached yet, in the
    /// background, and forgets the ones no longer listed.
    fn fetch_icons(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        /// Bounds the requests one refresh makes.
        const MAX_ICONS: usize = 200;
        let Some(account) = self.account(id) else {
            return Task::none();
        };
        let Some(signed_in) = account.signed_in().cloned() else {
            return Task::none();
        };
        let epoch = account.epoch;
        let listed: HashSet<Url> = account
            .workspaces()
            .iter()
            .flat_map(|w| &w.resources)
            .filter_map(|r| r.icon.clone())
            .collect();
        self.icons
            .retain(|(account, url), _| account != id || listed.contains(url));
        let missing: Vec<Url> = listed
            .into_iter()
            .filter(|u| !self.icons.contains_key(&(id.to_owned(), u.clone())))
            .take(MAX_ICONS)
            .collect();
        if missing.is_empty() {
            return Task::none();
        }
        let http = self.http.clone();
        let id = id.to_owned();
        cosmic::task::future(async move {
            let icons = tokio::task::spawn_blocking(move || {
                let mut icons = Vec::new();
                let mut refused = 0;
                for url in missing {
                    let fetched = http
                        .get(&url, Role::Icon, Some(&signed_in.tokens.access_token))
                        .ok()
                        .filter(|resp| icons::check_png(&resp.body).is_ok());
                    match fetched {
                        Some(resp) => icons.push((url, resp.body)),
                        None => refused += 1,
                    }
                }
                crate::app_log!(
                    "Desktop icons: {} loaded, {refused} failed or refused",
                    icons.len()
                );
                icons
            })
            .await
            .unwrap_or_default();
            Message::IconsLoaded(id, epoch, icons)
        })
    }

    fn icons_loaded(&mut self, id: &str, epoch: u64, icons: Vec<(Url, Vec<u8>)>) {
        // Not for an account signed out (or in again) meanwhile.
        if self.account(id).is_none_or(|a| a.epoch != epoch) {
            return;
        }
        for (url, bytes) in icons {
            self.icons
                .insert((id.to_owned(), url), widget::icon::from_raster_bytes(bytes));
        }
    }

    /// Saved per-desktop settings under keys from before workspaces were
    /// part of them move to the new keys.
    fn migrate_resource_keys(&mut self, pairs: &[(String, String)]) {
        let Some(handler) = &self.config_handler else {
            return;
        };
        if let Some(map) = migrate_keys(&self.config.logon_settings, pairs) {
            let _ = self.config.set_logon_settings(handler, map);
        }
        if let Some(map) = migrate_keys(&self.config.desktop_settings, pairs) {
            let _ = self.config.set_desktop_settings(handler, map);
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

    /// The leftovers of this account's sign-out (a newer one replaces an
    /// older one's).
    fn cleanup_retry(&mut self, id: &str) -> &mut CleanupRetry {
        if self.cleanup_retry.as_ref().is_some_and(|r| r.id != id) {
            self.cleanup_retry = None;
        }
        self.cleanup_retry.get_or_insert_with(|| CleanupRetry {
            id: id.to_owned(),
            keyring: None,
            files: false,
        })
    }

    /// Removes what a sign-out left behind. Not when the account has been
    /// signed in again meanwhile: its data is in use again.
    fn retry_sign_out_cleanup(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(retry) = self.cleanup_retry.take() else {
            return Task::none();
        };
        if self.account(&retry.id).is_some() {
            return Task::none();
        }
        let mut tasks = Vec::new();
        if retry.files {
            let id = retry.id.clone();
            tasks.push(cosmic::task::future(async move {
                let owner = id.clone();
                let result = tokio::task::spawn_blocking(move || accounts::remove_files(&owner))
                    .await
                    .unwrap_or(Err(()));
                Message::SignOutFilesRemoved(id, result)
            }));
        }
        if let Some(identity) = retry.keyring {
            tasks.push(keyring_cleanup(retry.id, identity));
        }
        Task::batch(tasks)
    }

    /// The account's open connections: running or starting.
    fn open_sessions(&self, id: &str) -> usize {
        self.running.values().filter(|r| r.account == id).count()
            + self.launching.values().filter(|l| l.account == id).count()
    }

    /// Sign-out asks first while the account has desktops or apps open
    /// (spec §6: disconnect and sign out, or cancel).
    fn ask_sign_out(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        if self.open_sessions(id) == 0 {
            return self.sign_out(id);
        }
        self.sign_out_asking = Some(id.to_owned());
        Task::none()
    }

    pub(super) fn sign_out_dialog(&self, id: &str) -> Element<'_, Message> {
        let label = self.account(id).map(Account::label).unwrap_or_default();
        let open = u32::try_from(self.open_sessions(id)).unwrap_or(u32::MAX);
        widget::dialog()
            .title(fl!("sign-out-title", account = label))
            .body(fl!("sign-out-body", count = open))
            .primary_action(
                widget::button::destructive(fl!("sign-out-disconnect"))
                    .on_press(Message::SignOutConfirm),
            )
            .secondary_action(
                widget::button::standard(fl!("cancel")).on_press(Message::SignOutCancel),
            )
            .into()
    }

    /// Signs one account out: ends its sessions, removes it from the list,
    /// deletes its cookies and keyring item (spec §6).
    pub(super) fn sign_out(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let Some(pos) = self.accounts.iter().position(|a| a.id == id) else {
            return Task::none();
        };
        let account = self.accounts.remove(pos);
        self.icons.retain(|(account, _), _| account != id);
        // From now on nothing started earlier saves its tokens.
        accounts::note_sign_out(id);
        let sign_in = match &account.state {
            AccountState::SigningIn { cancel } => {
                cancel.cancel();
                Some(cancel.clone())
            }
            _ => None,
        };
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
                .filter(|(_, l)| l.account == id)
                .map(|(k, _)| k.clone()),
        );
        let mut sessions = Vec::new();
        for key in keys {
            if let Some(r) = self.running.remove(&key) {
                sessions.push(r.session);
            }
            // Stopped as soon as it has started (`launched`).
            if let Some(l) = self.launching.get_mut(&key) {
                l.stop = true;
            }
            if let Some(stop) = self.movers.remove(&key) {
                stop.store(true, Ordering::SeqCst);
            }
            self.connections.remove(&key);
            self.pending_logon.remove(&key);
            self.last_failure.remove(&key);
            self.reconnects.remove(&key);
            self.pending_password.remove(&key);
            self.set_saved_password(&key, false);
            self.entra_failed.remove(&key);
            self.entra_failures.remove(&key);
        }
        if self.dialog.as_ref().is_some_and(|d| d.account == id) {
            self.dialog = None;
        }
        self.signing_out.insert(id.to_owned());
        self.select_first_account();
        // Its cookies go only after everything that could still write them
        // has ended: the sign-in window and FreeRDP with its sign-in helper
        // (spec §6). Blocking, so not on the UI thread.
        let files = {
            let id = id.to_owned();
            cosmic::task::future(async move {
                let owner = id.clone();
                let result = tokio::task::spawn_blocking(move || {
                    if let Some(cancel) = sign_in {
                        cancel.cancel_and_wait(launcher::STOP_GRACE);
                    }
                    launcher::stop_all(&sessions, launcher::STOP_GRACE);
                    accounts::remove_files(&owner)
                })
                .await
                .unwrap_or(Err(()));
                Message::SignOutFilesRemoved(id, result)
            })
        };
        let keyring = keyring_cleanup(id.to_owned(), account.identity);
        Task::batch([files, keyring])
    }
}

/// Deletes an account's keyring entries: its saved sign-in and its
/// remembered Windows passwords (spec §8). The keyring may block (for
/// example on an unlock prompt), so on its own.
fn keyring_cleanup(
    id: String,
    identity: crate::auth::id_token::Identity,
) -> Task<cosmic::Action<Message>> {
    cosmic::task::future(async move {
        let kept = identity.clone();
        let result = tokio::task::spawn_blocking(move || {
            let passwords = crate::secrets::delete_passwords(&identity, None);
            crate::app_log!(
                "remembered passwords deleted (sign-out): {:?}",
                passwords.is_ok()
            );
            let signed_out = accounts::delete(&identity);
            signed_out.and(passwords.map_err(|_| fl!("error-sign-out-incomplete")))
        })
        .await
        .unwrap_or_else(|_| Err(fl!("error-sign-out-incomplete")));
        Message::SignOutFinished(id, Box::new(kept), result)
    })
}
