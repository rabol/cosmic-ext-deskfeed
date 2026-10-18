// SPDX-License-Identifier: MIT

//! The app, account and desktop settings pages and saving them.
//!
//! Each level overrides the one above: built-in defaults → app → account →
//! desktop (`settings.rs`). The app page sets values; the account and
//! desktop pages either follow the level above or override it.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum Level {
    App,
    Account,
    Desktop,
}

/// Dropdown index of an override: follow the level above, on, off.
fn choice_value(choice: usize) -> Option<bool> {
    match choice {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

impl AppModel {
    /// Settings messages: opening and changing app, account and desktop
    /// settings. Routed here by `update`.
    pub(super) fn settings_message(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::OpenAccountSettings(id) => {
                self.settings_target = Some(id);
                self.context_page = ContextPage::AccountSettings;
                self.core.window.show_context = true;
            }
            Message::OpenDesktopSettings(key) => {
                self.settings_target = Some(key);
                self.context_page = ContextPage::DesktopSettings;
                self.core.window.show_context = true;
            }
            Message::AppToggle(t, on) => {
                self.edit_settings(Level::App, "", |s| s.set_toggle(t, Some(on)));
            }
            Message::AppTimeout(secs) => {
                self.edit_settings(Level::App, "", |s| s.set_timeout(Some(secs)));
            }
            Message::AccountToggle(id, t, choice) => {
                self.edit_settings(Level::Account, &id, |s| {
                    s.set_toggle(t, choice_value(choice));
                });
            }
            Message::AccountTimeout(id, secs) => {
                self.edit_settings(Level::Account, &id, |s| s.set_timeout(secs));
            }
            Message::DesktopToggle(key, t, choice) => {
                self.edit_settings(Level::Desktop, &key, |s| {
                    s.set_toggle(t, choice_value(choice));
                });
            }
            Message::DesktopTimeout(key, secs) => {
                self.edit_settings(Level::Desktop, &key, |s| s.set_timeout(secs));
            }
            // `update` routes only this handler's messages here.
            other => debug_assert!(false, "message routed to the wrong handler: {other:?}"),
        }
        Task::none()
    }

    /// Changes the app's, an account's or a desktop's settings and saves
    /// the config. Empty account and desktop entries are removed.
    pub(super) fn edit_settings(
        &mut self,
        level: Level,
        key: &str,
        f: impl FnOnce(&mut ConnectionSettings),
    ) {
        if let Level::App = level {
            let mut app = self.config.app_settings.clone();
            f(&mut app);
            match &self.config_handler {
                Some(handler) => {
                    let _ = self.config.set_app_settings(handler, app);
                }
                None => self.config.app_settings = app,
            }
            return;
        }
        let mut map = match level {
            Level::Account => self.config.account_settings.clone(),
            _ => self.config.desktop_settings.clone(),
        };
        let entry = map.entry(key.to_owned()).or_default();
        f(entry);
        if entry.is_empty() {
            map.remove(key);
        }
        let Some(handler) = &self.config_handler else {
            match level {
                Level::Account => self.config.account_settings = map,
                _ => self.config.desktop_settings = map,
            }
            return;
        };
        let _ = match level {
            Level::Account => self.config.set_account_settings(handler, map),
            _ => self.config.set_desktop_settings(handler, map),
        };
    }

    /// The settings one level inherits: the app's for an account, and the
    /// account's (over the app's) for a desktop.
    fn inherited(&self, account: Option<&str>) -> settings::Effective {
        settings::effective(
            Some(&self.config.app_settings),
            account.and_then(|a| self.config.account_settings.get(a)),
            None,
        )
    }

    /// The settings a desktop of `account` uses.
    pub(super) fn effective_settings(&self, account: &str, key: &str) -> settings::Effective {
        settings::effective(
            Some(&self.config.app_settings),
            self.config.account_settings.get(account),
            self.config.desktop_settings.get(key),
        )
    }

    pub(super) fn app_settings_view(&self) -> Element<'_, Message> {
        let current = self.inherited(None);
        let toggle = |t: Toggle| -> Element<'_, Message> {
            let (name, description) = toggle_text(t);
            widget::settings::item::builder(name)
                .description(description)
                .control(
                    widget::toggler(current.toggle(t))
                        .on_toggle(move |on| Message::AppToggle(t, on)),
                )
                .into()
        };
        let timeout = widget::settings::item::builder(fl!("setting-timeout"))
            .description(fl!("setting-timeout-desc"))
            .control(timeout_spin(current.timeout_secs, Message::AppTimeout))
            .into();
        settings_column(fl!("app-settings-hint"), timeout, toggle)
    }

    pub(super) fn account_settings_view(&self, id: &str) -> Element<'_, Message> {
        let own = self
            .config
            .account_settings
            .get(id)
            .cloned()
            .unwrap_or_default();
        let rows = OverrideRows {
            inherited: self.inherited(None),
            own,
            labels: (&self.follow_app_on, &self.follow_app_off),
            reset_label: fl!("use-app-setting"),
            timeout_description: |secs| fl!("setting-timeout-account-desc", value = secs),
        };
        let id_toggle = id.to_owned();
        let id_timeout = id.to_owned();
        rows.view(
            fl!("account-settings-hint"),
            move |t, i| Message::AccountToggle(id_toggle.clone(), t, i),
            move |secs| Message::AccountTimeout(id_timeout.clone(), secs),
        )
    }

    pub(super) fn desktop_settings_view(&self, key: &str) -> Element<'_, Message> {
        let Some(account) = self.account_of_resource(key) else {
            return widget::text::body(fl!("desktop-gone")).into();
        };
        let own = self
            .config
            .desktop_settings
            .get(key)
            .cloned()
            .unwrap_or_default();
        let rows = OverrideRows {
            inherited: self.inherited(Some(&account.id)),
            own,
            labels: (&self.follow_account_on, &self.follow_account_off),
            reset_label: fl!("use-account-setting"),
            timeout_description: |secs| fl!("setting-timeout-desktop-desc", value = secs),
        };
        let key_toggle = key.to_owned();
        let key_timeout = key.to_owned();
        rows.view(
            fl!("desktop-settings-hint", account = account.label()),
            move |t, i| Message::DesktopToggle(key_toggle.clone(), t, i),
            move |secs| Message::DesktopTimeout(key_timeout.clone(), secs),
        )
    }
}

/// An account or desktop page: each setting follows the level above or
/// overrides it.
struct OverrideRows<'a, D: Fn(u32) -> String> {
    /// What the level above gives.
    inherited: settings::Effective,
    /// This level's own overrides.
    own: ConnectionSettings,
    /// Dropdown labels when the level above is on, and when it is off.
    labels: (&'a Vec<String>, &'a Vec<String>),
    reset_label: String,
    timeout_description: D,
}

impl<'a, D: Fn(u32) -> String> OverrideRows<'a, D> {
    fn view(
        self,
        hint: String,
        on_toggle: impl Fn(Toggle, usize) -> Message + Clone + Send + Sync + 'static,
        on_timeout: impl Fn(Option<u32>) -> Message + Clone + 'static,
    ) -> Element<'a, Message> {
        let Self {
            inherited,
            own,
            labels,
            reset_label,
            timeout_description,
        } = self;
        let current = inherited.with(Some(&own));
        let has_own_timeout = own.timeout_secs.is_some();
        let toggle = move |t: Toggle| -> Element<'a, Message> {
            let (name, description) = toggle_text(t);
            let options = if inherited.toggle(t) {
                labels.0
            } else {
                labels.1
            };
            let selected = match own.toggle(t) {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            };
            let on_toggle = on_toggle.clone();
            widget::settings::item::builder(name)
                .description(description)
                .control(widget::dropdown(options, Some(selected), move |i| {
                    on_toggle(t, i)
                }))
                .into()
        };
        let set_timeout = on_timeout.clone();
        let mut timeout_row = widget::row::with_capacity(2)
            .spacing(cosmic::theme::spacing().space_xs)
            .align_y(Alignment::Center)
            .push(timeout_spin(current.timeout_secs, move |v| {
                set_timeout(Some(v))
            }));
        if has_own_timeout {
            timeout_row =
                timeout_row.push(widget::button::text(reset_label).on_press(on_timeout(None)));
        }
        let timeout = widget::settings::item::builder(fl!("setting-timeout"))
            .description(timeout_description(inherited.timeout_secs))
            .control(timeout_row)
            .into();
        settings_column(hint, timeout, toggle)
    }
}

/// A settings page: hint, then the connection, display and device sections.
fn settings_column<'a>(
    hint: String,
    timeout: Element<'a, Message>,
    toggle: impl Fn(Toggle) -> Element<'a, Message>,
) -> Element<'a, Message> {
    let connection = widget::settings::section()
        .title(fl!("settings-connection"))
        .add(timeout);
    let display = widget::settings::section()
        .title(fl!("settings-display"))
        .add(toggle(Toggle::Fullscreen))
        .add(toggle(Toggle::AllMonitors))
        .add(toggle(Toggle::NewWorkspace));
    let devices = widget::settings::section()
        .title(fl!("settings-devices"))
        .add(toggle(Toggle::Sound))
        .add(toggle(Toggle::Microphone))
        .add(toggle(Toggle::Camera))
        .add(toggle(Toggle::Clipboard));
    widget::settings::view_column(vec![
        widget::text::body(hint).into(),
        connection.into(),
        display.into(),
        devices.into(),
    ])
    .into()
}

/// The "time to wait" spin button: seconds, in steps of 10, within the
/// allowed range.
fn timeout_spin<'a>(
    secs: u32,
    on_change: impl Fn(u32) -> Message + 'static,
) -> widget::SpinButton<'a, u32, Message> {
    widget::spin_button(
        fl!("seconds", value = secs),
        fl!("setting-timeout"),
        secs,
        10,
        settings::TIMEOUT_MIN_SECS,
        settings::TIMEOUT_MAX_SECS,
        on_change,
    )
}

/// Name and description of an on/off setting.
pub(super) fn toggle_text(t: Toggle) -> (String, String) {
    match t {
        Toggle::Fullscreen => (fl!("setting-fullscreen"), fl!("setting-fullscreen-desc")),
        Toggle::AllMonitors => (
            fl!("setting-all-monitors"),
            fl!("setting-all-monitors-desc"),
        ),
        Toggle::NewWorkspace => (
            fl!("setting-new-workspace"),
            fl!("setting-new-workspace-desc"),
        ),
        Toggle::Sound => (fl!("setting-sound"), fl!("setting-sound-desc")),
        Toggle::Microphone => (fl!("setting-microphone"), fl!("setting-microphone-desc")),
        Toggle::Camera => (fl!("setting-camera"), fl!("setting-camera-desc")),
        Toggle::Clipboard => (fl!("setting-clipboard"), fl!("setting-clipboard-desc")),
    }
}
