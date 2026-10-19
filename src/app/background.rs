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
                self.update_tray()
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
            TrayEvent::ShowDesktop(key) => {
                cosmic::Application::update(self, Message::ShowSession(key))
            }
            TrayEvent::Quit => {
                // Asked in the window when desktops are open.
                self.quit_from_tray = true;
                let show = if self.running.is_empty() && self.launching.is_empty() {
                    Task::none()
                } else {
                    self.show_window()
                };
                Task::batch([
                    show,
                    cosmic::Application::update(self, Message::QuitRequested),
                ])
            }
        }
    }

    /// Lists the connected desktops in the tray's menu.
    pub(super) fn update_tray(&self) -> Task<cosmic::Action<Message>> {
        let Some(handle) = self.tray.clone() else {
            return Task::none();
        };
        let desktops: Vec<(String, String)> = self
            .connections
            .iter()
            .filter(|(_, c)| matches!(c, Connection::Connected))
            .filter_map(|(key, _)| {
                let title = self.account_of_resource(key)?.resource(key)?.title.clone();
                Some((key.clone(), title))
            })
            .collect();
        Task::future(async move { handle.set_desktops(desktops).await }).discard()
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
