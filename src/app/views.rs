// SPDX-License-Identifier: MIT

//! Page layouts: welcome / add account, account pages, desktop lists, dependencies.

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;

impl AppModel {
    pub(super) fn page_view(&self) -> Element<'_, Message> {
        if self.loading_saved {
            return centered(widget::text::body(fl!("loading-accounts")).into());
        }
        match self.selected_page() {
            Some(Page::Account(id)) => match self.account(&id) {
                Some(account) => self.account_view(account),
                None => self.add_view(),
            },
            _ => self.add_view(),
        }
    }

    pub(super) fn add_view(&self) -> Element<'_, Message> {
        let space = cosmic::theme::spacing();
        if let Some(Adding {
            cancel: Some(_), ..
        }) = &self.adding
        {
            return waiting_view(Message::CancelSignIn(None));
        }
        let mut col = widget::column::with_capacity(5)
            .push(widget::text::title1(if self.accounts.is_empty() {
                fl!("app-title")
            } else {
                fl!("add-account")
            }))
            .push(widget::text::body(fl!("unofficial-notice")).width(Length::Fixed(420.0)))
            .push(widget::button::suggested(fl!("sign-in")).on_press(Message::AddAccount))
            .spacing(space.space_m)
            .align_x(Alignment::Center);
        if let Some(Adding { error: Some(e), .. }) = &self.adding {
            col = col.push(widget::text::body(e.clone()).width(Length::Fixed(420.0)));
        }
        centered(col.into())
    }

    pub(super) fn account_view<'a>(&'a self, account: &'a Account) -> Element<'a, Message> {
        let space = cosmic::theme::spacing();
        match &account.state {
            AccountState::Restoring => centered(
                widget::column::with_capacity(2)
                    .push(widget::text::title3(fl!("restoring")))
                    .push(widget::text::body(fl!("restoring-hint")))
                    .spacing(space.space_s)
                    .align_x(Alignment::Center)
                    .into(),
            ),
            AccountState::NeedsSignIn { error } => {
                let mut col = widget::column::with_capacity(5)
                    .push(widget::text::title3(account.label()))
                    .push(
                        widget::text::body(error.clone().unwrap_or_else(|| fl!("sign-in-needed")))
                            .width(Length::Fixed(420.0)),
                    )
                    .push(
                        widget::button::suggested(fl!("sign-in"))
                            .on_press(Message::SignInAccount(account.id.clone())),
                    )
                    .spacing(space.space_m)
                    .align_x(Alignment::Center);
                if account.can_retry {
                    col = col.push(
                        widget::button::standard(fl!("try-again"))
                            .on_press(Message::RetryRestore(account.id.clone())),
                    );
                }
                col = col.push(
                    widget::button::text(fl!("sign-out"))
                        .on_press(Message::SignOut(account.id.clone())),
                );
                centered(col.into())
            }
            AccountState::SigningIn { .. } => {
                waiting_view(Message::CancelSignIn(Some(account.id.clone())))
            }
            AccountState::Ready {
                loading,
                workspaces,
                error,
                ..
            } => self.resources_view(account, *loading, workspaces, error.as_deref()),
        }
    }
}

/// "Waiting for sign-in" with a Cancel button.
pub(super) fn waiting_view(cancel: Message) -> Element<'static, Message> {
    centered(
        widget::column::with_capacity(3)
            .push(widget::text::title3(fl!("signing-in")))
            .push(widget::text::body(fl!("signing-in-hint")))
            .push(widget::button::standard(fl!("cancel")).on_press(cancel))
            .spacing(cosmic::theme::spacing().space_s)
            .align_x(Alignment::Center)
            .into(),
    )
}

pub(super) fn centered(content: Element<'_, Message>) -> Element<'_, Message> {
    widget::container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center)
        .into()
}

/// The dependency list, as COSMIC Settings-style sections.
pub(super) fn dependencies_view(checks: Option<&[Check]>) -> Element<'_, Message> {
    let Some(checks) = checks else {
        return widget::text::body(fl!("deps-checking")).into();
    };
    let section = |required: bool| {
        let mut sec = widget::settings::section().title(if required {
            fl!("deps-required")
        } else {
            fl!("deps-optional")
        });
        for c in checks
            .iter()
            .filter(|c| c.dependency.required() == required)
        {
            let (name, hint) = dependency_text(c.dependency);
            let state = match &c.status {
                Status::Ok(v) if v.is_empty() => fl!("dep-ok"),
                Status::Ok(v) => fl!("dep-ok-version", version = v.clone()),
                Status::Missing => fl!("dep-missing"),
                Status::TooOld(v) => fl!(
                    "dep-too-old",
                    version = v.clone(),
                    min = format!(
                        "{}.{}.{}",
                        deps::MIN_FREERDP.0,
                        deps::MIN_FREERDP.1,
                        deps::MIN_FREERDP.2
                    )
                ),
                Status::Broken(why) => fl!("dep-broken", reason = why.clone()),
            };
            let description = if c.status.is_ok() {
                hint
            } else {
                format!("{state}. {hint}")
            };
            let icon = if c.status.is_ok() {
                "emblem-ok-symbolic"
            } else if required {
                "dialog-error-symbolic"
            } else {
                "dialog-warning-symbolic"
            };
            sec = sec.add(
                widget::settings::item::builder(name)
                    .description(description)
                    .control(
                        widget::row::with_capacity(2)
                            .push(widget::icon::from_name(icon).size(16))
                            .push(widget::text::body(if c.status.is_ok() {
                                state
                            } else {
                                String::new()
                            }))
                            .spacing(cosmic::theme::spacing().space_xxs)
                            .align_y(Alignment::Center),
                    ),
            );
        }
        sec
    };
    widget::settings::view_column(vec![section(true).into(), section(false).into()]).into()
}

/// Name and what to install, per dependency.
pub(super) fn dependency_text(d: Dependency) -> (String, String) {
    match d {
        Dependency::FreeRdp => (fl!("dep-freerdp"), fl!("dep-freerdp-hint")),
        Dependency::FreeRdpEntra => (fl!("dep-freerdp-entra"), fl!("dep-freerdp-entra-hint")),
        Dependency::SignInHelper => (fl!("dep-signin"), fl!("dep-signin-hint")),
        Dependency::SecretService => (fl!("dep-keyring"), fl!("dep-keyring-hint")),
        Dependency::RuntimeDir => (fl!("dep-runtime"), fl!("dep-runtime-hint")),
        Dependency::CosmicWindows => (fl!("dep-cosmic-windows"), fl!("dep-cosmic-windows-hint")),
    }
}

impl AppModel {
    /// One desktop: icon, title, kind and status, then favorite, settings
    /// and connect buttons.
    fn resource_row<'a>(&'a self, account: &str, r: &'a ResourceView) -> Element<'a, Message> {
        let conn = self.connections.get(&r.key);
        let kind = match r.kind {
            ResourceKind::Desktop => fl!("kind-desktop"),
            ResourceKind::RemoteApp => fl!("kind-remoteapp"),
            ResourceKind::Other => fl!("kind-other"),
        };
        let (status, control): (Option<String>, Element<'a, Message>) = match conn {
            None => {
                let mut b = widget::button::suggested(fl!("connect"));
                if r.kind == ResourceKind::Desktop {
                    b = b.on_press(Message::Connect(r.key.clone()));
                }
                (None, b.into())
            }
            Some(Connection::Preparing) => (
                Some(fl!("status-preparing")),
                widget::button::standard(fl!("connect")).into(),
            ),
            Some(Connection::Connecting) => (
                Some(fl!("status-connecting")),
                widget::button::standard(fl!("disconnect"))
                    .on_press(Message::Disconnect(r.key.clone()))
                    .into(),
            ),
            Some(Connection::Connected) => (
                Some(fl!("status-connected")),
                widget::button::destructive(fl!("disconnect"))
                    .on_press(Message::Disconnect(r.key.clone()))
                    .into(),
            ),
            Some(Connection::Failed(e)) => (
                Some(e.clone()),
                widget::button::suggested(fl!("connect"))
                    .on_press(Message::Connect(r.key.clone()))
                    .into(),
            ),
        };
        // The note only belongs to a session that is starting or running.
        let note = self
            .session_notes
            .get(&r.key)
            .filter(|_| matches!(conn, Some(Connection::Connecting | Connection::Connected)));
        let description = match (status, note) {
            (Some(s), Some(n)) => format!("{kind} · {s} · {n}"),
            (Some(s), None) => format!("{kind} · {s}"),
            _ => kind,
        };
        let favorite = self.config.favorites.contains(&r.key);
        let favorite = widget::button::icon(widget::icon::from_name("emblem-favorite-symbolic"))
            .selected(favorite)
            .tooltip(if favorite {
                fl!("favorite-remove")
            } else {
                fl!("favorite-add")
            })
            .on_press(Message::ToggleFavorite(r.key.clone()));
        let settings = widget::button::icon(widget::icon::from_name("preferences-system-symbolic"))
            .tooltip(fl!("desktop-settings"))
            .on_press(Message::OpenDesktopSettings(r.key.clone()));
        // The feed's icon, or a stock one until it has loaded (or without one).
        let icon = match r
            .icon
            .as_ref()
            .and_then(|u| self.icons.get(&(account.to_owned(), u.clone())))
        {
            Some(h) => widget::icon(h.clone()),
            None => widget::icon::from_name(match r.kind {
                ResourceKind::Desktop => "computer-symbolic",
                _ => "application-x-executable-symbolic",
            })
            .icon(),
        }
        .size(32);
        widget::settings::item::builder(r.title.clone())
            .icon(icon)
            .description(description)
            .control(
                widget::row::with_capacity(3)
                    .push(favorite)
                    .push(settings)
                    .push(control)
                    .spacing(cosmic::theme::spacing().space_xs)
                    .align_y(Alignment::Center),
            )
            .into()
    }

    /// An account's desktops: search, last update, favorites, then each
    /// workspace.
    fn resources_view<'a>(
        &'a self,
        account: &'a Account,
        loading: bool,
        workspaces: &'a [WorkspaceView],
        error: Option<&'a str>,
    ) -> Element<'a, Message> {
        let space = cosmic::theme::spacing();
        let mut col = widget::column::with_capacity(workspaces.len() + 6).spacing(space.space_m);
        let any = workspaces.iter().any(|w| !w.resources.is_empty());
        if any {
            col = col.push(
                widget::search_input(fl!("search-desktops"), &self.search)
                    .on_input(Message::SearchInput)
                    .on_clear(Message::SearchInput(String::new())),
            );
        }
        if let Some(notice) = &account.notice {
            col = col.push(widget::text::body(notice.clone()));
        }
        if loading {
            col = col.push(widget::text::body(fl!("loading-desktops")));
        }
        if let Some(error) = error {
            col = col.push(widget::text::body(error.to_owned()));
        }
        if let Some(t) = account.last_refreshed {
            let when = clock_label(t);
            col = col.push(widget::text::caption(if error.is_some() {
                fl!("showing-list-from", when = when)
            } else {
                fl!("updated-at", when = when)
            }));
        }
        if !loading
            && error.is_none()
            && workspaces
                .iter()
                .all(|w| w.resources.is_empty() && w.error.is_none())
        {
            col = col.push(widget::text::body(fl!("no-desktops")));
        }
        let query = self.search.trim().to_lowercase();
        let matches = |ws: &WorkspaceView, r: &ResourceView| search_matches(&query, ws, r);
        let favorites: Vec<&ResourceView> = workspaces
            .iter()
            .flat_map(|ws| ws.resources.iter().map(move |r| (ws, r)))
            .filter(|(ws, r)| self.config.favorites.contains(&r.key) && matches(ws, r))
            .map(|(_, r)| r)
            .collect();
        if !favorites.is_empty() {
            let mut section = widget::settings::section().title(fl!("favorites"));
            for r in favorites {
                section = section.add(self.resource_row(&account.id, r));
            }
            col = col.push(section);
        }
        let mut shown = 0;
        for ws in workspaces {
            let listed: Vec<&ResourceView> =
                ws.resources.iter().filter(|r| matches(ws, r)).collect();
            if listed.is_empty() && (ws.error.is_none() || !query.is_empty()) {
                continue;
            }
            shown += listed.len();
            let mut section = widget::settings::section().title(ws.name.clone());
            if let Some(e) = &ws.error {
                section = section.add(widget::text::body(fl!(
                    "workspace-failed",
                    detail = e.clone()
                )));
            }
            for r in listed {
                section = section.add(self.resource_row(&account.id, r));
            }
            col = col.push(section);
        }
        if any && !query.is_empty() && shown == 0 {
            col = col.push(widget::text::body(fl!(
                "no-search-results",
                query = self.search.trim()
            )));
        }
        widget::scrollable(
            widget::container(col.max_width(800))
                .width(Length::Fill)
                .align_x(Horizontal::Center)
                .padding(space.space_m),
        )
        .into()
    }
}

/// Whether a desktop matches the (lower-case, trimmed) search text: by its
/// title or its workspace's name, ignoring case. Empty matches all.
pub(super) fn search_matches(query: &str, ws: &WorkspaceView, r: &ResourceView) -> bool {
    query.is_empty()
        || r.title.to_lowercase().contains(query)
        || ws.name.to_lowercase().contains(query)
}

/// "14:05" for today, "2026-10-17 14:05" for an earlier day, in local
/// time (`localtime_r`; no date crate needed).
pub(super) fn clock_label(t: SystemTime) -> String {
    fn local(t: SystemTime) -> Option<libc::tm> {
        let secs =
            libc::time_t::try_from(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs()).ok()?;
        // SAFETY: `localtime_r` writes only into `tm`, which outlives the
        // call; a null result means failure.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let ok = unsafe { !libc::localtime_r(&raw const secs, &raw mut tm).is_null() };
        ok.then_some(tm)
    }
    let Some(tm) = local(t) else {
        return String::new();
    };
    let time = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    let today = local(SystemTime::now())
        .is_some_and(|n| (n.tm_year, n.tm_yday) == (tm.tm_year, tm.tm_yday));
    if today {
        time
    } else {
        format!(
            "{:04}-{:02}-{:02} {time}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop(title: &str) -> ResourceView {
        ResourceView {
            key: title.into(),
            legacy_key: String::new(),
            title: title.into(),
            kind: ResourceKind::Desktop,
            connection_file: None,
            icon: None,
        }
    }

    #[test]
    fn search_matches_title_or_workspace() {
        let ws = WorkspaceView {
            name: "Finance Pool".into(),
            error: None,
            resources: Vec::new(),
        };
        let r = desktop("Windows 11 Dev");
        assert!(search_matches("", &ws, &r));
        assert!(search_matches("dev", &ws, &r));
        assert!(search_matches("finance", &ws, &r));
        assert!(!search_matches("linux", &ws, &r));
        // Non-ASCII titles compare case-insensitively too.
        assert!(search_matches("æble", &ws, &desktop("ÆBLE desktop")));
    }

    #[test]
    fn clock_label_shows_time_today_and_date_before() {
        let now = clock_label(SystemTime::now());
        assert_eq!(now.len(), 5, "{now}");
        assert_eq!(now.as_bytes()[2], b':');
        let old = clock_label(SystemTime::UNIX_EPOCH + Duration::from_secs(400 * 86_400));
        // "1971-02-05 hh:mm" in any time zone.
        assert_eq!(old.len(), 16, "{old}");
        assert!(old.starts_with("1971-02-0"), "{old}");
    }
}
