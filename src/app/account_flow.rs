// SPDX-License-Identifier: MIT

//! Account life cycle: restore from the keyring, add, sign in again, load desktops, sign out.

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;

impl AppModel {
    /// Account messages: saved sign-ins, sign-in, desktop lists, sign-out. Routed here by `route`.
    // One `match` over this handler's messages; splitting it would only
    // spread the routing out.
    #[allow(clippy::too_many_lines)]
    pub(super) fn account_message(
        &mut self,
        message: AccountMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            AccountMessage::SavedLoaded(result) => return self.saved_loaded(result),
            AccountMessage::Restored {
                account: id,
                generation,
                outcome,
            } => {
                return self.restored(&id, generation, outcome);
            }
            AccountMessage::RetryRestore(id) => return self.start_restore(&id),
            AccountMessage::AddAccount => return self.start_add(),
            AccountMessage::SignInAccount(id) => return self.start_account_sign_in(&id),
            AccountMessage::CancelSignIn(None) => {
                if let Some(Adding::SigningIn { cancel, .. }) = &self.adding {
                    cancel.cancel();
                }
                self.adding = None;
            }
            AccountMessage::CancelSignIn(Some(id)) => {
                let generation = self.next();
                if let Some(a) = self.account_mut(&id) {
                    if let AccountState::SigningIn { cancel } = &a.state {
                        cancel.cancel();
                    }
                    a.generation = generation;
                    a.state = AccountState::NeedsSignIn { error: None };
                }
            }
            AccountMessage::SignInFinished {
                account: origin,
                generation,
                profile,
                result,
            } => {
                return self.sign_in_finished(origin.as_deref(), generation, &profile, result);
            }
            AccountMessage::Saved {
                account: id,
                generation: sign_in,
                result,
            } => {
                if let Some(a) = self.account_mut(&id)
                    && a.sign_in == sign_in
                {
                    a.notice = result.err();
                }
            }
            AccountMessage::RetrySave(id) => {
                let Some(a) = self.account(&id) else {
                    return Task::none();
                };
                let AccountState::Ready { signed_in, .. } = &a.state else {
                    return Task::none();
                };
                let (signed_in, sign_in) = (signed_in.clone(), a.sign_in);
                let started = self.sign_ins.sign_out_count(&id);
                let sign_ins = self.sign_ins.clone();
                return cosmic::task::future(async move {
                    let result = off_ui(
                        move || {
                            sign_ins
                                .save(&signed_in, started)
                                .map_err(|e| describe_not_saved(&e))
                        },
                        Err(fl!("error-generic")),
                    )
                    .await;
                    Message::Account(AccountMessage::Saved {
                        account: id,
                        generation: sign_in,
                        result,
                    })
                });
            }
            AccountMessage::Refresh(id) => return self.load_resources(&id),
            AccountMessage::RefreshDue => {
                // The check timer stops while the computer sleeps; the
                // wall clock doesn't (spec §7: refresh after resume).
                let now = (Instant::now(), SystemTime::now());
                let slept = self.last_check.replace(now).is_some_and(|(mono, wall)| {
                    let wall = now.1.duration_since(wall).unwrap_or_default();
                    decide::slept(now.0 - mono, wall)
                });
                if slept {
                    crate::app_log!("woke from sleep: refreshing desktop lists");
                    for account in &mut self.accounts {
                        account.last_load = None;
                    }
                }
                return self.refresh_due();
            }
            AccountMessage::Discovered {
                account: id,
                epoch,
                generation,
                result,
            } => {
                return self.discovered(&id, epoch, generation, result);
            }
            AccountMessage::IconsLoaded {
                account: id,
                epoch,
                icons,
            } => self.icons_loaded(&id, epoch, icons),
            AccountMessage::CacheLoaded {
                account: id,
                epoch,
                cached,
            } => self.cache_loaded(&id, epoch, cached),
            AccountMessage::SignOut(id) => return self.ask_leave(&id, Leaving::SignOut),
            AccountMessage::Remove(id) => return self.ask_leave(&id, Leaving::Remove),
            AccountMessage::SignOutSelected => {
                if let Some(Page::Account(id)) = self.selected_page() {
                    return self.ask_leave(&id, Leaving::SignOut);
                }
            }
            AccountMessage::SignOutConfirm => {
                if let Some((id, leaving)) = self.sign_out_asking.take() {
                    return self.sign_out(&id, leaving);
                }
            }
            AccountMessage::SignOutCancel => self.sign_out_asking = None,
            AccountMessage::SignOutFinished {
                account: id,
                identity,
                result,
            } => {
                if let Err(e) = result {
                    crate::app_log!("sign-out: keyring not cleaned ({e})");
                    self.cleanup_retry(&id).keyring = Some(*identity);
                }
            }
            AccountMessage::RetrySignOutCleanup => return self.retry_sign_out_cleanup(),
            AccountMessage::DismissSignOutCleanup => self.cleanup_retry = None,
            AccountMessage::SignOutFilesRemoved {
                account: id,
                result,
            } => {
                if result.is_err() {
                    crate::app_log!("sign-out: sign-in files not removed");
                    self.cleanup_retry(&id).files = true;
                }
                // A session still starting removes the files once more
                // when it has been stopped (`launched`).
                if !self
                    .desks
                    .all_launching()
                    .any(|(_, l)| l.start.account == id)
                {
                    self.signing_out.remove(&id);
                }
                if self.quit == Quit::Stopping {
                    return self.continue_quit();
                }
            }
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
            // Saved by a version that didn't list accounts yet.
            self.remember_account(&id, &stored.identity);
            let epoch = self.next();
            let mut account = Account::new(
                id.clone(),
                stored.identity.clone(),
                AccountState::Restoring,
                epoch,
            );
            account.stored = Some(stored);
            self.accounts.push(account);
            tasks.push(self.load_cached(&id, epoch));
            tasks.push(self.start_restore(&id));
        }
        // Accounts signed out earlier: listed, signed out.
        let signed_out: Vec<(String, crate::config::KnownAccount)> = self
            .config
            .accounts
            .iter()
            .filter(|(id, _)| self.account(id).is_none())
            .map(|(id, k)| (id.clone(), k.clone()))
            .collect();
        for (id, known) in signed_out {
            let epoch = self.next();
            self.accounts.push(Account::new(
                id,
                (&known).into(),
                AccountState::NeedsSignIn { error: None },
                epoch,
            ));
        }
        if let (Some(store), Some(known)) = (self.cache.clone(), known) {
            tasks.push(
                Task::future(async move {
                    off_ui(move || store.remove_others(&known), ()).await;
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
            let cached = off_ui(move || store.load(&owner, keep), None).await;
            Message::Account(AccountMessage::CacheLoaded {
                account: id,
                epoch,
                cached,
            })
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
        // Read now: a sign-out before the save runs stops it.
        let started = self.sign_ins.sign_out_count(&id);
        let sign_ins = self.sign_ins.clone();
        Task::future(async move {
            let result = off_ui(
                move || sign_ins.save_cache(&store, &id, &list, started),
                Ok(()),
            )
            .await;
            if let Err(e) = result {
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
        // Read now: a sign-out before the restore runs stops it.
        let started = self.sign_ins.sign_out_count(&id);
        let sign_ins = self.sign_ins.clone();
        cosmic::task::future(async move {
            let outcome = off_ui(
                move || {
                    sign_ins
                        .restore(&http, &stored, started)
                        .map(|(account, unsaved)| {
                            (account, unsaved.as_ref().map(describe_not_saved))
                        })
                        .map_err(describe_restore_error)
                },
                Err(StepError::Show(fl!("error-generic"))),
            )
            .await;
            Message::Account(AccountMessage::Restored {
                account: id,
                generation,
                outcome,
            })
        })
    }

    pub(super) fn restored(
        &mut self,
        id: &str,
        generation: u64,
        outcome: Result<(Arc<SignedIn>, Option<String>), StepError>,
    ) -> Task<cosmic::Action<Message>> {
        let Some(account) = self.account_mut(id).filter(|a| a.generation == generation) else {
            return Task::none();
        };
        match outcome {
            Ok((signed_in, notice)) => {
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
            Err(StepError::SignInAgain(error)) => {
                account.stored = None;
                account.state = AccountState::NeedsSignIn { error: Some(error) };
                Task::none()
            }
            Err(StepError::Show(error)) => {
                account.can_retry = true;
                account.state = AccountState::NeedsSignIn { error: Some(error) };
                Task::none()
            }
        }
    }

    /// Opens the sign-in window for an account not in the list yet, with a
    /// fresh cookie profile.
    pub(super) fn start_add(&mut self) -> Task<cosmic::Action<Message>> {
        if matches!(self.adding, Some(Adding::SigningIn { .. })) {
            return Task::none();
        }
        self.rebuild_nav(&Page::Add);
        let generation = self.next();
        // The helper first: a profile made for a sign-in that can't start
        // would be left behind.
        let Some(helper) = signin::find_helper() else {
            self.adding = Some(Adding::Failed(fl!("error-helper-missing")));
            return Task::none();
        };
        let Some(profile) = accounts::new_profile_dir() else {
            self.adding = Some(Adding::Failed(fl!("error-helper-missing")));
            return Task::none();
        };
        let cancel = Cancel::default();
        self.adding = Some(Adding::SigningIn {
            generation,
            cancel: cancel.clone(),
        });
        self.run_sign_in(None, generation, helper, profile, None, cancel)
    }

    /// Signs in again to an account in the list, with its own cookies and
    /// its user name preselected.
    pub(super) fn start_account_sign_in(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        // Its sign-out is still deleting the cookie profile a new sign-in
        // would use.
        if self.signing_out.contains(id) {
            if let Some(a) = self.account_mut(id) {
                a.state = AccountState::NeedsSignIn {
                    error: Some(fl!("error-still-signing-out")),
                };
            }
            return Task::none();
        }
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
            let result = off_ui(
                move || {
                    signin::sign_in(
                        &http,
                        &ClientConfig::public_remote_desktop(),
                        &helper,
                        &profile,
                        login_hint.as_deref(),
                        time,
                        &cancel,
                    )
                },
                Err(SignInError::HelperFailed(signin::HelperProblem::Task)),
            )
            .await;
            Message::Account(AccountMessage::SignInFinished {
                account: origin,
                generation,
                profile: used,
                result: result.map(Arc::new).map_err(describe_sign_in_error),
            })
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
            None => matches!(
                &self.adding,
                Some(Adding::SigningIn { generation: g, .. }) if *g == generation
            ),
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
                        // `None`: cancelled, nothing to show.
                        self.adding = error.map(Adding::Failed);
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
            self.refuse_while_signing_out(origin, profile);
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
        if let Some(a) = self.account_mut(&id) {
            a.identity = signed_in.identity.clone();
            a.state = state;
            a.notice = None;
            a.can_retry = false;
            a.generation = generation;
            a.sign_in = generation;
        } else {
            let mut account = Account::new(id.clone(), signed_in.identity.clone(), state, epoch);
            account.generation = generation;
            account.sign_in = generation;
            self.accounts.push(account);
        }
        self.remember_account(&id, &signed_in.identity);
        self.rebuild_nav(&Page::Account(id.clone()));
        let started = self.sign_ins.sign_out_count(&id);
        self.sign_ins.signed_in(&signed_in, started);
        let save_id = id.clone();
        let sign_ins = self.sign_ins.clone();
        let save = cosmic::task::future(async move {
            let result = off_ui(
                move || {
                    sign_ins
                        .save(&signed_in, started)
                        .map_err(|e| describe_not_saved(&e))
                },
                Err(fl!("error-generic")),
            )
            .await;
            Message::Account(AccountMessage::Saved {
                account: save_id,
                generation,
                result,
            })
        });
        Task::batch([save, self.load_resources(&id)])
    }

    /// A sign-in finished for an account whose sign-out is still deleting
    /// its files: adopting this profile now would lose it.
    /// Keeps the account in the sidebar across restarts (its name and IDs).
    pub(super) fn remember_account(
        &mut self,
        id: &str,
        identity: &crate::auth::id_token::Identity,
    ) {
        let known = crate::config::KnownAccount::from(identity);
        if self.config.accounts.get(id) == Some(&known) {
            return;
        }
        let mut all = self.config.accounts.clone();
        all.insert(id.to_owned(), known);
        save_config!(self, accounts, set_accounts, all);
    }

    /// Takes a removed account off the remembered list, with its own
    /// settings.
    fn forget_account(&mut self, id: &str) {
        let mut all = self.config.accounts.clone();
        if all.remove(id).is_some() {
            save_config!(self, accounts, set_accounts, all);
        }
        let mut overrides = self.config.account_settings.clone();
        if overrides.remove(id).is_some() {
            save_config!(self, account_settings, set_account_settings, overrides);
        }
    }

    fn refuse_while_signing_out(&mut self, origin: Option<&str>, profile: &Path) {
        accounts::remove_unused_profile(profile);
        let error = fl!("error-still-signing-out");
        match origin {
            None => self.adding = Some(Adding::Failed(error)),
            Some(other) => {
                if let Some(a) = self.account_mut(other) {
                    a.state = AccountState::NeedsSignIn { error: Some(error) };
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
        account.jitter = getrandom::u32().unwrap_or(u32::MAX / 2);
        let signed_in = signed_in.clone();
        let epoch = account.epoch;
        let http = self.http.clone();
        let id = id.to_owned();
        // Read now: a sign-out before this runs stops it.
        let started = self.sign_ins.sign_out_count(&id);
        let owner = id.clone();
        let sign_ins = self.sign_ins.clone();
        cosmic::task::future(async move {
            let result = off_ui(
                move || {
                    // No more requests once the account signed out.
                    let wanted = || sign_ins.sign_out_count(&owner) == started;
                    // An expired access token would only get "session expired".
                    let (feeds, signed_in) = with_fresh_token(
                        &sign_ins,
                        &http,
                        signed_in,
                        started,
                        "desktop list",
                        |token| discovery::discover(&http, token, &wanted),
                    )?;
                    let list = feeds
                        .into_iter()
                        .map(|f| workspace_view(&signed_in, f))
                        .collect();
                    Ok((list, signed_in))
                },
                Err(StepError::Show(fl!("error-generic"))),
            )
            .await;
            Message::Account(AccountMessage::Discovered {
                account: id,
                epoch,
                generation,
                result,
            })
        })
    }

    /// Refreshes the desktop lists not refreshed for their account's
    /// refresh interval.
    pub(super) fn refresh_due(&mut self) -> Task<cosmic::Action<Message>> {
        let due: Vec<String> = self
            .accounts
            .iter()
            .filter(|a| {
                let every = decide::refresh_wait(
                    Duration::from_secs(
                        u64::from(self.account_settings(Some(&a.id)).refresh_mins) * 60,
                    ),
                    a.jitter,
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
                    let save = list.clone();
                    *workspaces = list;
                    *error = None;
                    account.last_refreshed = Some(SystemTime::now());
                    account.cached = None;
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
        let started = self.sign_ins.sign_out_count(&id);
        let owner = id.clone();
        let sign_ins = self.sign_ins.clone();
        cosmic::task::future(async move {
            let icons = off_ui(
                move || {
                    let mut icons = Vec::new();
                    let mut refused = 0;
                    for url in missing {
                        // No more requests once the account signed out.
                        if sign_ins.sign_out_count(&owner) != started {
                            break;
                        }
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
                },
                Vec::new(),
            )
            .await;
            Message::Account(AccountMessage::IconsLoaded {
                account: id,
                epoch,
                icons,
            })
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
            let sign_ins = self.sign_ins.clone();
            tasks.push(cosmic::task::future(async move {
                let owner = id.clone();
                let result = off_ui(move || sign_ins.remove_files(&owner), Err(())).await;
                Message::Account(AccountMessage::SignOutFilesRemoved {
                    account: id,
                    result,
                })
            }));
        }
        if let Some(identity) = retry.keyring {
            tasks.push(keyring_cleanup(self.sign_ins.clone(), retry.id, identity));
        }
        Task::batch(tasks)
    }

    /// Signing out or removing asks first while the account has desktops
    /// or apps open (spec §6: disconnect and sign out, or cancel).
    fn ask_leave(&mut self, id: &str, leaving: Leaving) -> Task<cosmic::Action<Message>> {
        if self.desks.open_count_for(id) == 0 {
            return self.sign_out(id, leaving);
        }
        self.sign_out_asking = Some((id.to_owned(), leaving));
        Task::none()
    }

    pub(super) fn sign_out_dialog(&self, id: &str, leaving: Leaving) -> Element<'_, Message> {
        let label = self.account(id).map(Account::label).unwrap_or_default();
        let open = u32::try_from(self.desks.open_count_for(id)).unwrap_or(u32::MAX);
        let (title, body, confirm) = match leaving {
            Leaving::SignOut => (
                fl!("sign-out-title", account = label),
                fl!("sign-out-body", count = open),
                fl!("sign-out-disconnect"),
            ),
            Leaving::Remove => (
                fl!("remove-title", account = label),
                fl!("remove-body", count = open),
                fl!("remove-disconnect"),
            ),
        };
        widget::dialog()
            .title(title)
            .body(body)
            .primary_action(
                widget::button::destructive(confirm)
                    .on_press(Message::Account(AccountMessage::SignOutConfirm)),
            )
            .secondary_action(
                widget::button::standard(fl!("cancel"))
                    .on_press(Message::Account(AccountMessage::SignOutCancel)),
            )
            .into()
    }

    /// Signs one account out: ends its sessions and deletes its cookies,
    /// keyring items and offline list (spec §6). Signed out, it stays
    /// listed as a fresh entry, so nothing started before can bring it back
    /// (each result checks its account's epoch or generation); removed, it
    /// leaves the list and its own settings go.
    pub(super) fn sign_out(&mut self, id: &str, leaving: Leaving) -> Task<cosmic::Action<Message>> {
        let Some(pos) = self.accounts.iter().position(|a| a.id == id) else {
            return Task::none();
        };
        let account = match leaving {
            Leaving::Remove => {
                self.forget_account(id);
                self.accounts.remove(pos)
            }
            Leaving::SignOut => {
                let epoch = self.next();
                let fresh = Account::new(
                    id.to_owned(),
                    self.accounts[pos].identity.clone(),
                    AccountState::NeedsSignIn { error: None },
                    epoch,
                );
                std::mem::replace(&mut self.accounts[pos], fresh)
            }
        };
        self.icons.retain(|(account, _), _| account != id);
        // From now on nothing started earlier saves its tokens.
        self.sign_ins.note_sign_out(id);
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
            self.desks
                .all_running()
                .filter(|(_, r)| r.start.account == id)
                .map(|(k, _)| k.clone()),
        );
        keys.extend(
            self.desks
                .all_launching()
                .filter(|(_, l)| l.start.account == id)
                .map(|(k, _)| k.clone()),
        );
        let mut sessions = Vec::new();
        for key in keys {
            // Everything about the desk goes; one still starting is stopped
            // as soon as it has started (`launched`).
            sessions.extend(self.desks.at(&key).sign_out());
            self.set_saved_password(&key, false);
        }
        self.withdraw_dialogs(|d| d.account == id);
        self.signing_out.insert(id.to_owned());
        match leaving {
            Leaving::Remove => self.select_first_account(),
            Leaving::SignOut => self.rebuild_nav(&Page::Account(id.to_owned())),
        }
        // Its cookies go only after everything that could still write them
        // has ended: the sign-in window and FreeRDP with its sign-in helper
        // (spec §6). Blocking, so not on the UI thread.
        let files = {
            let id = id.to_owned();
            let sign_ins = self.sign_ins.clone();
            cosmic::task::future(async move {
                let owner = id.clone();
                let result = off_ui(
                    move || {
                        if let Some(cancel) = sign_in {
                            cancel.cancel_and_wait(launcher::STOP_GRACE);
                        }
                        launcher::stop_all(&sessions, launcher::STOP_GRACE);
                        sign_ins.remove_files(&owner)
                    },
                    Err(()),
                )
                .await;
                Message::Account(AccountMessage::SignOutFilesRemoved {
                    account: id,
                    result,
                })
            })
        };
        let keyring = keyring_cleanup(self.sign_ins.clone(), id.to_owned(), account.identity);
        Task::batch([files, keyring])
    }
}

/// Deletes an account's keyring entries: its saved sign-in and its
/// remembered Windows passwords (spec §8). The keyring may block (for
/// example on an unlock prompt), so on its own.
fn keyring_cleanup(
    sign_ins: Arc<accounts::Accounts>,
    id: String,
    identity: crate::auth::id_token::Identity,
) -> Task<cosmic::Action<Message>> {
    cosmic::task::future(async move {
        let kept = identity.clone();
        // The saved sign-in and the remembered passwords, under the
        // account's lock like their saves.
        let result = off_ui(
            move || {
                sign_ins
                    .delete(&identity)
                    .map_err(|_| fl!("error-sign-out-incomplete"))
            },
            Err(fl!("error-sign-out-incomplete")),
        )
        .await;
        Message::Account(AccountMessage::SignOutFinished {
            account: id,
            identity: Box::new(kept),
            result,
        })
    })
}

/// Runs `request` with the account's access token, renewed first if it
/// expires soon. If the service refuses it anyway (401: revoked, or the
/// clocks disagree), the token is renewed once more and the request tried
/// again; refused a second time, the account must sign in again. `what`
/// names the request in the log. Blocking.
pub(super) fn with_fresh_token<T>(
    sign_ins: &accounts::Accounts,
    http: &Http,
    signed_in: Arc<SignedIn>,
    started: u64,
    what: &str,
    request: impl Fn(&crate::service::http::BearerToken) -> Result<T, FetchError>,
) -> Result<(T, Arc<SignedIn>), StepError> {
    let signed_in = sign_ins.renew(http, signed_in, accounts::Renewal::IfNeeded, started)?;
    match request(&signed_in.tokens.access_token) {
        Err(FetchError::Http(HttpError::Status { status: 401, .. })) => {
            crate::app_log!("{what}: access token refused; renewing");
            let renewed = sign_ins.renew(http, signed_in, accounts::Renewal::Now, started)?;
            let value = request(&renewed.tokens.access_token).map_err(refused_or_shown)?;
            Ok((value, renewed))
        }
        result => {
            let value = result.map_err(|e| StepError::Show(describe_fetch_error(&e)))?;
            Ok((value, signed_in))
        }
    }
}
