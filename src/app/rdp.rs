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
                    return self.remove_connection(&id);
                }
            }
            RdpMessage::PasswordStored(result) => {
                // Status only: never the password or the host.
                crate::app_log!("connection password in the keyring: {:?}", result.is_ok());
            }
        }
        Task::none()
    }

    /// Checks the form and saves the connection; a password given goes to
    /// the keyring, never the settings.
    fn save_connection(&mut self) -> Task<cosmic::Action<Message>> {
        let f = &self.connection_form;
        let saved = match SavedConnection::from_server(&f.name, &f.server, &f.username, &f.domain) {
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
        self.set_saved_password(&id, true);
        let password = Secret::new(password);
        cosmic::task::future(async move {
            let result = off_ui(
                move || {
                    crate::secrets::save_connection_password(&id, &title, &password)
                        .map_err(|e| keyring_detail(&e))
                },
                Err(fl!("error-generic")),
            )
            .await;
            Message::Rdp(RdpMessage::PasswordStored(result))
        })
    }

    /// Removes a saved connection and everything about it: its remembered
    /// password, approved certificate, settings, logon setting and favorite.
    fn remove_connection(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let mut all = self.config.connections.clone();
        if all.remove(id).is_none() {
            return Task::none();
        }
        save_config!(self, connections, set_connections, all);
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
            return Task::none();
        }
        let id = id.to_owned();
        cosmic::task::future(async move {
            let result = off_ui(
                move || {
                    crate::secrets::delete_connection_password(&id).map_err(|e| keyring_detail(&e))
                },
                Err(fl!("error-generic")),
            )
            .await;
            Message::Rdp(RdpMessage::PasswordStored(result))
        })
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

    pub(super) fn connections_view(&self) -> Element<'_, Message> {
        let mut section = widget::settings::section().title(fl!("connections"));
        for r in &self.connection_rows {
            section = section.add(self.resource_row("", r, true));
        }
        widget::scrollable(
            widget::container(section)
                .max_width(800)
                .padding(cosmic::theme::spacing().space_m),
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
        let mut buttons = widget::row::with_capacity(3)
            .spacing(space.space_xs)
            .push(widget::button::suggested(fl!("save")).on_press(Message::Rdp(RdpMessage::Save)));
        if f.editing.is_some() {
            buttons = buttons
                .push(
                    widget::button::standard(fl!("cancel"))
                        .on_press(Message::Rdp(RdpMessage::Cancel)),
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
fn describe_connection_error(e: ConnectionError) -> String {
    match e {
        ConnectionError::NoHost => fl!("error-connection-no-host"),
        ConnectionError::BadHost => fl!("error-connection-host"),
        ConnectionError::BadPort => fl!("error-connection-port"),
        ConnectionError::BadUserName => fl!("error-connection-username"),
        ConnectionError::BadName => fl!("error-connection-name"),
        ConnectionError::BadDomain => fl!("error-connection-domain"),
        // The form has no Entra choice; a hand-edited setting could.
        ConnectionError::EntraNotAvailable => fl!("error-generic"),
    }
}
