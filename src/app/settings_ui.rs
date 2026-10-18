// SPDX-License-Identifier: MIT

//! The account and desktop settings pages and saving them.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum Level {
    Account,
    Desktop,
}

impl AppModel {
    /// Settings messages: opening and changing account and desktop settings. Routed here by `update`.
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
            Message::AccountToggle(id, t, on) => {
                self.edit_settings(Level::Account, &id, |s| s.set_toggle(t, Some(on)));
            }
            Message::AccountTimeout(id, secs) => {
                self.edit_settings(Level::Account, &id, |s| s.set_timeout(Some(secs)));
            }
            Message::DesktopToggle(key, t, choice) => {
                let value = match choice {
                    1 => Some(true),
                    2 => Some(false),
                    _ => None,
                };
                self.edit_settings(Level::Desktop, &key, |s| s.set_toggle(t, value));
            }
            Message::DesktopTimeout(key, secs) => {
                self.edit_settings(Level::Desktop, &key, |s| s.set_timeout(secs));
            }
            // `update` routes only this handler's messages here.
            other => debug_assert!(false, "message routed to the wrong handler: {other:?}"),
        }
        Task::none()
    }

    /// Changes one account's or desktop's settings and saves the config.
    /// Empty entries are removed.
    pub(super) fn edit_settings(
        &mut self,
        level: Level,
        key: &str,
        f: impl FnOnce(&mut ConnectionSettings),
    ) {
        let mut map = match level {
            Level::Account => self.config.account_settings.clone(),
            Level::Desktop => self.config.desktop_settings.clone(),
        };
        let entry = map.entry(key.to_owned()).or_default();
        f(entry);
        if entry.is_empty() {
            map.remove(key);
        }
        let Some(handler) = &self.config_handler else {
            match level {
                Level::Account => self.config.account_settings = map,
                Level::Desktop => self.config.desktop_settings = map,
            }
            return;
        };
        let _ = match level {
            Level::Account => self.config.set_account_settings(handler, map),
            Level::Desktop => self.config.set_desktop_settings(handler, map),
        };
    }

    /// The settings a desktop of `account` uses.
    pub(super) fn effective_settings(&self, account: &str, key: &str) -> settings::Effective {
        settings::effective(
            self.config.account_settings.get(account),
            self.config.desktop_settings.get(key),
        )
    }

    pub(super) fn account_settings_view(&self, id: &str) -> Element<'_, Message> {
        let current = settings::effective(self.config.account_settings.get(id), None);
        let toggle = |t: Toggle| {
            let (name, description) = toggle_text(t);
            let id = id.to_owned();
            widget::settings::item::builder(name)
                .description(description)
                .toggler(current.toggle(t), move |on| {
                    Message::AccountToggle(id.clone(), t, on)
                })
        };
        let timeout_id = id.to_owned();
        let connection = widget::settings::section()
            .title(fl!("settings-connection"))
            .add(
                widget::settings::item::builder(fl!("setting-timeout"))
                    .description(fl!("setting-timeout-desc"))
                    .control(timeout_spin(current.timeout_secs, move |v| {
                        Message::AccountTimeout(timeout_id.clone(), v)
                    })),
            );
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
            widget::text::body(fl!("account-settings-hint")).into(),
            connection.into(),
            display.into(),
            devices.into(),
        ])
        .into()
    }

    pub(super) fn desktop_settings_view(&self, key: &str) -> Element<'_, Message> {
        let Some(account) = self.account_of_resource(key) else {
            return widget::text::body(fl!("desktop-gone")).into();
        };
        let inherited = settings::effective(self.config.account_settings.get(&account.id), None);
        let own = self
            .config
            .desktop_settings
            .get(key)
            .cloned()
            .unwrap_or_default();
        let current = self.effective_settings(&account.id, key);
        let choice = |t: Toggle| {
            let (name, description) = toggle_text(t);
            let labels = if inherited.toggle(t) {
                &self.follow_on
            } else {
                &self.follow_off
            };
            let selected = match own.toggle(t) {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            };
            let key = key.to_owned();
            widget::settings::item::builder(name)
                .description(description)
                .control(widget::dropdown(labels, Some(selected), move |i| {
                    Message::DesktopToggle(key.clone(), t, i)
                }))
        };
        let timeout_key = key.to_owned();
        let mut timeout_row = widget::row::with_capacity(2)
            .spacing(cosmic::theme::spacing().space_xs)
            .align_y(Alignment::Center)
            .push(timeout_spin(current.timeout_secs, move |v| {
                Message::DesktopTimeout(timeout_key.clone(), Some(v))
            }));
        if own.timeout_secs.is_some() {
            timeout_row = timeout_row.push(
                widget::button::text(fl!("use-account-setting"))
                    .on_press(Message::DesktopTimeout(key.to_owned(), None)),
            );
        }
        let connection = widget::settings::section()
            .title(fl!("settings-connection"))
            .add(
                widget::settings::item::builder(fl!("setting-timeout"))
                    .description(fl!(
                        "setting-timeout-desktop-desc",
                        value = inherited.timeout_secs
                    ))
                    .control(timeout_row),
            );
        let display = widget::settings::section()
            .title(fl!("settings-display"))
            .add(choice(Toggle::Fullscreen))
            .add(choice(Toggle::AllMonitors))
            .add(choice(Toggle::NewWorkspace));
        let devices = widget::settings::section()
            .title(fl!("settings-devices"))
            .add(choice(Toggle::Sound))
            .add(choice(Toggle::Microphone))
            .add(choice(Toggle::Camera))
            .add(choice(Toggle::Clipboard));
        widget::settings::view_column(vec![
            widget::text::body(fl!("desktop-settings-hint", account = account.label())).into(),
            connection.into(),
            display.into(),
            devices.into(),
        ])
        .into()
    }
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
