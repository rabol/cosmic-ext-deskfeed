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
            Message::AppNumber(n, value) => {
                self.edit_settings(Level::App, "", |s| s.set_number(n, Some(value)));
            }
            Message::AppChoice(c, i) => {
                let value = c.options().get(i).copied();
                self.edit_settings(Level::App, "", |s| s.set_choice(c, value));
            }
            Message::AccountChoice(id, c, i) => {
                let value = i.checked_sub(1).and_then(|i| c.options().get(i).copied());
                self.edit_settings(Level::Account, &id, |s| s.set_choice(c, value));
            }
            Message::DesktopChoice(key, c, i) => {
                let value = i.checked_sub(1).and_then(|i| c.options().get(i).copied());
                self.edit_settings(Level::Desktop, &key, |s| s.set_choice(c, value));
            }
            Message::AppTheme(i) => {
                let value = u32::try_from(i).unwrap_or(0).min(2);
                match &self.config_handler {
                    Some(handler) => {
                        let _ = self.config.set_app_theme(handler, Some(value));
                    }
                    None => self.config.app_theme = Some(value),
                }
                return apply_theme(value);
            }
            Message::AppBackground(on) => {
                match &self.config_handler {
                    Some(handler) => {
                        let _ = self.config.set_background(handler, on);
                    }
                    None => self.config.background = on,
                }
                if !on {
                    // Back to closing ends the app; the tray goes with the
                    // subscription.
                    self.tray = None;
                    self.tray_unavailable = None;
                }
            }
            Message::AppAutostart(on) => match crate::autostart::set(on) {
                Ok(()) => {
                    self.autostart = on;
                    crate::app_log!("start at login: {}", if on { "on" } else { "off" });
                }
                Err(e) => crate::app_log!("start at login not changed: {e}"),
            },
            Message::AppLogSize(mib) => {
                let (min, max, _) = crate::log::MAX_SIZE_MIB;
                let mib = mib.clamp(min, max);
                crate::log::set_max_size_mib(mib);
                match &self.config_handler {
                    Some(handler) => {
                        let _ = self.config.set_log_max_mib(handler, Some(mib));
                    }
                    None => self.config.log_max_mib = Some(mib),
                }
            }
            Message::AccountToggle(id, t, choice) => {
                self.edit_settings(Level::Account, &id, |s| {
                    s.set_toggle(t, choice_value(choice));
                });
            }
            Message::AccountNumber(id, n, value) => {
                self.edit_settings(Level::Account, &id, |s| s.set_number(n, value));
            }
            Message::DesktopToggle(key, t, choice) => {
                self.edit_settings(Level::Desktop, &key, |s| {
                    s.set_toggle(t, choice_value(choice));
                });
            }
            Message::DesktopNumber(key, n, value) => {
                self.edit_settings(Level::Desktop, &key, |s| s.set_number(n, value));
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

    /// The app's settings with `account`'s on top; the app's alone for an
    /// account being added.
    pub(super) fn account_settings(&self, account: Option<&str>) -> settings::Effective {
        self.inherited(account)
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
        let number = |n: Number| -> Element<'_, Message> {
            let (name, description) = number_text(n);
            widget::settings::item::builder(name)
                .description(description)
                .control(number_spin(n, current.number(n), move |v| {
                    Message::AppNumber(n, v)
                }))
                .into()
        };
        let choice = |c: Choice| -> Element<'_, Message> {
            let (name, description) = choice_text(c);
            let labels: Vec<String> = c.options().iter().map(|&v| choice_label(c, v)).collect();
            let selected = c.options().iter().position(|&v| v == current.choice(c));
            widget::settings::item::builder(name)
                .description(description)
                .control(widget::dropdown(labels, selected, move |i| {
                    Message::AppChoice(c, i)
                }))
                .into()
        };
        let appearance = widget::settings::section()
            .title(fl!("settings-appearance"))
            .add(
                widget::settings::item::builder(fl!("setting-theme")).control(widget::dropdown(
                    vec![fl!("theme-system"), fl!("theme-dark"), fl!("theme-light")],
                    Some(self.config.app_theme.unwrap_or(0).min(2) as usize),
                    Message::AppTheme,
                )),
            );
        let background_desc = match (&self.tray_unavailable, self.config.background) {
            (Some(why), true) => fl!("setting-background-unavailable", detail = why.clone()),
            _ => fl!("setting-background-desc"),
        };
        let background = widget::settings::section()
            .title(fl!("settings-background"))
            .add(
                widget::settings::item::builder(fl!("setting-background"))
                    .description(background_desc)
                    .control(
                        widget::toggler(self.config.background).on_toggle(Message::AppBackground),
                    ),
            )
            .add(
                widget::settings::item::builder(fl!("setting-autostart"))
                    .description(fl!("setting-autostart-desc"))
                    .control(widget::toggler(self.autostart).on_toggle(Message::AppAutostart)),
            );
        let (min, max, default) = crate::log::MAX_SIZE_MIB;
        let mib = self.config.log_max_mib.unwrap_or(default).clamp(min, max);
        let diagnostics = widget::settings::section()
            .title(fl!("settings-diagnostics"))
            .add(
                widget::settings::item::builder(fl!("setting-log-size"))
                    .description(fl!("setting-log-size-desc"))
                    .control(widget::spin_button(
                        fl!("mebibytes", value = mib),
                        fl!("setting-log-size"),
                        mib,
                        1,
                        min,
                        max,
                        Message::AppLogSize,
                    )),
            );
        settings_column(
            fl!("app-settings-hint"),
            true,
            number,
            toggle,
            choice,
            Some(
                widget::column::with_capacity(3)
                    .push(background)
                    .push(appearance)
                    .push(diagnostics)
                    .spacing(cosmic::theme::spacing().space_m)
                    .into(),
            ),
        )
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
            follow_value: |value| fl!("follow-app-value", value = value),
            follow_choice: |value| fl!("follow-app-choice", value = value),
        };
        let id_toggle = id.to_owned();
        let id_number = id.to_owned();
        let id_choice = id.to_owned();
        rows.view(
            fl!("account-settings-hint"),
            true,
            move |t, i| Message::AccountToggle(id_toggle.clone(), t, i),
            move |n, v| Message::AccountNumber(id_number.clone(), n, v),
            move |c, i| Message::AccountChoice(id_choice.clone(), c, i),
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
            follow_value: |value| fl!("follow-account-value", value = value),
            follow_choice: |value| fl!("follow-account-choice", value = value),
        };
        let key_toggle = key.to_owned();
        let key_number = key.to_owned();
        let key_choice = key.to_owned();
        let mut security = widget::settings::section().title(fl!("settings-security"));
        let mut any = false;
        if let Some(fp) = self.config.certificate_pins.get(key) {
            // The start and end are enough to recognize it.
            let short = format!(
                "{}…{}",
                &fp[..11.min(fp.len())],
                &fp[fp.len().saturating_sub(11)..]
            );
            security = security.add(
                widget::settings::item::builder(fl!("setting-certificate"))
                    .description(fl!("setting-certificate-desc", fingerprint = short))
                    .control(
                        widget::button::destructive(fl!("remove"))
                            .on_press(Message::ForgetCertificate(key.to_owned())),
                    ),
            );
            any = true;
        }
        if self.config.saved_passwords.contains(key) {
            security = security.add(
                widget::settings::item::builder(fl!("setting-saved-password"))
                    .description(fl!("setting-saved-password-desc"))
                    .control(
                        widget::button::destructive(fl!("remove"))
                            .on_press(Message::ForgetPassword(key.to_owned())),
                    ),
            );
            any = true;
        }
        let pin = any.then(|| security.into());
        rows.view_with(
            pin,
            fl!("desktop-settings-hint", account = account.label()),
            false,
            move |t, i| Message::DesktopToggle(key_toggle.clone(), t, i),
            move |n, v| Message::DesktopNumber(key_number.clone(), n, v),
            move |c, i| Message::DesktopChoice(key_choice.clone(), c, i),
        )
    }
}

/// An account or desktop page: each setting follows the level above or
/// overrides it.
struct OverrideRows<'a, F: Fn(String) -> String> {
    /// What the level above gives.
    inherited: settings::Effective,
    /// This level's own overrides.
    own: ConnectionSettings,
    /// Dropdown labels when the level above is on, and when it is off.
    labels: (&'a Vec<String>, &'a Vec<String>),
    reset_label: String,
    /// "The app setting is …" for a number's description.
    follow_value: F,
    /// "App setting (…)" as a choice's first entry.
    follow_choice: fn(String) -> String,
}

impl<'a, F: Fn(String) -> String> OverrideRows<'a, F> {
    fn view(
        self,
        hint: String,
        account_level: bool,
        on_toggle: impl Fn(Toggle, usize) -> Message + Clone + Send + Sync + 'static,
        on_number: impl Fn(Number, Option<u32>) -> Message + Clone + 'static,
        on_choice: impl Fn(Choice, usize) -> Message + Clone + Send + Sync + 'static,
    ) -> Element<'a, Message> {
        self.view_with(None, hint, account_level, on_toggle, on_number, on_choice)
    }

    /// With `last` at the end of the page.
    fn view_with(
        self,
        last: Option<Element<'a, Message>>,
        hint: String,
        account_level: bool,
        on_toggle: impl Fn(Toggle, usize) -> Message + Clone + Send + Sync + 'static,
        on_number: impl Fn(Number, Option<u32>) -> Message + Clone + 'static,
        on_choice: impl Fn(Choice, usize) -> Message + Clone + Send + Sync + 'static,
    ) -> Element<'a, Message> {
        let Self {
            inherited,
            own,
            labels,
            reset_label,
            follow_value,
            follow_choice,
        } = self;
        let current = inherited.with(Some(&own));
        let number = |n: Number| -> Element<'a, Message> {
            let (name, description) = number_text(n);
            let set = on_number.clone();
            let mut row = widget::row::with_capacity(2)
                .spacing(cosmic::theme::spacing().space_xs)
                .align_y(Alignment::Center)
                .push(number_spin(n, current.number(n), move |v| set(n, Some(v))));
            if own.number(n).is_some() {
                row = row
                    .push(widget::button::text(reset_label.clone()).on_press(on_number(n, None)));
            }
            let inherited_value = number_label(n, inherited.number(n));
            widget::settings::item::builder(name)
                .description(format!("{description} {}", follow_value(inherited_value)))
                .control(row)
                .into()
        };
        let toggle = |t: Toggle| -> Element<'a, Message> {
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
        let choice = |c: Choice| -> Element<'a, Message> {
            let (name, description) = choice_text(c);
            // First the level above's value, then the options.
            let mut labels = vec![follow_choice(choice_label(c, inherited.choice(c)))];
            labels.extend(c.options().iter().map(|&v| choice_label(c, v)));
            let selected = own
                .choice(c)
                .and_then(|v| c.options().iter().position(|&o| o == v))
                .map_or(0, |i| i + 1);
            let on_choice = on_choice.clone();
            widget::settings::item::builder(name)
                .description(description)
                .control(widget::dropdown(labels, Some(selected), move |i| {
                    on_choice(c, i)
                }))
                .into()
        };
        settings_column(hint, account_level, number, toggle, choice, last)
    }
}

/// A settings page: hint, the account section (app and account pages),
/// the connection, display and device sections, then `last`.
fn settings_column<'a>(
    hint: String,
    account_level: bool,
    number: impl Fn(Number) -> Element<'a, Message>,
    toggle: impl Fn(Toggle) -> Element<'a, Message>,
    choice: impl Fn(Choice) -> Element<'a, Message>,
    last: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut column = vec![widget::text::body(hint).into()];
    if account_level {
        column.push(
            widget::settings::section()
                .title(fl!("settings-account"))
                .add(number(Number::SignIn))
                .add(number(Number::Refresh))
                .add(number(Number::OfflineDays))
                .into(),
        );
    }
    let connection = widget::settings::section()
        .title(fl!("settings-connection"))
        .add(number(Number::Timeout))
        .add(toggle(Toggle::Reconnect))
        .add(number(Number::ReconnectAttempts));
    let display = widget::settings::section()
        .title(fl!("settings-display"))
        .add(choice(Choice::Scale))
        .add(toggle(Toggle::Fullscreen))
        .add(toggle(Toggle::AllMonitors))
        .add(toggle(Toggle::NewWorkspace))
        .add(toggle(Toggle::KeepAwake));
    let keyboard = widget::settings::section()
        .title(fl!("settings-keyboard"))
        .add(toggle(Toggle::SystemShortcuts));
    let devices = widget::settings::section()
        .title(fl!("settings-devices"))
        .add(toggle(Toggle::Sound))
        .add(toggle(Toggle::Microphone))
        .add(toggle(Toggle::Camera))
        .add(choice(Choice::Clipboard))
        .add(toggle(Toggle::ClipboardFiles));
    column.extend([
        connection.into(),
        display.into(),
        keyboard.into(),
        devices.into(),
    ]);
    column.extend(last);
    widget::settings::view_column(column).into()
}

/// A number's spin button, within its range and in its steps.
fn number_spin<'a>(
    n: Number,
    value: u32,
    on_change: impl Fn(u32) -> Message + 'static,
) -> widget::SpinButton<'a, u32, Message> {
    let (min, max, step) = n.range();
    widget::spin_button(
        number_label(n, value),
        number_text(n).0,
        value,
        step,
        min,
        max,
        on_change,
    )
}

/// Applies Deskfeed's appearance: 0 follows the desktop, 1 dark, 2
/// light, each with the desktop's own palette.
///
/// A forced mode sets `prefer_dark`, which libcosmic checks before it
/// follows a desktop mode change (`app/cosmic.rs`,
/// `SystemThemeModeChange`); without it, Dark would turn light again.
pub(super) fn apply_theme(value: u32) -> Task<cosmic::Action<Message>> {
    let (mut theme, prefer) = match value {
        1 => (cosmic::theme::system_dark(), Some(true)),
        2 => (cosmic::theme::system_light(), Some(false)),
        _ => (cosmic::theme::system_preference(), None),
    };
    if let cosmic::theme::ThemeType::System { prefer_dark, .. } = &mut theme.theme_type {
        *prefer_dark = prefer;
    }
    cosmic::command::set_theme(theme)
}

/// Name and description of a choice.
fn choice_text(c: Choice) -> (String, String) {
    match c {
        Choice::Scale => (fl!("setting-scale"), fl!("setting-scale-desc")),
        Choice::Clipboard => (fl!("setting-clipboard"), fl!("setting-clipboard-desc")),
    }
}

/// One option of a choice.
fn choice_label(c: Choice, value: u32) -> String {
    match (c, value) {
        (Choice::Scale, 0) => fl!("scale-automatic"),
        (Choice::Scale, v) => fl!("percent", value = v),
        (Choice::Clipboard, settings::clipboard::BOTH) => fl!("clipboard-both"),
        (Choice::Clipboard, settings::clipboard::TO_DESKTOP) => fl!("clipboard-to-desktop"),
        (Choice::Clipboard, settings::clipboard::TO_COMPUTER) => fl!("clipboard-to-computer"),
        (Choice::Clipboard, _) => fl!("off"),
    }
}

/// A number with its unit.
fn number_label(n: Number, value: u32) -> String {
    match n {
        Number::Timeout => fl!("seconds", value = value),
        Number::Refresh | Number::SignIn => fl!("minutes", value = value),
        Number::OfflineDays => fl!("days", value = value),
        Number::ReconnectAttempts => value.to_string(),
    }
}

/// Name and description of a number setting.
fn number_text(n: Number) -> (String, String) {
    match n {
        Number::Timeout => (fl!("setting-timeout"), fl!("setting-timeout-desc")),
        Number::Refresh => (fl!("setting-refresh"), fl!("setting-refresh-desc")),
        Number::SignIn => (fl!("setting-sign-in"), fl!("setting-sign-in-desc")),
        Number::OfflineDays => (fl!("setting-offline"), fl!("setting-offline-desc")),
        Number::ReconnectAttempts => (
            fl!("setting-reconnect-attempts"),
            fl!("setting-reconnect-attempts-desc"),
        ),
    }
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
        Toggle::ClipboardFiles => (
            fl!("setting-clipboard-files"),
            fl!("setting-clipboard-files-desc"),
        ),
        Toggle::KeepAwake => (fl!("setting-keep-awake"), fl!("setting-keep-awake-desc")),
        Toggle::SystemShortcuts => (
            fl!("setting-system-shortcuts"),
            fl!("setting-system-shortcuts-desc"),
        ),
        Toggle::Reconnect => (fl!("setting-reconnect"), fl!("setting-reconnect-desc")),
    }
}
