// SPDX-License-Identifier: MIT

//! Background operation (spec §5, ADR 0017): with the setting on and a
//! tray available, closing the window keeps Deskfeed and its desktops
//! running; the tray icon opens it again.

use super::*;
use cosmic::iced::window;
use desk::Desks;

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
    <AppModel as cosmic::Application>::APP_ID.clone_into(&mut s.platform_specific.application_id);
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
                self.desks.at(&key).from_tray = true;
                self.connection_message(ConnectionMessage::Connect(key))
            }
            TrayEvent::ShowDesktop(key) => {
                self.connection_message(ConnectionMessage::ShowSession(key))
            }
            TrayEvent::Quit => {
                // Asked in the window when desktops are open.
                let show = if self.desks.open_count() == 0 {
                    Task::none()
                } else {
                    self.show_window()
                };
                Task::batch([show, self.quit_message(QuitMessage::Chosen)])
            }
        }
    }

    /// With the window closed: opens it when something needs the user (the
    /// quit question, a password, a certificate, a desktop connected from
    /// the tray that failed).
    pub(super) fn window_needed(&mut self) -> Task<cosmic::Action<Message>> {
        if self.core.main_window_id().is_some() {
            self.desks.retain_from_tray(|_, _| false);
            return Task::none();
        }
        let failed = self
            .desks
            .tray_keys()
            .any(|k| matches!(self.desks.conn(k), Some(Connection::Failed(_))));
        let needs_sign_in = self.desks.tray_keys().any(|k| {
            self.owner_of(k).is_none()
                && self
                    .accounts
                    .iter()
                    .any(|a| matches!(a.state, AccountState::NeedsSignIn { .. }))
        });
        // Settled ones don't need watching any more.
        self.desks
            .retain_from_tray(|_, d| d.connection.as_ref().is_some_and(Connection::is_starting));
        if self.quit == Quit::Asking
            || self.dialogs.shown().is_some()
            || self.certificate_dialog.is_some()
            || failed
            || needs_sign_in
        {
            self.desks.retain_from_tray(|_, _| false);
            return self.show_window();
        }
        Task::none()
    }

    /// The tray's menu, from the signed-in accounts' lists (`tray_menu`).
    fn tray_items(&self) -> Vec<crate::tray::TrayItem> {
        let ready: Vec<(String, Vec<&ResourceView>)> = self
            .accounts
            .iter()
            .filter_map(|account| {
                let AccountState::Ready { workspaces, .. } = &account.state else {
                    return None;
                };
                let resources = workspaces.iter().flat_map(|w| &w.resources).collect();
                Some((account.label(), resources))
            })
            .collect();
        tray_menu(&ready, &self.config.favorites, &self.desks)
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
            .desks
            .all_running()
            .map(|(_, r)| r)
            .filter(|r| r.start.keep_awake)
            .map(|r| r.start.title.clone())
            .collect();
        if titles.is_empty() {
            return Subscription::none();
        }
        titles.sort();
        Subscription::run_with(titles, crate::awake::watch)
            .map(|event| Message::App(AppMessage::Awake(event)))
    }
}

/// The tray's menu: how many desktops are connected, Show, every desktop
/// of the signed-in accounts (`(label, resources)`; a submenu per account
/// when there are several), Quit.
fn tray_menu(
    accounts: &[(String, Vec<&ResourceView>)],
    favorites: &std::collections::BTreeSet<String>,
    desks: &Desks,
) -> Vec<crate::tray::TrayItem> {
    use crate::tray::{TrayAction, TrayItem};
    let action = |label: String, action: TrayAction| TrayItem::Action {
        label,
        action,
        checked: false,
        enabled: true,
    };
    let mut items = Vec::new();
    let connected = desks.connected_count();
    if connected > 0 {
        let count = u32::try_from(connected).unwrap_or(u32::MAX);
        items.push(TrayItem::Note(fl!("tray-connected", count = count)));
    }
    items.push(action(fl!("tray-show"), TrayAction::Show));
    let several = accounts.len() > 1;
    let mut groups = Vec::new();
    for (label, resources) in accounts {
        let mut desktops: Vec<&ResourceView> = resources
            .iter()
            .copied()
            .filter(|r| r.kind != ResourceKind::Other)
            .collect();
        // Favorites first, then by title.
        desktops.sort_by_key(|r| (!favorites.contains(&r.key), r.title.clone()));
        desktops.dedup_by(|a, b| a.key == b.key);
        let lines: Vec<TrayItem> = desktops
            .into_iter()
            .map(|r| tray_desktop(r, desks))
            .collect();
        if lines.is_empty() {
            continue;
        }
        if several {
            groups.push(TrayItem::Submenu {
                label: label.clone(),
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
fn tray_desktop(r: &ResourceView, desks: &Desks) -> crate::tray::TrayItem {
    use crate::tray::{TrayAction, TrayItem};
    let busy = |status: String| TrayItem::Action {
        label: format!("{} · {status}", r.title),
        action: TrayAction::ShowDesktop(r.key.clone()),
        checked: false,
        enabled: false,
    };
    match desks.conn(&r.key) {
        Some(Connection::Connected) => TrayItem::Action {
            label: r.title.clone(),
            // An app's windows can't be told apart (ADR 0020): open
            // Deskfeed instead.
            action: if r.kind == ResourceKind::Desktop {
                TrayAction::ShowDesktop(r.key.clone())
            } else {
                TrayAction::Show
            },
            checked: true,
            enabled: true,
        },
        None | Some(Connection::Failed(_)) => TrayItem::Action {
            label: fl!("tray-connect", title = r.title.clone()),
            action: TrayAction::Connect(r.key.clone()),
            checked: false,
            enabled: true,
        },
        Some(under_way) => busy(under_way.progress_label().unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::{TrayAction, TrayItem};

    fn resource(key: &str, title: &str, kind: ResourceKind) -> ResourceView {
        ResourceView {
            key: key.into(),
            title: title.into(),
            kind,
            connection_file: None,
            icon: None,
        }
    }

    /// The desktop lines: the part between the first and the last
    /// separator (Show and the count above, Quit below), as (label, action,
    /// checked, enabled).
    fn lines(items: &[TrayItem]) -> Vec<(String, TrayAction, bool, bool)> {
        let seps: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i, TrayItem::Separator))
            .map(|(n, _)| n)
            .collect();
        let (Some(&first), Some(&last)) = (seps.first(), seps.last()) else {
            return Vec::new();
        };
        items[first..last]
            .iter()
            .filter_map(|i| match i {
                TrayItem::Action {
                    label,
                    action,
                    checked,
                    enabled,
                } => Some((label.clone(), action.clone(), *checked, *enabled)),
                _ => None,
            })
            .collect()
    }

    // Expectations from ADR 0017 ("The app builds the menu") and ADR 0020
    // (an app's windows can't be told apart), not from the code's output.

    #[test]
    fn nothing_connected_shows_no_count_and_offers_connect() {
        let a = resource("a", "Alpha", ResourceKind::Desktop);
        let b = resource("b", "Beta", ResourceKind::Desktop);
        let other = resource("x", "Unknown kind", ResourceKind::Other);
        let accounts = vec![("anna@example.com".to_owned(), vec![&b, &a, &other, &a])];
        let favorites = std::collections::BTreeSet::from(["b".to_owned()]);
        let items = tray_menu(&accounts, &favorites, &Desks::default());
        // No count line; Show first, Quit last.
        assert!(!items.iter().any(|i| matches!(i, TrayItem::Note(_))));
        assert!(matches!(
            &items[0],
            TrayItem::Action {
                action: TrayAction::Show,
                ..
            }
        ));
        assert!(matches!(
            items.last(),
            Some(TrayItem::Action {
                action: TrayAction::Quit,
                ..
            })
        ));
        // One account: its desktops inline, favorite first, each once,
        // unknown kinds left out, all offering Connect.
        assert!(!items.iter().any(|i| matches!(i, TrayItem::Submenu { .. })));
        let got = lines(&items);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].1, TrayAction::Connect("b".into()));
        assert_eq!(got[1].1, TrayAction::Connect("a".into()));
        assert!(got[0].0.contains("Beta") && got[0].0.starts_with("Connect"));
        assert!(
            got.iter()
                .all(|(_, _, checked, enabled)| !checked && *enabled)
        );
    }

    #[test]
    fn connected_busy_and_failed_desktops() {
        let desk = resource("d", "Desk", ResourceKind::Desktop);
        let app = resource("p", "Paint", ResourceKind::RemoteApp);
        let busy = resource("r", "Retry", ResourceKind::Desktop);
        let failed = resource("f", "Failed", ResourceKind::Desktop);
        let mut desks = Desks::default();
        desks.set_conn("d", Connection::Connected);
        desks.set_conn("p", Connection::Connected);
        desks.set_conn("r", Connection::Reconnecting { attempt: 2, of: 5 });
        desks.set_conn("f", Connection::Failed("x".into()));
        let accounts = vec![(
            "anna@example.com".to_owned(),
            vec![&desk, &app, &busy, &failed],
        )];
        let items = tray_menu(&accounts, &std::collections::BTreeSet::new(), &desks);
        // "2 connected": a reconnecting desktop isn't connected.
        let Some(TrayItem::Note(count)) = items.first() else {
            panic!("{items:?}")
        };
        assert!(count.contains('2'), "{count}");
        let got = lines(&items);
        let find = |title: &str| got.iter().find(|l| l.0.contains(title)).unwrap().clone();
        // Connected: checked, brings the desktop to the front.
        assert_eq!(find("Desk").1, TrayAction::ShowDesktop("d".into()));
        assert!(find("Desk").2 && find("Desk").3);
        // A connected app opens Deskfeed instead (ADR 0020).
        assert_eq!(find("Paint").1, TrayAction::Show);
        assert!(find("Paint").2);
        // Busy: its state, disabled, not checked.
        let r = find("Retry");
        assert!(!r.2 && !r.3, "{r:?}");
        assert!(r.0.contains('2') && r.0.contains('5'), "{}", r.0);
        // Failed: Connect again.
        assert_eq!(find("Failed").1, TrayAction::Connect("f".into()));
        assert!(find("Failed").3);
    }

    #[test]
    fn several_accounts_get_a_submenu_each() {
        let a = resource("a", "Alpha", ResourceKind::Desktop);
        let b = resource("b", "Beta", ResourceKind::Desktop);
        let accounts = vec![
            ("anna@example.com".to_owned(), vec![&a]),
            ("bob@example.com".to_owned(), vec![&b]),
            // An account without desktops gets no empty submenu.
            ("carl@example.com".to_owned(), vec![]),
        ];
        let items = tray_menu(
            &accounts,
            &std::collections::BTreeSet::new(),
            &Desks::default(),
        );
        let subs: Vec<(&str, usize)> = items
            .iter()
            .filter_map(|i| match i {
                TrayItem::Submenu { label, items } => Some((label.as_str(), items.len())),
                _ => None,
            })
            .collect();
        assert_eq!(subs, [("anna@example.com", 1), ("bob@example.com", 1)]);
        // No desktop lines outside the submenus.
        assert_eq!(lines(&items), []);
    }
}
