// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use cosmic::app::context_drawer;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::alignment::{Horizontal, Vertical};
use cosmic::iced::futures::{StreamExt, channel::mpsc};
use cosmic::iced::{Alignment, Length, Subscription};
use cosmic::prelude::*;
use cosmic::widget::{self, about::About, menu};
use sha2::Digest;
use url::Url;

use crate::auth::attempt::ClientConfig;
use crate::auth::signin::{self, Cancel, SignInError, SignedIn};
use crate::auth::token::{Secret, TokenError};
use crate::config::Config;
use crate::fl;
use crate::service::discovery::{self, FetchError};
use crate::service::endpoint::{EndpointPolicy, Role};
use crate::service::feed::ResourceKind;
use crate::service::http::{Http, HttpError};
use crate::sessions::launcher::{self, LaunchSpec, LogonSetting, SessionEnd, SessionEvent};
use crate::sessions::rdpfile::ConnectionFile;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const APP_ICON: &[u8] = include_bytes!("../resources/icons/hicolor/scalable/apps/icon.svg");

pub struct AppModel {
    core: cosmic::Core,
    context_page: ContextPage,
    about: About,
    key_binds: HashMap<menu::KeyBind, MenuAction>,
    config: Config,
    config_handler: Option<cosmic_config::Config>,
    http: Arc<Http>,
    session: Session,
    /// Increases on every sign-in and sign-out; results tagged with an older
    /// value are stale and ignored (spec §8).
    generation: u64,
    /// Connection state per resource key.
    connections: HashMap<String, Connection>,
    running: HashMap<String, launcher::Session>,
    /// Logon setting and connection file of sessions not yet connected, to
    /// remember the setting on success or ask again on logon failure.
    pending_logon: HashMap<String, (LogonSetting, Arc<ConnectionFile>)>,
    dialog: Option<PasswordDialog>,
    logon_labels: Vec<String>,
}

enum Session {
    SignedOut {
        error: Option<String>,
    },
    SigningIn {
        cancel: Cancel,
    },
    Ready {
        account: Arc<SignedIn>,
        loading: bool,
        workspaces: Vec<WorkspaceView>,
        error: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct WorkspaceView {
    name: String,
    error: Option<String>,
    resources: Vec<ResourceView>,
}

#[derive(Debug, Clone)]
pub struct ResourceView {
    /// Opaque key: hash of account, tenant and resource ID.
    key: String,
    title: String,
    kind: ResourceKind,
    connection_file: Option<Url>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Connection {
    Preparing,
    Connecting,
    Connected,
    Failed(String),
}

struct PasswordDialog {
    key: String,
    title: String,
    connection: Arc<ConnectionFile>,
    password: String,
    password_hidden: bool,
    logon: usize,
    custom_domain: String,
    error: Option<String>,
}

/// Text from the password field; never shown in `Debug` output.
#[derive(Clone)]
pub struct PasswordText(String);

impl std::fmt::Debug for PasswordText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PasswordText(<redacted>)")
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    LaunchUrl(String),
    ToggleContextPage(ContextPage),
    UpdateConfig(Config),
    SignIn,
    CancelSignIn,
    SignInFinished(u64, Result<Arc<SignedIn>, SignInFailure>),
    Refresh,
    Discovered(u64, Result<Vec<WorkspaceView>, String>),
    SignOut,
    Connect(String),
    ConnectionFileReady(u64, String, Result<Arc<ConnectionFile>, String>),
    PasswordInput(PasswordText),
    TogglePasswordVisible,
    LogonSelected(usize),
    CustomDomainInput(String),
    DialogConnect,
    DialogCancel,
    Session(u64, String, SessionEvent),
    Disconnect(String),
}

/// Sign-in failures, cheap to clone for messages.
#[derive(Debug, Clone)]
pub struct SignInFailure(Option<String>);

const LOGON_AUTOMATIC: usize = 0;
const LOGON_EMPTY: usize = 1;
const LOGON_CUSTOM: usize = 2;
const LOGON_ENTRA: usize = 3;

fn logon_index(setting: &LogonSetting) -> (usize, String) {
    match setting {
        LogonSetting::Automatic => (LOGON_AUTOMATIC, String::new()),
        LogonSetting::EmptyDomain => (LOGON_EMPTY, String::new()),
        LogonSetting::CustomDomain(d) => (LOGON_CUSTOM, d.clone()),
        LogonSetting::EntraSignIn => (LOGON_ENTRA, String::new()),
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "io.github.rabol.cosmic-ext-avd";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        launcher::clean_stale_runtime_dirs();
        let about = About::default()
            .name(fl!("app-title"))
            .icon(widget::icon::from_svg_bytes(APP_ICON))
            .version(env!("CARGO_PKG_VERSION"))
            .links([(fl!("repository"), REPOSITORY)])
            .license(env!("CARGO_PKG_LICENSE"));
        let config_handler = cosmic_config::Config::new(Self::APP_ID, Config::VERSION).ok();
        let config = config_handler
            .as_ref()
            .map(|context| match Config::get_entry(context) {
                Ok(config) | Err((_, config)) => config,
            })
            .unwrap_or_default();

        let mut app = AppModel {
            core,
            context_page: ContextPage::default(),
            about,
            key_binds: HashMap::new(),
            config,
            config_handler,
            http: Arc::new(Http::new(EndpointPolicy::azure_public())),
            session: Session::SignedOut { error: None },
            generation: 0,
            connections: HashMap::new(),
            running: HashMap::new(),
            pending_logon: HashMap::new(),
            dialog: None,
            logon_labels: vec![
                fl!("logon-automatic"),
                fl!("logon-empty-domain"),
                fl!("logon-custom-domain"),
                fl!("logon-entra"),
            ],
        };
        let command = app.update_title();
        (app, command)
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        let menu_bar = menu::bar(vec![menu::Tree::with_children(
            menu::root(fl!("view")).apply(Element::from),
            menu::items(
                &self.key_binds,
                vec![menu::Item::Button(fl!("about"), None, MenuAction::About)],
            ),
        )]);
        vec![menu_bar.into()]
    }

    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        let Session::Ready {
            account, loading, ..
        } = &self.session
        else {
            return vec![];
        };
        let name = account
            .identity
            .username
            .clone()
            .or_else(|| account.identity.display_name.clone())
            .unwrap_or_default();
        let mut refresh = widget::button::text(fl!("refresh"));
        if !loading {
            refresh = refresh.on_press(Message::Refresh);
        }
        vec![
            widget::text::body(name).into(),
            refresh.into(),
            widget::button::text(fl!("sign-out"))
                .on_press(Message::SignOut)
                .into(),
        ]
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Self::Message>> {
        if !self.core.window.show_context {
            return None;
        }
        Some(match self.context_page {
            ContextPage::About => context_drawer::about(
                &self.about,
                |url| Message::LaunchUrl(url.to_string()),
                Message::ToggleContextPage(ContextPage::About),
            ),
        })
    }

    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        let d = self.dialog.as_ref()?;
        let account = match &self.session {
            Session::Ready { account, .. } => account.identity.username.clone().unwrap_or_default(),
            _ => String::new(),
        };
        let space = cosmic::theme::spacing();
        let mut controls = widget::column::with_capacity(6).spacing(space.space_s);
        controls = controls.push(widget::text::body(fl!("password-for", account = account)));
        if d.logon != LOGON_ENTRA {
            controls = controls.push(
                widget::secure_input(
                    fl!("windows-password"),
                    d.password.as_str(),
                    Some(Message::TogglePasswordVisible),
                    d.password_hidden,
                )
                .on_input(|s| Message::PasswordInput(PasswordText(s)))
                .on_submit(|_| Message::DialogConnect),
            );
        }
        controls = controls.push(
            widget::column::with_capacity(2)
                .push(widget::text::caption(fl!("logon-setting")))
                .push(widget::dropdown(
                    &self.logon_labels,
                    Some(d.logon),
                    Message::LogonSelected,
                ))
                .spacing(space.space_xxs),
        );
        if d.logon == LOGON_CUSTOM {
            controls = controls.push(
                widget::text_input(fl!("domain-placeholder"), d.custom_domain.as_str())
                    .on_input(Message::CustomDomainInput),
            );
        }
        if let Some(e) = &d.error {
            controls = controls.push(widget::text::body(e.clone()));
        }
        controls = controls.push(widget::text::caption(fl!("password-explainer")));

        Some(
            widget::dialog()
                .title(fl!("connect-to", title = d.title.clone()))
                .control(controls)
                .primary_action(
                    widget::button::suggested(fl!("connect")).on_press(Message::DialogConnect),
                )
                .secondary_action(
                    widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                )
                .into(),
        )
    }

    fn view(&self) -> Element<'_, Self::Message> {
        match &self.session {
            Session::SignedOut { error } => signed_out_view(error.as_deref()),
            Session::SigningIn { .. } => centered(
                widget::column::with_capacity(3)
                    .push(widget::text::title3(fl!("signing-in")))
                    .push(widget::text::body(fl!("signing-in-hint")))
                    .push(widget::button::standard(fl!("cancel")).on_press(Message::CancelSignIn))
                    .spacing(cosmic::theme::spacing().space_s)
                    .align_x(Alignment::Center)
                    .into(),
            ),
            Session::Ready {
                loading,
                workspaces,
                error,
                ..
            } => resources_view(*loading, workspaces, error.as_deref(), &self.connections),
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        self.core()
            .watch_config::<Config>(Self::APP_ID)
            .map(|update| Message::UpdateConfig(update.config))
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::ToggleContextPage(page) => {
                if self.context_page == page {
                    self.core.window.show_context = !self.core.window.show_context;
                } else {
                    self.context_page = page;
                    self.core.window.show_context = true;
                }
            }
            Message::UpdateConfig(config) => self.config = config,
            Message::LaunchUrl(url) => {
                if let Err(err) = open::that_detached(&url) {
                    eprintln!("failed to open link: {err}");
                }
            }
            Message::SignIn => return self.start_sign_in(),
            Message::CancelSignIn => {
                if let Session::SigningIn { cancel } = &self.session {
                    cancel.cancel();
                }
                self.generation += 1;
                self.session = Session::SignedOut { error: None };
            }
            Message::SignInFinished(generation, result) => {
                if generation != self.generation {
                    return Task::none();
                }
                match result {
                    Ok(account) => {
                        self.session = Session::Ready {
                            account,
                            loading: true,
                            workspaces: Vec::new(),
                            error: None,
                        };
                        return self.load_resources();
                    }
                    Err(SignInFailure(error)) => self.session = Session::SignedOut { error },
                }
            }
            Message::Refresh => return self.load_resources(),
            Message::Discovered(generation, result) => self.discovered(generation, result),
            Message::SignOut => self.sign_out(),
            Message::Connect(key) => return self.prepare_connection(&key),
            Message::ConnectionFileReady(generation, key, result) => {
                if generation == self.generation {
                    return self.connection_file_ready(key, result);
                }
            }
            Message::PasswordInput(PasswordText(s)) => {
                if let Some(d) = &mut self.dialog {
                    d.password = s;
                }
            }
            Message::TogglePasswordVisible => {
                if let Some(d) = &mut self.dialog {
                    d.password_hidden = !d.password_hidden;
                }
            }
            Message::LogonSelected(i) => {
                if let Some(d) = &mut self.dialog {
                    d.logon = i;
                    d.error = None;
                }
            }
            Message::CustomDomainInput(s) => {
                if let Some(d) = &mut self.dialog {
                    d.custom_domain = s;
                }
            }
            Message::DialogCancel => {
                if let Some(d) = self.dialog.take() {
                    self.connections.remove(&d.key);
                }
            }
            Message::DialogConnect => return self.dialog_connect(),
            Message::Session(generation, key, event) => {
                if generation == self.generation {
                    self.session_event(&key, event);
                }
            }
            Message::Disconnect(key) => {
                if let Some(s) = self.running.get(&key) {
                    s.disconnect();
                }
            }
        }
        Task::none()
    }
}

impl AppModel {
    pub fn update_title(&mut self) -> Task<cosmic::Action<Message>> {
        let title = fl!("app-title");
        match self.core.main_window_id() {
            Some(id) => self.set_window_title(title, id),
            None => Task::none(),
        }
    }

    fn account(&self) -> Option<Arc<SignedIn>> {
        match &self.session {
            Session::Ready { account, .. } => Some(account.clone()),
            _ => None,
        }
    }

    fn start_sign_in(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(helper) = signin::find_helper() else {
            self.session = Session::SignedOut {
                error: Some(fl!("error-helper-missing")),
            };
            return Task::none();
        };
        let Some(profile) = signin_profile_dir() else {
            self.session = Session::SignedOut {
                error: Some(fl!("error-no-data-dir")),
            };
            return Task::none();
        };
        self.generation += 1;
        let generation = self.generation;
        let cancel = Cancel::default();
        self.session = Session::SigningIn {
            cancel: cancel.clone(),
        };
        let http = self.http.clone();
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                signin::sign_in(
                    &http,
                    &ClientConfig::public_remote_desktop(),
                    &helper,
                    &profile,
                    None,
                    &cancel,
                )
            })
            .await
            .unwrap_or(Err(SignInError::HelperFailed("task".into())));
            Message::SignInFinished(
                generation,
                result.map(Arc::new).map_err(describe_sign_in_error),
            )
        })
    }

    fn load_resources(&mut self) -> Task<cosmic::Action<Message>> {
        let Session::Ready {
            account, loading, ..
        } = &mut self.session
        else {
            return Task::none();
        };
        *loading = true;
        let generation = self.generation;
        let http = self.http.clone();
        let account = account.clone();
        cosmic::task::future(async move {
            let acc = account.clone();
            let result = tokio::task::spawn_blocking(move || {
                discovery::discover(&http, &acc.tokens.access_token)
            })
            .await;
            let result = match result {
                Ok(Ok(feeds)) => Ok(feeds
                    .into_iter()
                    .map(|f| workspace_view(&account, f))
                    .collect()),
                Ok(Err(e)) => Err(describe_fetch_error(&e)),
                Err(_) => Err(fl!("error-generic")),
            };
            Message::Discovered(generation, result)
        })
    }

    fn discovered(&mut self, generation: u64, result: Result<Vec<WorkspaceView>, String>) {
        if generation != self.generation {
            return;
        }
        if let Session::Ready {
            loading,
            workspaces,
            error,
            ..
        } = &mut self.session
        {
            *loading = false;
            match result {
                Ok(list) => {
                    *workspaces = list;
                    *error = None;
                }
                Err(e) => *error = Some(e),
            }
        }
    }

    fn sign_out(&mut self) {
        // Invalidate everything in flight, end sessions, then forget the
        // account and its sign-in cookies (spec §6).
        self.generation += 1;
        if let Session::SigningIn { cancel } = &self.session {
            cancel.cancel();
        }
        for s in self.running.values() {
            s.disconnect();
        }
        self.running.clear();
        self.connections.clear();
        self.pending_logon.clear();
        self.dialog = None;
        let mut error = None;
        if let Some(dir) = signin_profile_dir()
            && dir.exists()
            && std::fs::remove_dir_all(&dir).is_err()
        {
            error = Some(fl!("error-sign-out-incomplete"));
        }
        self.session = Session::SignedOut { error };
    }

    fn find_resource(&self, key: &str) -> Option<ResourceView> {
        let Session::Ready { workspaces, .. } = &self.session else {
            return None;
        };
        workspaces
            .iter()
            .flat_map(|w| &w.resources)
            .find(|r| r.key == key)
            .cloned()
    }

    /// Step 1 of connecting: download and filter the connection file.
    fn prepare_connection(&mut self, key: &str) -> Task<cosmic::Action<Message>> {
        if self.running.contains_key(key)
            || matches!(
                self.connections.get(key),
                Some(Connection::Preparing | Connection::Connecting)
            )
        {
            return Task::none(); // no accidental duplicate launches
        }
        let (Some(account), Some(resource)) = (self.account(), self.find_resource(key)) else {
            return Task::none();
        };
        let Some(url) = resource.connection_file.clone() else {
            self.connections.insert(
                key.to_owned(),
                Connection::Failed(fl!("error-no-connection-file")),
            );
            return Task::none();
        };
        self.connections
            .insert(key.to_owned(), Connection::Preparing);
        let generation = self.generation;
        let http = self.http.clone();
        let key = key.to_owned();
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                let resp = http
                    .get(
                        &url,
                        Role::ConnectionFile,
                        Some(&account.tokens.access_token),
                    )
                    .map_err(|e| describe_fetch_error(&FetchError::Http(e)))?;
                ConnectionFile::parse(&resp.body)
                    .map(Arc::new)
                    .map_err(|e| fl!("error-bad-connection-file", detail = format!("{e:?}")))
            })
            .await
            .unwrap_or_else(|_| Err(fl!("error-generic")));
            Message::ConnectionFileReady(generation, key, result)
        })
    }

    /// Step 2: Entra single sign-on hosts connect at once; others ask for
    /// the Windows password.
    fn connection_file_ready(
        &mut self,
        key: String,
        result: Result<Arc<ConnectionFile>, String>,
    ) -> Task<cosmic::Action<Message>> {
        let connection = match result {
            Ok(c) => c,
            Err(e) => {
                self.connections.insert(key, Connection::Failed(e));
                return Task::none();
            }
        };
        let remembered = self
            .config
            .logon_settings
            .get(&key)
            .cloned()
            .unwrap_or_default();
        if connection.entra_sso() || remembered == LogonSetting::EntraSignIn {
            return self.launch(&key, &connection, LogonSetting::EntraSignIn, None);
        }
        let title = self
            .find_resource(&key)
            .map(|r| r.title)
            .unwrap_or_default();
        let (logon, custom_domain) = logon_index(&remembered);
        self.dialog = Some(PasswordDialog {
            key,
            title,
            connection,
            password: String::new(),
            password_hidden: true,
            logon,
            custom_domain,
            error: None,
        });
        Task::none()
    }

    fn dialog_connect(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(d) = &mut self.dialog else {
            return Task::none();
        };
        let logon = match d.logon {
            LOGON_EMPTY => LogonSetting::EmptyDomain,
            LOGON_CUSTOM => {
                let domain = d.custom_domain.trim().to_owned();
                if !valid_domain(&domain) {
                    d.error = Some(fl!("error-bad-domain"));
                    return Task::none();
                }
                LogonSetting::CustomDomain(domain)
            }
            LOGON_ENTRA => LogonSetting::EntraSignIn,
            _ => LogonSetting::Automatic,
        };
        let password = if logon.needs_password() {
            if d.password.is_empty() {
                d.error = Some(fl!("error-password-empty"));
                return Task::none();
            }
            if d.password.contains(['\n', '\r', '\0']) {
                d.error = Some(fl!("error-password-chars"));
                return Task::none();
            }
            Some(Secret::new(std::mem::take(&mut d.password)))
        } else {
            None
        };
        let d = self.dialog.take().expect("checked above");
        self.launch(&d.key, &d.connection, logon, password)
    }

    /// Step 3: start FreeRDP and forward its events to the app.
    fn launch(
        &mut self,
        key: &str,
        connection: &Arc<ConnectionFile>,
        logon: LogonSetting,
        password: Option<Secret>,
    ) -> Task<cosmic::Action<Message>> {
        let (Some(account), Some(helper), Some(profile)) =
            (self.account(), signin::find_helper(), signin_profile_dir())
        else {
            self.connections.insert(
                key.to_owned(),
                Connection::Failed(fl!("error-helper-missing")),
            );
            return Task::none();
        };
        let spec = LaunchSpec {
            connection: (**connection).clone(),
            username: account.identity.username.clone(),
            password,
            logon: logon.clone(),
            helper,
            profile,
        };
        let (tx, rx) = mpsc::unbounded();
        let started = launcher::launch(&spec, move |e| {
            let _ = tx.unbounded_send(e);
        });
        match started {
            Ok(session) => {
                self.running.insert(key.to_owned(), session);
                self.connections
                    .insert(key.to_owned(), Connection::Connecting);
                self.pending_logon
                    .insert(key.to_owned(), (logon, connection.clone()));
                let generation = self.generation;
                let key = key.to_owned();
                cosmic::task::stream(rx.map(move |e| Message::Session(generation, key.clone(), e)))
            }
            Err(e) => {
                let text = match e {
                    launcher::LaunchError::FreeRdpMissing => fl!("error-freerdp-missing"),
                    launcher::LaunchError::BadPassword => fl!("error-password-chars"),
                    other => fl!("error-launch", detail = format!("{other:?}")),
                };
                self.connections
                    .insert(key.to_owned(), Connection::Failed(text));
                Task::none()
            }
        }
    }

    fn session_event(&mut self, key: &str, event: SessionEvent) {
        match event {
            SessionEvent::Connected => {
                self.connections
                    .insert(key.to_owned(), Connection::Connected);
                // Remember the logon setting that worked.
                if let Some((logon, _)) = self.pending_logon.get(key).cloned() {
                    self.remember_logon(key, logon);
                }
            }
            SessionEvent::Ended { end, log } => {
                self.running.remove(key);
                let pending = self.pending_logon.remove(key);
                if !log.is_empty() && end != SessionEnd::Closed {
                    eprintln!("FreeRDP ended ({end:?}); last log lines:");
                    for line in &log {
                        eprintln!("  {line}");
                    }
                }
                match end {
                    SessionEnd::Closed | SessionEnd::Cancelled => {
                        self.connections.remove(key);
                    }
                    SessionEnd::LogonFailed => {
                        self.connections.remove(key);
                        if let Some((logon, connection)) = pending {
                            self.reopen_after_logon_failure(key, &logon, connection);
                        }
                    }
                    other => {
                        self.connections.insert(
                            key.to_owned(),
                            Connection::Failed(describe_session_end(&other)),
                        );
                    }
                }
            }
        }
    }

    /// Logon failed: ask again, explain, and suggest another setting
    /// without switching on our own (ADR 0005).
    fn reopen_after_logon_failure(
        &mut self,
        key: &str,
        logon: &LogonSetting,
        connection: Arc<ConnectionFile>,
    ) {
        let (index, custom_domain) = logon_index(logon);
        let tried = self.logon_labels[index].clone();
        let title = self.find_resource(key).map(|r| r.title).unwrap_or_default();
        self.dialog = Some(PasswordDialog {
            key: key.to_owned(),
            title,
            connection,
            password: String::new(),
            password_hidden: true,
            logon: index,
            custom_domain,
            error: Some(fl!("error-logon-failed", setting = tried)),
        });
    }

    fn remember_logon(&mut self, key: &str, logon: LogonSetting) {
        if self.config.logon_settings.get(key) == Some(&logon) {
            return;
        }
        let mut map = self.config.logon_settings.clone();
        map.insert(key.to_owned(), logon);
        if let Some(handler) = &self.config_handler {
            let _ = self.config.set_logon_settings(handler, map);
        }
    }
}

/// The sign-in web view's private directory for the (single) account.
fn signin_profile_dir() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("cosmic-ext-avd")
            .join("signin")
            .join("default"),
    )
}

/// Opaque key for a resource: hash of account, tenant and resource ID.
fn resource_key(account: &SignedIn, tenant: &str, resource_id: &str) -> String {
    let mut h = sha2::Sha256::new();
    for part in [
        account.identity.tenant_id.as_str(),
        account.identity.object_id.as_str(),
        tenant,
        resource_id,
    ] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&h.finalize()[..18])
}

fn valid_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 255
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn workspace_view(account: &SignedIn, feed: discovery::WorkspaceFeed) -> WorkspaceView {
    let fallback = feed
        .tenant
        .workspace_name
        .clone()
        .or_else(|| feed.tenant.tenant_display_name.clone())
        .unwrap_or_else(|| fl!("unnamed-workspace"));
    let tenant = feed.tenant.tenant_id.clone();
    match feed.result {
        Ok(ws) => WorkspaceView {
            name: if ws.name.is_empty() {
                fallback
            } else {
                ws.name
            },
            error: None,
            resources: ws
                .resources
                .into_iter()
                .filter(|r| r.show_by_default)
                .map(|r| ResourceView {
                    key: resource_key(account, &tenant, &r.id),
                    title: r.title,
                    kind: r.kind,
                    connection_file: r.connection_file,
                })
                .collect(),
        },
        Err(e) => WorkspaceView {
            name: fallback,
            error: Some(describe_fetch_error(&e)),
            resources: Vec::new(),
        },
    }
}

fn describe_sign_in_error(e: SignInError) -> SignInFailure {
    let text = match e {
        SignInError::Cancelled => return SignInFailure(None),
        SignInError::HelperMissing => fl!("error-helper-missing"),
        SignInError::Timeout => fl!("error-sign-in-timeout"),
        SignInError::Provider { error, aadsts } => match error.as_str() {
            "access_denied" => access_denied(aadsts),
            "consent_required" => fl!("error-consent-required"),
            _ => fl!("error-provider", code = error),
        },
        SignInError::Token(TokenError::Denied { aadsts }) => {
            access_denied(aadsts.map(|c| c.to_string()))
        }
        SignInError::Token(TokenError::ConsentRequired { .. }) => fl!("error-consent-required"),
        SignInError::Token(TokenError::Transient(_)) => fl!("error-network"),
        SignInError::HelperFailed(reason) if reason == "tls_error" => fl!("error-tls"),
        SignInError::HelperFailed(reason) if reason == "no_display" => fl!("error-no-display"),
        other => fl!("error-sign-in-generic", detail = format!("{other:?}")),
    };
    SignInFailure(Some(text))
}

fn access_denied(code: Option<String>) -> String {
    match code.filter(|c| !c.is_empty()) {
        Some(code) => fl!("error-access-denied-code", code = code),
        None => fl!("error-access-denied"),
    }
}

fn describe_fetch_error(e: &FetchError) -> String {
    match e {
        FetchError::Http(HttpError::Status { status: 401, .. }) => fl!("error-session-expired"),
        FetchError::Http(HttpError::Status { status: 403, .. }) => fl!("error-forbidden"),
        FetchError::Http(
            HttpError::HostNotFound | HttpError::ConnectionFailed | HttpError::Timeout,
        ) => {
            fl!("error-network")
        }
        FetchError::Http(HttpError::Tls) => fl!("error-tls"),
        other => fl!("error-feed", detail = other.to_string()),
    }
}

fn describe_session_end(end: &SessionEnd) -> String {
    match end {
        SessionEnd::AccountLockedOut => fl!("error-account-locked"),
        SessionEnd::PasswordExpired => fl!("error-password-expired"),
        SessionEnd::AccountDisabled => fl!("error-account-disabled"),
        SessionEnd::AuthenticationFailed => fl!("error-auth-failed"),
        SessionEnd::DnsFailed => fl!("error-network"),
        SessionEnd::TlsFailed => fl!("error-tls"),
        SessionEnd::Crashed => fl!("error-freerdp-crashed"),
        SessionEnd::Failed { code } => fl!("error-session-failed", code = code.to_string()),
        SessionEnd::Closed | SessionEnd::Cancelled | SessionEnd::LogonFailed => String::new(),
    }
}

fn centered(content: Element<'_, Message>) -> Element<'_, Message> {
    widget::container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center)
        .into()
}

fn signed_out_view(error: Option<&str>) -> Element<'_, Message> {
    let space = cosmic::theme::spacing();
    let mut col = widget::column::with_capacity(5)
        .push(widget::text::title1(fl!("app-title")))
        .push(widget::text::body(fl!("unofficial-notice")).width(Length::Fixed(420.0)))
        .push(widget::button::suggested(fl!("sign-in")).on_press(Message::SignIn))
        .spacing(space.space_m)
        .align_x(Alignment::Center);
    if let Some(error) = error {
        col = col.push(widget::text::body(error.to_owned()).width(Length::Fixed(420.0)));
    }
    centered(col.into())
}

fn resource_row<'a>(r: &'a ResourceView, conn: Option<&'a Connection>) -> Element<'a, Message> {
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
    let description = match status {
        Some(s) => format!("{kind} · {s}"),
        None => kind,
    };
    widget::settings::item::builder(r.title.clone())
        .description(description)
        .control(control)
        .into()
}

fn resources_view<'a>(
    loading: bool,
    workspaces: &'a [WorkspaceView],
    error: Option<&'a str>,
    connections: &'a HashMap<String, Connection>,
) -> Element<'a, Message> {
    let space = cosmic::theme::spacing();
    let mut col = widget::column::with_capacity(workspaces.len() + 3).spacing(space.space_m);
    if loading {
        col = col.push(widget::text::body(fl!("loading-desktops")));
    }
    if let Some(error) = error {
        col = col.push(widget::text::body(error.to_owned()));
    }
    if !loading
        && error.is_none()
        && workspaces
            .iter()
            .all(|w| w.resources.is_empty() && w.error.is_none())
    {
        col = col.push(widget::text::body(fl!("no-desktops")));
    }
    for ws in workspaces {
        let mut section = widget::settings::section().title(ws.name.clone());
        if let Some(e) = &ws.error {
            section = section.add(widget::text::body(fl!(
                "workspace-failed",
                detail = e.clone()
            )));
        }
        for r in &ws.resources {
            section = section.add(resource_row(r, connections.get(&r.key)));
        }
        col = col.push(section);
    }
    widget::scrollable(
        widget::container(col.max_width(800))
            .width(Length::Fill)
            .align_x(Horizontal::Center)
            .padding(space.space_m),
    )
    .into()
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
        }
    }
}
