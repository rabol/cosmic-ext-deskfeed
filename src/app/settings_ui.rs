// SPDX-License-Identifier: MIT

//! The app, account and desktop settings pages and saving them.
//!
//! Each level overrides the one above: built-in defaults → app → account →
//! desktop (`settings.rs`). The app page sets values; the account and
//! desktop pages either follow the level above or override it.

use super::*;

/// A change on the app page, as a message.
fn app_edit(edit: settings::Edit) -> Message {
    Message::Settings(SettingsMessage::Edit(Level::App, edit))
}

/// An override's dropdown position: follow the level above, on, off.
pub(super) fn override_toggle(position: usize) -> Option<bool> {
    match position {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// A choice's dropdown position on the app page: the options in order.
pub(super) fn app_choice(c: Choice, position: usize) -> Option<u32> {
    c.options().get(position).copied()
}

/// An override's dropdown position for a choice: follow the level above,
/// then the options.
pub(super) fn override_choice(c: Choice, position: usize) -> Option<u32> {
    position.checked_sub(1).and_then(|i| app_choice(c, i))
}

impl AppModel {
    /// Settings messages: opening and changing app, account and desktop
    /// settings. Routed here by `route`.
    pub(super) fn settings_message(
        &mut self,
        message: SettingsMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            SettingsMessage::OpenAccountSettings(id) => {
                self.settings_target = Some(id);
                self.context_page = ContextPage::AccountSettings;
                self.core.window.show_context = true;
            }
            // A saved connection's own page is its form (ADR 0029).
            SettingsMessage::OpenDesktopSettings(key)
                if self.config.connections.contains_key(&key) =>
            {
                return self.rdp_message(RdpMessage::Edit(key));
            }
            SettingsMessage::OpenDesktopSettings(key) => {
                self.settings_target = Some(key);
                self.context_page = ContextPage::DesktopSettings;
                self.core.window.show_context = true;
            }
            SettingsMessage::Edit(level, edit) => self.edit_settings(&level, edit),
            SettingsMessage::ChooseFolder(key) => {
                self.folder_error = None;
                return cosmic::task::future(async move {
                    let dialog = cosmic::dialog::file_chooser::open::Dialog::new()
                        .title(fl!("shared-folder-choose-title"));
                    let result = match dialog.open_folder().await {
                        Ok(response) => response
                            .url()
                            .to_file_path()
                            .ok()
                            .and_then(|p| p.to_str().map(str::to_owned))
                            // Not a local path, or not UTF-8: FreeRDP's
                            // argument can't carry it.
                            .ok_or_else(|| Some(fl!("shared-folder-unusable"))),
                        Err(cosmic::dialog::file_chooser::Error::Cancelled) => Err(None),
                        Err(e) => Err(Some(fl!(
                            "shared-folder-chooser-failed",
                            detail = e.to_string()
                        ))),
                    };
                    Message::Settings(SettingsMessage::FolderChosen { key, result })
                });
            }
            SettingsMessage::FolderChosen { key, result } => match result {
                Ok(path) if crate::shared_folder::check_form(&path).is_ok() => {
                    self.folder_error = None;
                    let mut all = self.config.shared_folders.clone();
                    all.insert(key, path);
                    save_config!(self, shared_folders, set_shared_folders, all);
                }
                Ok(_) => self.folder_error = Some((key, fl!("shared-folder-unusable"))),
                Err(Some(e)) => self.folder_error = Some((key, e)),
                Err(None) => {}
            },
            SettingsMessage::StopSharingFolder(key) => {
                self.folder_error = None;
                let mut all = self.config.shared_folders.clone();
                if all.remove(&key).is_some() {
                    save_config!(self, shared_folders, set_shared_folders, all);
                }
            }
            SettingsMessage::AppTheme(i) => {
                let value = u32::try_from(i).unwrap_or(0).min(2);
                save_config!(self, app_theme, set_app_theme, Some(value));
                return apply_theme(value);
            }
            SettingsMessage::AppBackground(on) => {
                save_config!(self, background, set_background, on);
                if !on {
                    // Back to closing ends the app; the tray goes with the
                    // subscription.
                    self.tray = None;
                    self.tray_unavailable = None;
                }
            }
            SettingsMessage::AppAutostart(on) => match crate::autostart::set(on) {
                Ok(()) => {
                    self.autostart = on;
                    crate::app_log!("start at login: {}", if on { "on" } else { "off" });
                }
                Err(e) => crate::app_log!("start at login not changed: {e}"),
            },
            SettingsMessage::AppLogSize(mib) => {
                let (min, max, _) = crate::log::MAX_SIZE_MIB;
                let mib = mib.clamp(min, max);
                crate::log::set_max_size_mib(mib);
                save_config!(self, log_max_mib, set_log_max_mib, Some(mib));
            }
        }
        Task::none()
    }

    /// Changes the app's, an account's or a desktop's settings and saves
    /// the config. Empty account and desktop entries are removed.
    pub(super) fn edit_settings(&mut self, level: &Level, edit: settings::Edit) {
        let (mut map, key) = match level {
            Level::App => {
                let mut app = self.config.app_settings.clone();
                app.edit(edit);
                save_config!(self, app_settings, set_app_settings, app);
                return;
            }
            Level::Account(id) => (self.config.account_settings.clone(), id),
            Level::Desktop(key) => (self.config.desktop_settings.clone(), key),
        };
        let entry = map.entry(key.clone()).or_default();
        entry.edit(edit);
        if entry.is_empty() {
            map.remove(key);
        }
        if let Level::Account(_) = level {
            save_config!(self, account_settings, set_account_settings, map);
        } else {
            save_config!(self, desktop_settings, set_desktop_settings, map);
        }
    }

    /// The app's settings with `account`'s on top; the app's alone for an
    /// account being added, and for the app page.
    pub(super) fn account_settings(&self, account: Option<&str>) -> settings::Effective {
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
        let current = self.account_settings(None);
        let toggle = |t: Toggle| -> Element<'_, Message> {
            let (name, description) = toggle_text(t);
            widget::settings::item::builder(name)
                .description(description)
                .control(
                    widget::toggler(current.toggle(t))
                        .on_toggle(move |on| app_edit(settings::Edit::Toggle(t, Some(on)))),
                )
                .into()
        };
        let number = |n: Number| -> Element<'_, Message> {
            let (name, description) = number_text(n);
            widget::settings::item::builder(name)
                .description(description)
                .control(number_spin(n, current.number(n), move |v| {
                    app_edit(settings::Edit::Number(n, Some(v)))
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
                    app_edit(settings::Edit::Choice(c, app_choice(c, i)))
                }))
                .into()
        };
        let appearance = widget::settings::section()
            .title(fl!("settings-appearance"))
            .add(
                widget::settings::item::builder(fl!("setting-theme")).control(widget::dropdown(
                    vec![fl!("theme-system"), fl!("theme-dark"), fl!("theme-light")],
                    Some(self.config.app_theme.unwrap_or(0).min(2) as usize),
                    |theme| Message::Settings(SettingsMessage::AppTheme(theme)),
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
                        widget::toggler(self.config.background)
                            .on_toggle(|on| Message::Settings(SettingsMessage::AppBackground(on))),
                    ),
            )
            .add(
                widget::settings::item::builder(fl!("setting-autostart"))
                    .description(fl!("setting-autostart-desc"))
                    .control(
                        widget::toggler(self.autostart)
                            .on_toggle(|on| Message::Settings(SettingsMessage::AppAutostart(on))),
                    ),
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
                        |mib| Message::Settings(SettingsMessage::AppLogSize(mib)),
                    )),
            );
        settings_column(
            open_note(fl!("app-settings-hint"), self.desks.open_count() > 0, false),
            Page::WithAccount,
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
            inherited: self.account_settings(None),
            own,
            labels: (&self.follow_app_on, &self.follow_app_off),
            reset_label: fl!("use-app-setting"),
            follow_value: |value: String| fl!("follow-app-value", value = value),
            follow_choice: |value: String| fl!("follow-app-choice", value = value),
        };
        rows.view(
            None,
            open_note(
                fl!("account-settings-hint"),
                self.desks.open_count_for(id) > 0,
                false,
            ),
            Page::WithAccount,
            Level::Account(id.to_owned()),
        )
    }

    pub(super) fn desktop_settings_view(&self, key: &str) -> Element<'_, Message> {
        // A saved plain RDP connection has no account: its settings follow
        // the app's (ADR 0029).
        let account = if self.config.connections.contains_key(key) {
            None
        } else {
            let Some(account) = self.account_of_resource(key) else {
                return widget::text::body(fl!("desktop-gone")).into();
            };
            Some(account)
        };
        let own = self
            .config
            .desktop_settings
            .get(key)
            .cloned()
            .unwrap_or_default();
        let rows: OverrideRows<'_, fn(String) -> String> = match account {
            Some(account) => OverrideRows {
                inherited: self.account_settings(Some(&account.id)),
                own,
                labels: (&self.follow_account_on, &self.follow_account_off),
                reset_label: fl!("use-account-setting"),
                follow_value: |value: String| fl!("follow-account-value", value = value),
                follow_choice: |value: String| fl!("follow-account-choice", value = value),
            },
            None => OverrideRows {
                inherited: self.account_settings(None),
                own,
                labels: (&self.follow_app_on, &self.follow_app_off),
                reset_label: fl!("use-app-setting"),
                follow_value: |value: String| fl!("follow-app-value", value = value),
                follow_choice: |value: String| fl!("follow-app-choice", value = value),
            },
        };
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
                    .control(widget::button::destructive(fl!("remove")).on_press(
                        Message::Connection(ConnectionMessage::ForgetCertificate(key.to_owned())),
                    )),
            );
            any = true;
        }
        if self.config.saved_passwords.contains(key) {
            security = security.add(
                widget::settings::item::builder(fl!("setting-saved-password"))
                    .description(fl!("setting-saved-password-desc"))
                    .control(widget::button::destructive(fl!("remove")).on_press(
                        Message::Connection(ConnectionMessage::ForgetPassword(key.to_owned())),
                    )),
            );
            any = true;
        }
        let mut last = widget::column::with_capacity(2).spacing(cosmic::theme::spacing().space_m);
        last = last.push(self.shared_folder_section(key));
        if any {
            last = last.push(security);
        }
        let (page, hint) = match account.map(|a| (a, a.resource(key).map(|r| r.kind))) {
            None => (Page::Desktop, fl!("connection-settings-hint")),
            Some((account, Some(ResourceKind::RemoteApp))) => (
                Page::RemoteApp,
                fl!("app-only-settings-hint", account = account.label()),
            ),
            Some((account, _)) => (
                Page::Desktop,
                fl!("desktop-settings-hint", account = account.label()),
            ),
        };
        let open = self.desks.running(key).is_some() || self.desks.launching(key).is_some();
        rows.view(
            Some(last.into()),
            open_note(hint, open, true),
            page,
            Level::Desktop(key.to_owned()),
        )
    }
}

impl AppModel {
    /// The shared folder of a desktop or connection: which one, Choose…,
    /// Stop sharing, and why a chosen one wasn't taken.
    fn shared_folder_section(&self, key: &str) -> Element<'_, Message> {
        let shared = self.config.shared_folders.get(key);
        let space = cosmic::theme::spacing();
        let mut controls = widget::row::with_capacity(2)
            .spacing(space.space_xs)
            .align_y(Alignment::Center)
            .push(
                widget::button::standard(fl!("shared-folder-choose")).on_press(Message::Settings(
                    SettingsMessage::ChooseFolder(key.to_owned()),
                )),
            );
        if shared.is_some() {
            controls = controls.push(
                widget::button::destructive(fl!("shared-folder-stop")).on_press(Message::Settings(
                    SettingsMessage::StopSharingFolder(key.to_owned()),
                )),
            );
        }
        let mut section = widget::settings::section()
            .title(fl!("settings-shared-folder"))
            .add(
                widget::settings::item::builder(fl!("setting-shared-folder"))
                    .description(fl!("setting-shared-folder-desc"))
                    .control(controls),
            )
            .add(widget::text::body(
                shared
                    .cloned()
                    .unwrap_or_else(|| fl!("setting-shared-folder-none")),
            ));
        if let Some((_, e)) = self.folder_error.as_ref().filter(|(k, _)| k == key) {
            section = section.add(widget::text::body(e.clone()));
        }
        section.into()
    }
}

/// A page's hint, plus a note while desktops it applies to are open:
/// settings reach FreeRDP only when a connection starts (its arguments
/// and connection file), so an open session keeps the old ones; without
/// the note a changed setting looks as if it doesn't work.
fn open_note(hint: String, open: bool, this_desktop: bool) -> String {
    if !open {
        return hint;
    }
    let note = if this_desktop {
        fl!("settings-apply-next-connect")
    } else {
        fl!("settings-apply-next-connect-open")
    };
    format!("{hint}\n\n{note}")
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
    /// The page for `level`, with `last` at its end.
    fn view(
        self,
        last: Option<Element<'a, Message>>,
        hint: String,
        page: Page,
        level: Level,
    ) -> Element<'a, Message> {
        let edit = move |edit| Message::Settings(SettingsMessage::Edit(level.clone(), edit));
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
            let set = edit.clone();
            let mut row = widget::row::with_capacity(2)
                .spacing(cosmic::theme::spacing().space_xs)
                .align_y(Alignment::Center)
                .push(number_spin(n, current.number(n), move |v| {
                    set(settings::Edit::Number(n, Some(v)))
                }));
            if own.number(n).is_some() {
                row = row.push(
                    widget::button::text(reset_label.clone())
                        .on_press(edit(settings::Edit::Number(n, None))),
                );
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
            let set = edit.clone();
            widget::settings::item::builder(name)
                .description(description)
                .control(widget::dropdown(options, Some(selected), move |i| {
                    set(settings::Edit::Toggle(t, override_toggle(i)))
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
            let set = edit.clone();
            widget::settings::item::builder(name)
                .description(description)
                .control(widget::dropdown(labels, Some(selected), move |i| {
                    set(settings::Edit::Choice(c, override_choice(c, i)))
                }))
                .into()
        };
        settings_column(hint, page, number, toggle, choice, last)
    }
}

/// Which settings a page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    /// App and account pages: with the account section.
    WithAccount,
    Desktop,
    /// A RemoteApp: without what only a desktop's window has (ADR 0020).
    RemoteApp,
}

/// A settings page: hint, the account section (app and account pages),
/// the connection, display and device sections, then `last`.
fn settings_column<'a>(
    hint: String,
    page: Page,
    number: impl Fn(Number) -> Element<'a, Message>,
    toggle: impl Fn(Toggle) -> Element<'a, Message>,
    choice: impl Fn(Choice) -> Element<'a, Message>,
    last: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut column = vec![widget::text::body(hint).into()];
    if page == Page::WithAccount {
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
        .add(number(Number::ReconnectAttempts))
        .add(choice(Choice::Network));
    let display = widget::settings::section()
        .title(fl!("settings-display"))
        .add(choice(Choice::Scale))
        .add(choice(Choice::ColorDepth));
    // An app's windows are ordinary windows on every monitor; full
    // screen, workspaces and the keyboard grab belong to a desktop's
    // window (`Effective::freerdp_app_args`).
    let desktop = page != Page::RemoteApp;
    let display = if desktop {
        display
            .add(toggle(Toggle::Fullscreen))
            .add(choice(Choice::WindowSize))
            .add(toggle(Toggle::AllMonitors))
            .add(toggle(Toggle::SpanMonitors))
            .add(toggle(Toggle::NewWorkspace))
            .add(toggle(Toggle::KeepAwake))
    } else {
        display
    };
    let keyboard = desktop.then(|| {
        widget::settings::section()
            .title(fl!("settings-keyboard"))
            .add(toggle(Toggle::SystemShortcuts))
            .add(toggle(Toggle::FullscreenKey))
    });
    let devices = widget::settings::section()
        .title(fl!("settings-devices"))
        .add(toggle(Toggle::Sound))
        .add(toggle(Toggle::Microphone))
        .add(toggle(Toggle::Camera))
        .add(choice(Choice::Clipboard))
        .add(toggle(Toggle::ClipboardFiles));
    column.extend([connection.into(), display.into()]);
    column.extend(keyboard.map(Into::into));
    column.push(devices.into());
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
        Choice::WindowSize => (fl!("setting-window-size"), fl!("setting-window-size-desc")),
        Choice::ColorDepth => (fl!("setting-color-depth"), fl!("setting-color-depth-desc")),
        Choice::Network => (fl!("setting-network"), fl!("setting-network-desc")),
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
        (Choice::WindowSize, 0) => fl!("window-size-default"),
        (Choice::WindowSize, v) => fl!("window-size-percent", value = v),
        (Choice::ColorDepth, v) => fl!("color-depth-bits", value = v),
        (Choice::Network, settings::network::LAN) => fl!("network-lan"),
        (Choice::Network, settings::network::WAN) => fl!("network-wan"),
        (Choice::Network, settings::network::BROADBAND_HIGH) => fl!("network-broadband-high"),
        (Choice::Network, settings::network::BROADBAND_LOW) => fl!("network-broadband-low"),
        (Choice::Network, settings::network::MODEM) => fl!("network-modem"),
        (Choice::Network, _) => fl!("network-auto"),
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
        Toggle::SpanMonitors => (fl!("setting-span"), fl!("setting-span-desc")),
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
        Toggle::FullscreenKey => (
            fl!("setting-fullscreen-key"),
            fl!("setting-fullscreen-key-desc"),
        ),
        Toggle::KeepAwake => (fl!("setting-keep-awake"), fl!("setting-keep-awake-desc")),
        Toggle::SystemShortcuts => (
            fl!("setting-system-shortcuts"),
            fl!("setting-system-shortcuts-desc"),
        ),
        Toggle::Reconnect => (fl!("setting-reconnect"), fl!("setting-reconnect-desc")),
    }
}
