// SPDX-License-Identifier: MIT

//! Background operation (spec §5, ADR 0017): with the setting on and a
//! tray available, closing the window keeps Deskfeed and its desktops
//! running; the tray icon opens it again.

use super::*;
use cosmic::iced::window;

/// The main window as libcosmic opens it (`app::Settings` defaults with
/// Deskfeed's size limits, `main.rs`).
fn main_window_settings() -> window::Settings {
    let mut s = window::Settings {
        size: cosmic::iced::Size::new(1024.0, 768.0),
        min_size: Some(cosmic::iced::Size::new(360.0, 180.0)),
        decorations: false,
        transparent: true,
        resizable: true,
        resize_border: 8,
        exit_on_close_request: false,
        ..window::Settings::default()
    };
    s.platform_specific.application_id = <AppModel as cosmic::Application>::APP_ID.to_owned();
    s
}

impl AppModel {
    /// Closes the window; the app keeps running.
    pub(super) fn hide_window(&mut self) -> Task<cosmic::Action<Message>> {
        match self.core.set_main_window_id(None) {
            Some(id) => {
                crate::app_log!("window closed; running in the background");
                window::close(id)
            }
            None => Task::none(),
        }
    }

    /// Brings the window back: opens it again, or focuses it if it's open.
    pub(super) fn show_window(&mut self) -> Task<cosmic::Action<Message>> {
        if let Some(id) = self.core.main_window_id() {
            return window::gain_focus(id);
        }
        let (id, open) = window::open(main_window_settings());
        self.core.set_main_window_id(Some(id));
        crate::app_log!("window opened from the background");
        let title = self.update_title();
        Task::batch([open.discard(), title])
    }

    /// Background operation is on and a tray answers (spec §5: never
    /// without a way back).
    pub(super) fn can_run_in_background(&self) -> bool {
        self.config.background && self.tray.is_some()
    }

    /// What the tray asks for.
    pub(super) fn tray_event(
        &mut self,
        event: crate::tray::TrayEvent,
    ) -> Task<cosmic::Action<Message>> {
        use crate::tray::TrayEvent;
        match event {
            TrayEvent::Ready(handle) => {
                self.tray = Some(handle);
                self.tray_unavailable = None;
                // Sent by `update` right after this.
                self.tray_menu = None;
                Task::none()
            }
            TrayEvent::Unavailable(why) => {
                self.tray = None;
                self.tray_unavailable = Some(why);
                // Never stay hidden without a tray.
                self.show_window()
            }
            TrayEvent::Show(token) => {
                let show = self.show_window();
                match token {
                    Some(t) => Task::batch([
                        show,
                        Task::done(cosmic::Action::Cosmic(cosmic::app::Action::Activate(t))),
                    ]),
                    None => show,
                }
            }
            TrayEvent::Connect(key) => {
                // A password, a certificate or a failure opens the window
                // (`window_needed`).
                self.tray_connects.insert(key.clone());
                self.route(Message::Connect(key))
            }
            TrayEvent::ShowDesktop(key) => self.route(Message::ShowSession(key)),
            TrayEvent::Quit => {
                // Asked in the window when desktops are open.
                self.quit_from_tray = true;
                let show = if self.running.is_empty() && self.launching.is_empty() {
                    Task::none()
                } else {
                    self.show_window()
                };
                Task::batch([show, self.route(Message::QuitRequested)])
            }
        }
    }

    /// With the window closed: opens it when something needs the user (the
    /// quit question, a password, a certificate, a desktop connected from
    /// the tray that failed).
    pub(super) fn window_needed(&mut self) -> Task<cosmic::Action<Message>> {
        if self.core.main_window_id().is_some() {
            self.tray_connects.clear();
            return Task::none();
        }
        let failed = self
            .tray_connects
            .iter()
            .any(|k| matches!(self.connections.get(k), Some(Connection::Failed(_))));
        let needs_sign_in = self.tray_connects.iter().any(|k| {
            self.account_of_resource(k).is_none()
                && self
                    .accounts
                    .iter()
                    .any(|a| matches!(a.state, AccountState::NeedsSignIn { .. }))
        });
        // Settled ones don't need watching any more.
        self.tray_connects.retain(|k| {
            matches!(
                self.connections.get(k),
                Some(
                    Connection::Preparing
                        | Connection::AwaitingUser
                        | Connection::Connecting
                        | Connection::Authenticating
                )
            )
        });
        if self.quit == Quit::Asking
            || self.dialog.is_some()
            || self.certificate_dialog.is_some()
            || failed
            || needs_sign_in
        {
            self.tray_connects.clear();
            return self.show_window();
        }
        Task::none()
    }

    /// The tray's menu: how many desktops are connected, Show, every
    /// desktop (per account when there are several), Quit.
    fn tray_items(&self) -> Vec<crate::tray::TrayItem> {
        use crate::tray::{TrayAction, TrayItem};
        let action = |label: String, action: TrayAction| TrayItem::Action {
            label,
            action,
            checked: false,
            enabled: true,
        };
        let mut items = Vec::new();
        let connected = self
            .connections
            .values()
            .filter(|c| matches!(c, Connection::Connected))
            .count();
        if connected > 0 {
            let count = u32::try_from(connected).unwrap_or(u32::MAX);
            items.push(TrayItem::Note(fl!("tray-connected", count = count)));
        }
        items.push(action(fl!("tray-show"), TrayAction::Show));
        let ready: Vec<&Account> = self
            .accounts
            .iter()
            .filter(|a| matches!(a.state, AccountState::Ready { .. }))
            .collect();
        let several = ready.len() > 1;
        let mut groups = Vec::new();
        for account in ready {
            let AccountState::Ready { workspaces, .. } = &account.state else {
                continue;
            };
            let mut desktops: Vec<&ResourceView> = workspaces
                .iter()
                .flat_map(|w| &w.resources)
                .filter(|r| r.kind == ResourceKind::Desktop)
                .collect();
            // Favorites first, then by title.
            desktops.sort_by_key(|r| (!self.config.favorites.contains(&r.key), r.title.clone()));
            desktops.dedup_by(|a, b| a.key == b.key);
            let lines: Vec<TrayItem> = desktops.into_iter().map(|r| self.tray_desktop(r)).collect();
            if lines.is_empty() {
                continue;
            }
            if several {
                groups.push(TrayItem::Submenu {
                    label: account.label(),
                    items: lines,
                });
            } else {
                groups.extend(lines);
            }
        }
        if !groups.is_empty() {
            items.push(TrayItem::Separator);
            items.extend(groups);
        }
        items.push(TrayItem::Separator);
        items.push(action(fl!("tray-quit"), TrayAction::Quit));
        items
    }

    /// One desktop in the tray: Show when connected (checked), Connect when
    /// not, its state while busy.
    fn tray_desktop(&self, r: &ResourceView) -> crate::tray::TrayItem {
        use crate::tray::{TrayAction, TrayItem};
        let busy = |status: String| TrayItem::Action {
            label: format!("{} · {status}", r.title),
            action: TrayAction::ShowDesktop(r.key.clone()),
            checked: false,
            enabled: false,
        };
        match self.connections.get(&r.key) {
            Some(Connection::Connected) => TrayItem::Action {
                label: r.title.clone(),
                action: TrayAction::ShowDesktop(r.key.clone()),
                checked: true,
                enabled: true,
            },
            None | Some(Connection::Failed(_)) => TrayItem::Action {
                label: fl!("tray-connect", title = r.title.clone()),
                action: TrayAction::Connect(r.key.clone()),
                checked: false,
                enabled: true,
            },
            Some(Connection::Preparing | Connection::Connecting) => busy(fl!("status-connecting")),
            Some(Connection::AwaitingUser) => busy(fl!("status-awaiting-user")),
            Some(Connection::Authenticating) => busy(fl!("status-authenticating")),
            Some(Connection::Disconnecting) => busy(fl!("status-disconnecting")),
            Some(Connection::Reconnecting { attempt, of }) => {
                let (attempt, of) = (*attempt, *of);
                busy(fl!("status-reconnecting", attempt = attempt, of = of))
            }
        }
    }

    /// Sends the tray its menu when it changed.
    pub(super) fn update_tray(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(handle) = self.tray.clone() else {
            self.tray_menu = None;
            return Task::none();
        };
        let items = self.tray_items();
        if self.tray_menu.as_ref() == Some(&items) {
            return Task::none();
        }
        self.tray_menu = Some(items.clone());
        Task::future(async move { handle.set_menu(items).await }).discard()
    }

    /// Watches the sessions started with "Keep the screen on in full
    /// screen" (by window title); none, no watcher.
    pub(super) fn awake_subscription(&self) -> Subscription<Message> {
        let mut titles: Vec<String> = self
            .running
            .values()
            .filter(|r| r.keep_awake)
            .map(|r| r.title.clone())
            .collect();
        if titles.is_empty() {
            return Subscription::none();
        }
        titles.sort();
        Subscription::run_with(titles, crate::awake::watch).map(Message::Awake)
    }
}
