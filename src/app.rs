// SPDX-License-Identifier: MIT

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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
use crate::auth::id_token;
use crate::auth::signin::{self, Cancel, SignInError, SignedIn};
use crate::auth::token::{self, Secret, TokenError};
use crate::config::Config;
use crate::deps::{self, Check, Dependency, Status};
use crate::fl;
use crate::secrets::{self, SecretsError};
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
    /// Why the last attempt for a resource failed, shown at the top of the
    /// next password dialog.
    last_failure: HashMap<String, String>,
    /// Resources where Entra sign-in failed in this run: the next Connect
    /// asks for a password instead of trying it again.
    entra_failed: HashSet<String>,
    logon_labels: Vec<String>,
    /// Shown above the desktop list, for example when the keyring couldn't
    /// save the sign-in.
    notice: Option<String>,
    /// The saved sign-in couldn't be renewed for a temporary reason; the
    /// signed-out view offers to try again.
    can_retry_restore: bool,
    /// Last dependency check; `None` while it runs.
    deps: Option<Vec<Check>>,
}

enum Session {
    SignedOut {
        error: Option<String>,
    },
    /// Signing in with the refresh token saved in the keyring.
    Restoring,
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
    /// The connection file, and the account with renewed tokens if they
    /// had to be renewed first.
    ConnectionFileReady(
        u64,
        String,
        Result<(Arc<ConnectionFile>, Arc<SignedIn>), String>,
    ),
    PasswordInput(PasswordText),
    TogglePasswordVisible,
    LogonSelected(usize),
    CustomDomainInput(String),
    DialogConnect,
    DialogCancel,
    Session(u64, String, SessionEvent),
    Disconnect(String),
    Restored(u64, Restore),
    RetryRestore,
    /// The keyring saved (or failed to save) the sign-in.
    Saved(u64, Result<(), String>),
    SignOutFinished(u64, Result<(), String>),
    CheckDependencies,
    DependenciesChecked(Vec<Check>),
}

/// The outcome of signing in with the saved refresh token.
#[derive(Debug, Clone)]
pub enum Restore {
    NothingSaved,
    /// Signed in. The text is a notice if the renewed token couldn't be
    /// saved.
    Ready(Arc<SignedIn>, Option<String>),
    /// The saved sign-in can't be used any more; sign in again.
    SignInAgain(String),
    /// Network trouble; the saved sign-in is kept for another try.
    Retry(String),
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
            last_failure: HashMap::new(),
            entra_failed: HashSet::new(),
            notice: None,
            can_retry_restore: false,
            deps: None,
            logon_labels: vec![
                fl!("logon-automatic"),
                fl!("logon-empty-domain"),
                fl!("logon-custom-domain"),
                fl!("logon-entra"),
            ],
        };
        let title = app.update_title();
        let restore = app.start_restore();
        let deps = app.check_dependencies();
        (app, Task::batch([title, restore, deps]))
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        let menu_bar = menu::bar(vec![menu::Tree::with_children(
            menu::root(fl!("view")).apply(Element::from),
            menu::items(
                &self.key_binds,
                vec![
                    menu::Item::Button(fl!("dependencies"), None, MenuAction::Dependencies),
                    menu::Item::Button(fl!("about"), None, MenuAction::About),
                ],
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
            ContextPage::Dependencies => context_drawer::context_drawer(
                dependencies_view(self.deps.as_deref()),
                Message::ToggleContextPage(ContextPage::Dependencies),
            )
            .title(fl!("dependencies"))
            .actions({
                let mut b = widget::button::standard(fl!("check-again"));
                if self.deps.is_some() {
                    b = b.on_press(Message::CheckDependencies);
                }
                b
            }),
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
        if let Some(e) = &d.error {
            controls = controls.push(widget::text::title4(e.clone()));
        }
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
        let main = self.session_view();
        // Missing required dependencies: a warning above everything, with
        // the details in the context drawer.
        match &self.deps {
            Some(checks) if !deps::required_ok(checks) => widget::column::with_capacity(2)
                .push(
                    widget::warning(fl!("deps-missing"))
                        .on_close(Message::ToggleContextPage(ContextPage::Dependencies)),
                )
                .push(main)
                .spacing(cosmic::theme::spacing().space_s)
                .into(),
            _ => main,
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        self.core()
            .watch_config::<Config>(Self::APP_ID)
            .map(|update| Message::UpdateConfig(update.config))
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        self.update_main(message)
    }
}

impl AppModel {
    fn check_dependencies(&mut self) -> Task<cosmic::Action<Message>> {
        self.deps = None;
        cosmic::task::future(async move {
            let checks = tokio::task::spawn_blocking(deps::check_all)
                .await
                .unwrap_or_default();
            Message::DependenciesChecked(checks)
        })
    }

    fn session_view(&self) -> Element<'_, Message> {
        match &self.session {
            Session::SignedOut { error } => {
                signed_out_view(error.as_deref(), self.can_retry_restore)
            }
            Session::Restoring => centered(
                widget::column::with_capacity(2)
                    .push(widget::text::title3(fl!("restoring")))
                    .push(widget::text::body(fl!("restoring-hint")))
                    .spacing(cosmic::theme::spacing().space_s)
                    .align_x(Alignment::Center)
                    .into(),
            ),
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
            } => resources_view(
                *loading,
                workspaces,
                error.as_deref(),
                self.notice.as_deref(),
                &self.connections,
            ),
        }
    }

    fn update_main(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::CheckDependencies => return self.check_dependencies(),
            Message::DependenciesChecked(checks) => {
                for c in &checks {
                    if !c.status.is_ok() {
                        eprintln!("dependency {:?}: {:?}", c.dependency, c.status);
                    }
                }
                self.deps = Some(checks);
            }
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
                        let save = self.save_sign_in(&account);
                        self.session = Session::Ready {
                            account,
                            loading: true,
                            workspaces: Vec::new(),
                            error: None,
                        };
                        return Task::batch([save, self.load_resources()]);
                    }
                    Err(SignInFailure(error)) => self.session = Session::SignedOut { error },
                }
            }
            Message::Refresh => return self.load_resources(),
            Message::Discovered(generation, result) => self.discovered(generation, result),
            Message::SignOut => return self.sign_out(),
            Message::Restored(generation, outcome) => {
                if generation == self.generation {
                    return self.restored(outcome);
                }
            }
            Message::RetryRestore => return self.start_restore(),
            Message::Saved(generation, result) => {
                if generation == self.generation {
                    self.notice = result.err();
                }
            }
            Message::SignOutFinished(generation, result) => {
                if generation == self.generation
                    && let (Session::SignedOut { error }, Err(e)) = (&mut self.session, result)
                {
                    *error = Some(e);
                }
            }
            Message::Connect(key) => return self.prepare_connection(&key),
            Message::ConnectionFileReady(generation, key, result) => {
                if generation == self.generation {
                    let result = result.map(|(connection, renewed)| {
                        if let Session::Ready { account, .. } = &mut self.session
                            && !Arc::ptr_eq(account, &renewed)
                        {
                            *account = renewed;
                        }
                        connection
                    });
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

    fn sign_out(&mut self) -> Task<cosmic::Action<Message>> {
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
        self.last_failure.clear();
        self.entra_failed.clear();
        self.dialog = None;
        self.notice = None;
        self.can_retry_restore = false;
        let mut error = None;
        for dir in [signin_profile_dir(), signin_session_dir()]
            .into_iter()
            .flatten()
        {
            if dir.exists() && std::fs::remove_dir_all(&dir).is_err() {
                error = Some(fl!("error-sign-out-incomplete"));
            }
        }
        self.session = Session::SignedOut { error };
        // The keyring may block (for example on an unlock prompt), so not on
        // the UI thread.
        SIGN_OUTS.fetch_add(1, Ordering::SeqCst);
        let generation = self.generation;
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(|| {
                let _renewal = RENEWAL.lock();
                let deleted = secrets::delete_all(&ClientConfig::public_remote_desktop());
                eprintln!("saved sign-in: deleted (sign-out): {deleted:?}");
                deleted
            })
            .await;
            let result = match result {
                Ok(Ok(())) => Ok(()),
                _ => Err(fl!("error-sign-out-incomplete")),
            };
            Message::SignOutFinished(generation, result)
        })
    }

    /// Signs in with the refresh token from the keyring, if there is one.
    fn start_restore(&mut self) -> Task<cosmic::Action<Message>> {
        self.generation += 1;
        let generation = self.generation;
        self.session = Session::Restoring;
        self.can_retry_restore = false;
        let http = self.http.clone();
        cosmic::task::future(async move {
            let outcome = tokio::task::spawn_blocking(move || restore(&http))
                .await
                .unwrap_or_else(|_| Restore::Retry(fl!("error-generic")));
            Message::Restored(generation, outcome)
        })
    }

    fn restored(&mut self, outcome: Restore) -> Task<cosmic::Action<Message>> {
        match outcome {
            Restore::Ready(account, notice) => {
                self.notice = notice;
                self.session = Session::Ready {
                    account,
                    loading: true,
                    workspaces: Vec::new(),
                    error: None,
                };
                self.load_resources()
            }
            Restore::NothingSaved => {
                self.session = Session::SignedOut { error: None };
                Task::none()
            }
            Restore::SignInAgain(error) => {
                self.session = Session::SignedOut { error: Some(error) };
                Task::none()
            }
            Restore::Retry(error) => {
                self.can_retry_restore = true;
                self.session = Session::SignedOut { error: Some(error) };
                Task::none()
            }
        }
    }

    /// Saves a new sign-in's refresh token in the keyring.
    fn save_sign_in(&mut self, account: &Arc<SignedIn>) -> Task<cosmic::Action<Message>> {
        self.notice = None;
        self.can_retry_restore = false;
        let generation = self.generation;
        let account = account.clone();
        let started = sign_out_count();
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                let _renewal = RENEWAL.lock();
                save_refresh_token(&account, started)
            })
            .await
            .unwrap_or_else(|_| Err(fl!("error-generic")));
            Message::Saved(generation, result)
        })
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
                // The access token also goes to FreeRDP, so it must stay
                // valid through the connection.
                let account = renew_if_needed(&http, account)?;
                let resp = http
                    .get(
                        &url,
                        Role::ConnectionFile,
                        Some(&account.tokens.access_token),
                    )
                    .map_err(|e| describe_fetch_error(&FetchError::Http(e)))?;
                ConnectionFile::parse(&resp.body)
                    .map(|c| (Arc::new(c), account))
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
        let remembered = self.config.logon_settings.get(&key).cloned();
        // Entra sign-in first: it needs no password, so trying it can't
        // count toward an account lockout. Password logon only when the
        // user chose it, or after Entra sign-in failed for this desktop.
        let entra_first =
            connection.entra_sso() || matches!(remembered, None | Some(LogonSetting::EntraSignIn));
        if entra_first && !self.entra_failed.contains(&key) {
            return self.launch(&key, &connection, LogonSetting::EntraSignIn, None);
        }
        let remembered = match remembered {
            Some(LogonSetting::EntraSignIn) | None => LogonSetting::Automatic,
            Some(other) => other,
        };
        let title = self
            .find_resource(&key)
            .map(|r| r.title)
            .unwrap_or_default();
        let (logon, custom_domain) = logon_index(&remembered);
        let error = self.last_failure.remove(&key);
        self.dialog = Some(PasswordDialog {
            key,
            title,
            connection,
            password: String::new(),
            password_hidden: true,
            logon,
            custom_domain,
            error,
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
            gateway_token: (!account.tokens.needs_refresh(Instant::now()))
                .then(|| account.tokens.access_token.clone()),
        };
        // No secrets here: the setting and, for a custom domain, its name.
        eprintln!("Starting FreeRDP with logon setting {logon:?}");
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
                self.entra_failed.remove(key);
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
                // Only a session that never got connected says anything
                // about the logon method; later errors are ordinary ones.
                let was_connected =
                    matches!(self.connections.get(key), Some(Connection::Connected));
                // Once Entra sign-in has worked for a desktop, a failure is
                // more likely temporary; keep using it.
                let worked_before =
                    self.config.logon_settings.get(key) == Some(&LogonSetting::EntraSignIn);
                let was_entra = !was_connected
                    && !worked_before
                    && matches!(pending, Some((LogonSetting::EntraSignIn, _)));
                match end {
                    SessionEnd::Closed | SessionEnd::Cancelled => {
                        self.connections.remove(key);
                    }
                    ref other if was_entra => {
                        // Offer password logon next time, and say why.
                        self.entra_failed.insert(key.to_owned());
                        self.connections.insert(
                            key.to_owned(),
                            Connection::Failed(fl!("status-entra-failed")),
                        );
                        self.last_failure.insert(
                            key.to_owned(),
                            fl!("error-entra-failed", detail = describe_session_end(other)),
                        );
                    }
                    SessionEnd::LogonFailed => {
                        // Stop and explain; the user decides whether to try
                        // again (ADR 0005). No automatic retry or re-prompt.
                        let tried = pending
                            .map(|(logon, _)| self.logon_labels[logon_index(&logon).0].clone())
                            .unwrap_or_default();
                        let message = fl!("error-logon-failed", setting = tried);
                        self.connections.insert(
                            key.to_owned(),
                            Connection::Failed(fl!("status-logon-failed")),
                        );
                        self.last_failure.insert(key.to_owned(), message);
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

/// One token renewal (or keyring write) at a time (spec §6: serialize
/// refresh per account; the app has one account).
static RENEWAL: Mutex<()> = Mutex::new(());

/// Counts sign-outs. Work that started before a sign-out must not save its
/// tokens afterwards (spec §6: late callbacks must not restore the account).
static SIGN_OUTS: AtomicU64 = AtomicU64::new(0);

fn sign_out_count() -> u64 {
    SIGN_OUTS.load(Ordering::SeqCst)
}

/// Saves the account's refresh token, unless the user signed out since
/// `started` (a [`sign_out_count`]). The error is the notice to show.
/// Blocking; the caller holds [`RENEWAL`], as sign-out's delete does.
fn save_refresh_token(account: &SignedIn, started: u64) -> Result<(), String> {
    if sign_out_count() != started {
        eprintln!("saved sign-in: not stored (signed out meanwhile)");
        return Ok(());
    }
    let Some(refresh_token) = &account.tokens.refresh_token else {
        eprintln!("saved sign-in: not stored (no refresh token in the response)");
        return Err(fl!("notice-not-saved", detail = "no refresh token"));
    };
    let result = secrets::save(
        &ClientConfig::public_remote_desktop(),
        &account.identity,
        refresh_token,
    );
    // Diagnostics: outcome and error kind only, never tokens or IDs.
    match &result {
        Ok(()) => eprintln!("saved sign-in: stored"),
        Err(e) => eprintln!("saved sign-in: not stored ({e})"),
    }
    result.map_err(|e| fl!("notice-not-saved", detail = keyring_detail(&e)))
}

fn keyring_detail(e: &SecretsError) -> String {
    match e {
        SecretsError::Dismissed => fl!("keyring-dismissed"),
        SecretsError::Malformed => fl!("keyring-malformed"),
        SecretsError::Unavailable(_) => fl!("keyring-unavailable"),
    }
}

/// The kind of a token error, for diagnostics (no server text).
fn token_error_kind(e: &TokenError) -> String {
    match e {
        TokenError::Transient(_) => "network".into(),
        TokenError::InvalidGrant { aadsts } => format!("invalid_grant, AADSTS {aadsts:?}"),
        TokenError::InteractionRequired { aadsts } => {
            format!("interaction_required, AADSTS {aadsts:?}")
        }
        TokenError::ConsentRequired { aadsts } => format!("consent_required, AADSTS {aadsts:?}"),
        TokenError::Denied { aadsts } => format!("denied by policy, AADSTS {aadsts:?}"),
        TokenError::Other { error, aadsts } => format!("{error}, AADSTS {aadsts:?}"),
        TokenError::Malformed => "malformed response".into(),
    }
}

/// Signs in with the saved refresh token. Blocking.
fn restore(http: &Http) -> Restore {
    let config = ClientConfig::public_remote_desktop();
    let started = sign_out_count();
    let _renewal = RENEWAL.lock();
    let stored = match secrets::load(&config) {
        Ok(Some(stored)) => {
            eprintln!("saved sign-in: found");
            stored
        }
        Ok(None) => {
            eprintln!("saved sign-in: none");
            return Restore::NothingSaved;
        }
        // Spec §6: no files as a fallback. The user signs in for this
        // session; saving will report the keyring problem again.
        Err(e) => {
            eprintln!("saved sign-in: can't load ({e})");
            return Restore::SignInAgain(fl!("error-keyring-load", detail = keyring_detail(&e)));
        }
    };
    let renewed = token::refresh(http, &config, &stored.refresh_token);
    if let Err(e) = &renewed {
        eprintln!("saved sign-in: renewal failed ({})", token_error_kind(e));
    }
    let tokens = match renewed {
        Ok(tokens) => tokens,
        Err(TokenError::Transient(_)) => return Restore::Retry(fl!("error-restore-network")),
        Err(TokenError::InvalidGrant { .. }) => {
            // Revoked or expired for good: forget it.
            let deleted = secrets::delete_all(&config);
            eprintln!("saved sign-in: deleted (invalid_grant): {deleted:?}");
            return Restore::SignInAgain(fl!("error-saved-sign-in-expired"));
        }
        // Policy or interaction needed: an interactive sign-in replaces it.
        Err(_) => return Restore::SignInAgain(fl!("error-saved-sign-in-expired")),
    };
    // A renewed ID token must name the same account (and is checked like
    // any other); without one, the saved identity stands.
    let identity = match tokens.id_token.as_deref() {
        None => stored.identity.clone(),
        Some(raw) => {
            let keys = match id_token::fetch_keys(http) {
                Ok(keys) => keys,
                Err(_) => return Restore::Retry(fl!("error-restore-network")),
            };
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            match id_token::validate_renewed(raw, &keys, &config.client_id, now, &stored.identity) {
                Ok(identity) => identity,
                Err(_) => return Restore::SignInAgain(fl!("error-saved-sign-in-expired")),
            }
        }
    };
    let rotated = tokens
        .refresh_token
        .as_ref()
        .is_some_and(|t| t.expose() != stored.refresh_token.expose());
    eprintln!(
        "saved sign-in: renewed (refresh token {})",
        if rotated { "rotated" } else { "unchanged" }
    );
    let account = SignedIn { identity, tokens };
    let notice = if rotated {
        save_refresh_token(&account, started).err()
    } else {
        None
    };
    Restore::Ready(Arc::new(account), notice)
}

/// Renews the tokens when the access token expires within the margin, and
/// saves a rotated refresh token. Blocking.
fn renew_if_needed(http: &Http, account: Arc<SignedIn>) -> Result<Arc<SignedIn>, String> {
    if !account.tokens.needs_refresh(Instant::now()) {
        return Ok(account);
    }
    let started = sign_out_count();
    let _renewal = RENEWAL.lock();
    let Some(refresh_token) = &account.tokens.refresh_token else {
        return Err(fl!("error-session-expired"));
    };
    match token::refresh(http, &ClientConfig::public_remote_desktop(), refresh_token) {
        Ok(tokens) => {
            let renewed = SignedIn {
                identity: account.identity.clone(),
                tokens,
            };
            let rotated = renewed
                .tokens
                .refresh_token
                .as_ref()
                .is_some_and(|t| t.expose() != refresh_token.expose());
            if rotated {
                // Best effort: if this fails, the next start asks to sign in.
                let _ = save_refresh_token(&renewed, started);
            }
            Ok(Arc::new(renewed))
        }
        Err(TokenError::Transient(_)) => Err(fl!("error-generic")),
        Err(_) => Err(fl!("error-session-expired")),
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

/// The sign-in session cookies the helpers share (RAM-only; see
/// `signin/src/session_jar.rs`).
fn signin_session_dir() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    runtime
        .is_absolute()
        .then(|| runtime.join("cosmic-ext-avd-web"))
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

/// The dependency list, as COSMIC Settings-style sections.
fn dependencies_view(checks: Option<&[Check]>) -> Element<'_, Message> {
    let Some(checks) = checks else {
        return widget::text::body(fl!("deps-checking")).into();
    };
    let section = |required: bool| {
        let mut sec = widget::settings::section().title(if required {
            fl!("deps-required")
        } else {
            fl!("deps-optional")
        });
        for c in checks
            .iter()
            .filter(|c| c.dependency.required() == required)
        {
            let (name, hint) = dependency_text(c.dependency);
            let state = match &c.status {
                Status::Ok(v) if v.is_empty() => fl!("dep-ok"),
                Status::Ok(v) => fl!("dep-ok-version", version = v.clone()),
                Status::Missing => fl!("dep-missing"),
                Status::TooOld(v) => fl!(
                    "dep-too-old",
                    version = v.clone(),
                    min = format!(
                        "{}.{}.{}",
                        deps::MIN_FREERDP.0,
                        deps::MIN_FREERDP.1,
                        deps::MIN_FREERDP.2
                    )
                ),
                Status::Broken(why) => fl!("dep-broken", reason = why.clone()),
            };
            let description = if c.status.is_ok() {
                hint
            } else {
                format!("{state}. {hint}")
            };
            let icon = if c.status.is_ok() {
                "emblem-ok-symbolic"
            } else if required {
                "dialog-error-symbolic"
            } else {
                "dialog-warning-symbolic"
            };
            sec = sec.add(
                widget::settings::item::builder(name)
                    .description(description)
                    .control(
                        widget::row::with_capacity(2)
                            .push(widget::icon::from_name(icon).size(16))
                            .push(widget::text::body(if c.status.is_ok() {
                                state
                            } else {
                                String::new()
                            }))
                            .spacing(cosmic::theme::spacing().space_xxs)
                            .align_y(Alignment::Center),
                    ),
            );
        }
        sec
    };
    widget::settings::view_column(vec![section(true).into(), section(false).into()]).into()
}

/// Name and what to install, per dependency.
fn dependency_text(d: Dependency) -> (String, String) {
    match d {
        Dependency::FreeRdp => (fl!("dep-freerdp"), fl!("dep-freerdp-hint")),
        Dependency::FreeRdpEntra => (fl!("dep-freerdp-entra"), fl!("dep-freerdp-entra-hint")),
        Dependency::SignInHelper => (fl!("dep-signin"), fl!("dep-signin-hint")),
        Dependency::SecretService => (fl!("dep-keyring"), fl!("dep-keyring-hint")),
        Dependency::RuntimeDir => (fl!("dep-runtime"), fl!("dep-runtime-hint")),
    }
}

fn signed_out_view(error: Option<&str>, can_retry: bool) -> Element<'_, Message> {
    let space = cosmic::theme::spacing();
    let mut col = widget::column::with_capacity(6)
        .push(widget::text::title1(fl!("app-title")))
        .push(widget::text::body(fl!("unofficial-notice")).width(Length::Fixed(420.0)))
        .push(widget::button::suggested(fl!("sign-in")).on_press(Message::SignIn))
        .spacing(space.space_m)
        .align_x(Alignment::Center);
    if can_retry {
        col = col.push(widget::button::standard(fl!("try-again")).on_press(Message::RetryRestore));
    }
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
    notice: Option<&'a str>,
    connections: &'a HashMap<String, Connection>,
) -> Element<'a, Message> {
    let space = cosmic::theme::spacing();
    let mut col = widget::column::with_capacity(workspaces.len() + 4).spacing(space.space_m);
    if let Some(notice) = notice {
        col = col.push(widget::text::body(notice.to_owned()));
    }
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
    Dependencies,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
    Dependencies,
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
            MenuAction::Dependencies => Message::ToggleContextPage(ContextPage::Dependencies),
        }
    }
}
