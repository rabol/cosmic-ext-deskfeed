// SPDX-License-Identifier: MIT

//! Saved plain RDP connections (ADR 0029): the Connections page, the
//! "Add connection" form (also for editing one), and removing one.
//! Connecting goes through the same rows and sessions as a desktop.

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;
use crate::connections::{ConnectionError, SavedConnection};

impl AppModel {
    pub(super) fn rdp_message(&mut self, message: RdpMessage) -> Task<cosmic::Action<Message>> {
        match message {
            RdpMessage::Edit(id) => {
                let Some(c) = self.config.connections.get(&id) else {
                    return Task::none();
                };
                self.connection_form = ConnectionForm {
                    editing: Some(id),
                    name: c.name.clone(),
                    server: c.server(),
                    username: c.username.clone(),
                    domain: c.domain().to_owned(),
                    ..ConnectionForm::default()
                };
                self.rebuild_nav(&Page::AddConnection);
            }
            RdpMessage::Input(field, text) => {
                let f = &mut self.connection_form;
                match field {
                    FormField::Name => f.name = text,
                    FormField::Server => f.server = text,
                    FormField::Username => f.username = text,
                    FormField::Domain => f.domain = text,
                }
            }
            RdpMessage::PasswordInput(PasswordText(text)) => self.connection_form.password = text,
            RdpMessage::TogglePasswordVisible => {
                self.connection_form.password_shown = !self.connection_form.password_shown;
            }
            RdpMessage::Save => return self.save_connection(),
            RdpMessage::Cancel => {
                self.connection_form = ConnectionForm::default();
                self.rebuild_nav(&Page::Connections);
            }
            RdpMessage::Remove => {
                if let Some(id) = self.connection_form.editing.clone() {
                    // Open: ask first (ADR 0029), as for an account.
                    if self.desks.open_count_for(&id) > 0 {
                        self.leave_asking = Some(Leave::Connection(id));
                        return Task::none();
                    }
                    return self.remove_connection(&id);
                }
            }
            RdpMessage::OpenSettings => {
                if let Some(id) = self.connection_form.editing.clone() {
                    self.settings_target = Some(id);
                    self.context_page = ContextPage::DesktopSettings;
                    self.core.window.show_context = true;
                }
            }
            RdpMessage::PasswordStored { id, ticket, result } => match result {
                // Only while nothing changed since: a removal or Forget
                // meanwhile wins.
                Ok(true)
                    if self.config.connections.contains_key(&id)
                        && self.sign_ins.password_ticket(&id, &id) == ticket =>
                {
                    crate::app_log!("connection password remembered");
                    self.set_saved_password(&id, true);
                }
                Ok(_) => {}
                Err(e) => {
                    crate::app_log!("connection password not remembered ({e})");
                    self.notice = Some(fl!("connection-password-not-saved", detail = e));
                }
            },
        }
        Task::none()
    }

    /// Checks the form and saves the connection; a password given goes to
    /// the keyring, never the settings.
    fn save_connection(&mut self) -> Task<cosmic::Action<Message>> {
        let f = &self.connection_form;
        let saved = match SavedConnection::from_server(&f.name, &f.server, &f.username, &f.domain)
            .and_then(|c| {
                if c.username.is_empty() {
                    Err(ConnectionError::NoUserName)
                } else {
                    Ok(c)
                }
            }) {
            Ok(c) => c,
            Err(e) => {
                self.connection_form.error = Some(describe_connection_error(e));
                return Task::none();
            }
        };
        let Some(id) = f.editing.clone().or_else(crate::connections::new_id) else {
            self.connection_form.error = Some(fl!("error-generic"));
            return Task::none();
        };
        let password = std::mem::take(&mut self.connection_form.password);
        self.connection_form = ConnectionForm::default();
        let title = saved.title().to_owned();
        let mut all = self.config.connections.clone();
        all.insert(id.clone(), saved);
        save_config!(self, connections, set_connections, all);
        self.refresh_connection_rows();
        self.rebuild_nav(&Page::Connections);
        if password.is_empty() {
            return Task::none();
        }
        // Marked remembered once it is in the keyring, as from the
        // password dialog.
        let password = Secret::new(password);
        let ticket = self.sign_ins.password_ticket(&id, &id);
        let sign_ins = self.sign_ins.clone();
        cosmic::task::future(async move {
            let owner = id.clone();
            let result = off_ui(
                move || {
                    sign_ins
                        .save_connection_password(&owner, &title, &password, ticket)
                        .map_err(|e| keyring_detail(&e))
                },
                Err(fl!("error-generic")),
            )
            .await;
            Message::Rdp(RdpMessage::PasswordStored { id, ticket, result })
        })
    }

    /// Removes a saved connection and everything about it: its remembered
    /// password, approved certificate, settings, logon setting and favorite.
    pub(super) fn remove_connection(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let mut all = self.config.connections.clone();
        if all.remove(id).is_none() {
            return Task::none();
        }
        save_config!(self, connections, set_connections, all);
        let ended = self.end_connection(id);
        let mut pins = self.config.certificate_pins.clone();
        if pins.remove(id).is_some() {
            save_config!(self, certificate_pins, set_certificate_pins, pins);
        }
        let mut settings = self.config.desktop_settings.clone();
        if settings.remove(id).is_some() {
            save_config!(self, desktop_settings, set_desktop_settings, settings);
        }
        let mut logons = self.config.logon_settings.clone();
        if logons.remove(id).is_some() {
            save_config!(self, logon_settings, set_logon_settings, logons);
        }
        let mut favorites = self.config.favorites.clone();
        if favorites.remove(id) {
            save_config!(self, favorites, set_favorites, favorites);
        }
        let had_password = self.config.saved_passwords.contains(id);
        self.set_saved_password(id, false);
        self.connection_form = ConnectionForm::default();
        self.refresh_connection_rows();
        self.rebuild_nav(&Page::Connections);
        if !had_password {
            return ended;
        }
        Task::batch([ended, self.forget_connection_password(id)])
    }

    /// Ends whatever a removed connection had going: its session (stopped
    /// in the background), its dialogs and its row's state.
    pub(super) fn end_connection(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        // One still starting is stopped once it has started (`launched`).
        let running = self.desks.at(id).sign_out();
        self.withdraw_dialogs(|d| d.key == id);
        if self
            .certificate_dialog
            .as_ref()
            .is_some_and(|d| d.key == id)
        {
            self.certificate_dialog = None;
        }
        if self.context_page == ContextPage::DesktopSettings
            && self.settings_target.as_deref() == Some(id)
        {
            self.core.window.show_context = false;
        }
        let Some(session) = running else {
            return Task::none();
        };
        Task::future(async move {
            off_ui(
                move || launcher::stop_all(&[session], launcher::STOP_GRACE),
                (),
            )
            .await;
        })
        .discard()
    }

    /// The saved connections as rows, in title order.
    pub(super) fn refresh_connection_rows(&mut self) {
        let mut rows: Vec<ResourceView> = self
            .config
            .connections
            .iter()
            .map(|(id, c)| ResourceView {
                key: id.clone(),
                title: c.title().to_owned(),
                kind: ResourceKind::Desktop,
                connection_file: None,
                icon: None,
            })
            .collect();
        rows.sort_by_key(|r| r.title.to_lowercase());
        self.connection_rows = rows;
    }

    /// Favorites first, as on an account's page, then every connection.
    pub(super) fn connections_view(&self) -> Element<'_, Message> {
        let space = cosmic::theme::spacing();
        let mut col = widget::column::with_capacity(2).spacing(space.space_m);
        let favorites: Vec<&ResourceView> = self
            .connection_rows
            .iter()
            .filter(|r| self.config.favorites.contains(&r.key))
            .collect();
        if !favorites.is_empty() {
            let mut section = widget::settings::section().title(fl!("favorites"));
            for r in favorites {
                section = section.add(self.resource_row("", r, true));
            }
            col = col.push(section);
        }
        let mut section = widget::settings::section().title(fl!("connections"));
        for r in &self.connection_rows {
            section = section.add(self.resource_row("", r, true));
        }
        widget::scrollable(
            widget::container(col.push(section))
                .max_width(800)
                .padding(space.space_m),
        )
        .into()
    }

    pub(super) fn connection_form_view(&self) -> Element<'_, Message> {
        let f = &self.connection_form;
        let input = |field| move |text| Message::Rdp(RdpMessage::Input(field, text));
        let save = Message::Rdp(RdpMessage::Save);
        let remembered = f
            .editing
            .as_ref()
            .is_some_and(|id| self.config.saved_passwords.contains(id));
        let fields = widget::settings::section()
            .add(
                widget::settings::item::builder(fl!("connection-server")).control(
                    widget::text_input(fl!("connection-server-placeholder"), f.server.as_str())
                        .on_input(input(FormField::Server))
                        .on_submit(move |_| save.clone()),
                ),
            )
            .add(
                widget::settings::item::builder(fl!("connection-name")).control(
                    widget::text_input(fl!("connection-name-placeholder"), f.name.as_str())
                        .on_input(input(FormField::Name)),
                ),
            )
            .add(
                widget::settings::item::builder(fl!("connection-username")).control(
                    widget::text_input("", f.username.as_str())
                        .on_input(input(FormField::Username)),
                ),
            )
            .add(
                widget::settings::item::builder(fl!("connection-password"))
                    .description(fl!("connection-password-hint"))
                    .control(
                        widget::secure_input(
                            if remembered {
                                fl!("connection-password-keep")
                            } else {
                                fl!("connection-password-placeholder")
                            },
                            f.password.as_str(),
                            Some(Message::Rdp(RdpMessage::TogglePasswordVisible)),
                            !f.password_shown,
                        )
                        .on_input(|s| Message::Rdp(RdpMessage::PasswordInput(PasswordText(s)))),
                    ),
            )
            .add(
                widget::settings::item::builder(fl!("connection-domain")).control(
                    widget::text_input(fl!("connection-domain-placeholder"), f.domain.as_str())
                        .on_input(input(FormField::Domain)),
                ),
            );
        let space = cosmic::theme::spacing();
        let mut buttons = widget::row::with_capacity(4)
            .spacing(space.space_xs)
            .push(widget::button::suggested(fl!("save")).on_press(Message::Rdp(RdpMessage::Save)));
        if f.editing.is_some() {
            buttons = buttons
                .push(
                    widget::button::standard(fl!("cancel"))
                        .on_press(Message::Rdp(RdpMessage::Cancel)),
                )
                .push(
                    widget::button::standard(fl!("connection-settings"))
                        .on_press(Message::Rdp(RdpMessage::OpenSettings)),
                )
                .push(
                    widget::button::destructive(fl!("remove-connection"))
                        .on_press(Message::Rdp(RdpMessage::Remove)),
                );
        }
        let mut col = widget::column::with_capacity(4)
            .spacing(space.space_m)
            .push(widget::text::title3(if f.editing.is_some() {
                fl!("edit-connection")
            } else {
                fl!("add-connection")
            }))
            .push(fields);
        if let Some(e) = &f.error {
            col = col.push(widget::text::body(e.clone()));
        }
        widget::scrollable(
            widget::container(col.push(buttons))
                .max_width(800)
                .padding(space.space_m),
        )
        .into()
    }
}

/// What the form says about a field it refused.
pub(super) fn describe_connection_error(e: ConnectionError) -> String {
    match e {
        ConnectionError::NoHost => fl!("error-connection-no-host"),
        ConnectionError::BadHost => fl!("error-connection-host"),
        ConnectionError::BadPort => fl!("error-connection-port"),
        ConnectionError::BadUserName => fl!("error-connection-username"),
        ConnectionError::NoUserName => fl!("error-connection-no-username"),
        ConnectionError::BadName => fl!("error-connection-name"),
        ConnectionError::BadDomain => fl!("error-connection-domain"),
        // The form has no Entra choice; a hand-edited setting could.
        ConnectionError::EntraNotAvailable => fl!("error-generic"),
    }
}
