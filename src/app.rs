// SPDX-License-Identifier: MIT

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use base64::Engine as _;
use cosmic::app::context_drawer;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::alignment::{Horizontal, Vertical};
use cosmic::iced::futures::{StreamExt, channel::mpsc};
use cosmic::iced::{Alignment, Length, Subscription};
use cosmic::prelude::*;
use cosmic::widget::{self, about::About, menu, nav_bar};
use sha2::Digest;
use url::Url;

use crate::accounts::{self, Restore};
use crate::auth::attempt::ClientConfig;
use crate::auth::id_token::Identity;
use crate::auth::signin::{self, Cancel, SignInError, SignedIn};
use crate::auth::token::{Secret, TokenError};
use crate::config::Config;
use crate::deps::{self, Check, Dependency, Status};
use crate::fl;
use crate::secrets::Stored;
use crate::service::discovery::{self, FetchError};
use crate::service::endpoint::{EndpointPolicy, Role};
use crate::service::feed::ResourceKind;
use crate::service::http::{Http, HttpError};
use crate::sessions::launcher::{self, LaunchSpec, LogonSetting, SessionEnd, SessionEvent};
use crate::sessions::rdpfile::ConnectionFile;
use crate::settings::{self, ConnectionSettings, Toggle};

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
    /// Accounts in the sidebar, then "Add account".
    nav: nav_bar::Model,
    accounts: Vec<Account>,
    /// Reading saved sign-ins from the keyring at start.
    loading_saved: bool,
    /// Signing in to an account that isn't in the list yet.
    adding: Option<Adding>,
    /// App-wide message, for example when the keyring can't be read.
    notice: Option<String>,
    /// Source of generations and epochs; results tagged with an older value
    /// are stale and ignored (spec §8).
    counter: u64,
    /// Connection state per resource key (keys include the account).
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
    /// Last dependency check; `None` while it runs.
    deps: Option<Vec<Check>>,
    /// Account ID or resource key the settings drawer shows.
    settings_target: Option<String>,
    /// Desktop setting choices when the account's value is on / off.
    follow_on: Vec<String>,
    follow_off: Vec<String>,
    /// "Open on a new workspace" waits per running session; set to stop.
    movers: HashMap<String, Arc<AtomicBool>>,
}

/// A page in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Page {
    Account(String),
    Add,
}

struct Adding {
    generation: u64,
    /// `Some` while the sign-in window is open.
    cancel: Option<Cancel>,
    error: Option<String>,
}

struct Account {
    /// [`accounts::account_id`].
    id: String,
    identity: Identity,
    state: AccountState,
    /// The saved sign-in this account was restored from, for "Try again".
    stored: Option<Stored>,
    /// Shown above the desktop list, for example when the keyring couldn't
    /// save the sign-in.
    notice: Option<String>,
    /// The saved sign-in couldn't be renewed for a temporary reason.
    can_retry: bool,
    /// Changes on every sign-in attempt and refresh of this account.
    generation: u64,
    /// Fixed for the life of this entry; session and connection events
    /// carry it, so events from before a sign-out are ignored.
    epoch: u64,
}

enum AccountState {
    /// Signing in with the refresh token saved in the keyring.
    Restoring,
    NeedsSignIn {
        error: Option<String>,
    },
    SigningIn {
        cancel: Cancel,
    },
    Ready {
        signed_in: Arc<SignedIn>,
        loading: bool,
        workspaces: Vec<WorkspaceView>,
        error: Option<String>,
    },
}

impl Account {
    fn label(&self) -> String {
        self.identity
            .username
            .clone()
            .or_else(|| self.identity.display_name.clone())
            .unwrap_or_else(|| fl!("unnamed-account"))
    }

    fn signed_in(&self) -> Option<&Arc<SignedIn>> {
        match &self.state {
            AccountState::Ready { signed_in, .. } => Some(signed_in),
            _ => None,
        }
    }

    fn resource(&self, key: &str) -> Option<&ResourceView> {
        match &self.state {
            AccountState::Ready { workspaces, .. } => workspaces
                .iter()
                .flat_map(|w| &w.resources)
                .find(|r| r.key == key),
            _ => None,
        }
    }

    fn resource_keys(&self) -> Vec<String> {
        match &self.state {
            AccountState::Ready { workspaces, .. } => workspaces
                .iter()
                .flat_map(|w| &w.resources)
                .map(|r| r.key.clone())
                .collect(),
            _ => Vec::new(),
        }
    }
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
    account: String,
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
    DismissNotice,
    /// The saved sign-ins, or why they couldn't be read.
    SavedLoaded(Result<Vec<Stored>, String>),
    /// Account ID, generation, outcome.
    Restored(String, u64, Restore),
    RetryRestore(String),
    AddAccount,
    /// Sign in again to an account in the list.
    SignInAccount(String),
    /// `None`: the sign-in for a new account.
    CancelSignIn(Option<String>),
    /// Account it was for (`None`: a new one), generation, profile used.
    SignInFinished(
        Option<String>,
        u64,
        PathBuf,
        Result<Arc<SignedIn>, SignInFailure>,
    ),
    /// The keyring saved (or failed to save) the sign-in.
    Saved(String, u64, Result<(), String>),
    Refresh(String),
    Discovered(String, u64, Result<Vec<WorkspaceView>, String>),
    SignOut(String),
    SignOutSelected,
    SignOutFinished(Result<(), String>),
    Connect(String),
    /// Account ID, epoch, resource key, and the connection file with the
    /// account (tokens renewed first if needed).
    ConnectionFileReady(
        String,
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
    /// Account ID, epoch, resource key, event.
    Session(String, u64, String, SessionEvent),
    Disconnect(String),
    CheckDependencies,
    DependenciesChecked(Vec<Check>),
    OpenLogFolder,
    OpenAccountSettings(String),
    OpenDesktopSettings(String),
    AccountToggle(String, Toggle, bool),
    AccountTimeout(String, u32),
    /// Resource key, setting, dropdown choice (0: follow the account).
    DesktopToggle(String, Toggle, usize),
    /// `None`: follow the account.
    DesktopTimeout(String, Option<u32>),
    /// Resource key; outcome of "open on a new workspace" (log text).
    WorkspaceMoved(String, Result<(), String>),
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
        crate::app_log!("AVD Client {} started", env!("CARGO_PKG_VERSION"));
        launcher::clean_stale_runtime_dirs();
        accounts::clean_unfinished_profiles();
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
            nav: nav_bar::Model::default(),
            accounts: Vec::new(),
            loading_saved: true,
            adding: None,
            notice: None,
            counter: 0,
            connections: HashMap::new(),
            running: HashMap::new(),
            pending_logon: HashMap::new(),
            dialog: None,
            last_failure: HashMap::new(),
            entra_failed: HashSet::new(),
            logon_labels: vec![
                fl!("logon-automatic"),
                fl!("logon-empty-domain"),
                fl!("logon-custom-domain"),
                fl!("logon-entra"),
            ],
            deps: None,
            settings_target: None,
            follow_on: vec![fl!("follow-account-on"), fl!("on"), fl!("off")],
            follow_off: vec![fl!("follow-account-off"), fl!("on"), fl!("off")],
            movers: HashMap::new(),
        };
        app.rebuild_nav(&Page::Add);
        let title = app.update_title();
        let load = cosmic::task::future(async {
            let result = tokio::task::spawn_blocking(accounts::load_saved)
                .await
                .unwrap_or_else(|_| Err(fl!("error-generic")));
            Message::SavedLoaded(result)
        });
        let deps = app.check_dependencies();
        (app, Task::batch([title, load, deps]))
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<cosmic::Action<Self::Message>> {
        self.nav.activate(id);
        Task::none()
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        let menu_bar = menu::bar(vec![
            menu::Tree::with_children(
                menu::root(fl!("account-menu")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![
                        menu::Item::Button(fl!("add-account"), None, MenuAction::AddAccount),
                        menu::Item::Button(fl!("sign-out"), None, MenuAction::SignOut),
                    ],
                ),
            ),
            menu::Tree::with_children(
                menu::root(fl!("view")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![
                        menu::Item::Button(fl!("dependencies"), None, MenuAction::Dependencies),
                        menu::Item::Button(fl!("open-log-folder"), None, MenuAction::OpenLogFolder),
                        menu::Item::Button(fl!("about"), None, MenuAction::About),
                    ],
                ),
            ),
        ]);
        vec![menu_bar.into()]
    }

    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        let Some(account) = self.selected_account() else {
            return vec![];
        };
        let AccountState::Ready { loading, .. } = &account.state else {
            return vec![
                widget::button::icon(widget::icon::from_name("preferences-system-symbolic"))
                    .tooltip(fl!("account-settings"))
                    .on_press(Message::OpenAccountSettings(account.id.clone()))
                    .into(),
            ];
        };
        let mut refresh = widget::button::text(fl!("refresh"));
        if !loading {
            refresh = refresh.on_press(Message::Refresh(account.id.clone()));
        }
        vec![
            refresh.into(),
            widget::button::icon(widget::icon::from_name("preferences-system-symbolic"))
                .tooltip(fl!("account-settings"))
                .on_press(Message::OpenAccountSettings(account.id.clone()))
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
            ContextPage::AccountSettings => {
                let id = self.settings_target.clone().unwrap_or_default();
                let title = self.account(&id).map(Account::label).unwrap_or_default();
                context_drawer::context_drawer(
                    self.account_settings_view(&id),
                    Message::ToggleContextPage(ContextPage::AccountSettings),
                )
                .title(title)
                .actions(
                    widget::button::destructive(fl!("sign-out")).on_press(Message::SignOut(id)),
                )
            }
            ContextPage::DesktopSettings => {
                let key = self.settings_target.clone().unwrap_or_default();
                let title = self
                    .account_of_resource(&key)
                    .and_then(|a| a.resource(&key))
                    .map(|r| r.title.clone())
                    .unwrap_or_default();
                context_drawer::context_drawer(
                    self.desktop_settings_view(&key),
                    Message::ToggleContextPage(ContextPage::DesktopSettings),
                )
                .title(title)
            }
        })
    }

    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        let d = self.dialog.as_ref()?;
        let account = self
            .account(&d.account)
            .and_then(|a| a.identity.username.clone())
            .unwrap_or_default();
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
        let space = cosmic::theme::spacing();
        let mut col = widget::column::with_capacity(3).spacing(space.space_s);
        // Missing required dependencies: a warning above everything, with
        // the details in the context drawer.
        if let Some(checks) = &self.deps
            && !deps::required_ok(checks)
        {
            col = col.push(
                widget::warning(fl!("deps-missing"))
                    .on_close(Message::ToggleContextPage(ContextPage::Dependencies)),
            );
        }
        if let Some(notice) = &self.notice {
            col = col.push(widget::warning(notice.clone()).on_close(Message::DismissNotice));
        }
        col.push(self.page_view()).into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        self.core()
            .watch_config::<Config>(Self::APP_ID)
            .map(|update| Message::UpdateConfig(update.config))
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::CheckDependencies => return self.check_dependencies(),
            Message::DependenciesChecked(checks) => {
                for c in &checks {
                    if !c.status.is_ok() {
                        crate::app_log!("dependency {:?}: {:?}", c.dependency, c.status);
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
                    crate::app_log!("failed to open link: {err}");
                }
            }
            Message::DismissNotice => self.notice = None,
            Message::OpenLogFolder => {
                if let Some(dir) = crate::log::dir() {
                    // Make sure it exists even before the first line.
                    crate::app_log!("log folder opened");
                    if let Err(err) = open::that_detached(&dir) {
                        crate::app_log!("failed to open the log folder: {err}");
                    }
                }
            }
            Message::SavedLoaded(result) => return self.saved_loaded(result),
            Message::Restored(id, generation, outcome) => {
                return self.restored(&id, generation, outcome);
            }
            Message::RetryRestore(id) => return self.start_restore(&id),
            Message::AddAccount => return self.start_add(),
            Message::SignInAccount(id) => return self.start_account_sign_in(&id),
            Message::CancelSignIn(None) => {
                if let Some(Adding {
                    cancel: Some(c), ..
                }) = &self.adding
                {
                    c.cancel();
                }
                self.adding = None;
            }
            Message::CancelSignIn(Some(id)) => {
                let generation = self.next();
                if let Some(a) = self.account_mut(&id) {
                    if let AccountState::SigningIn { cancel } = &a.state {
                        cancel.cancel();
                    }
                    a.generation = generation;
                    a.state = AccountState::NeedsSignIn { error: None };
                }
            }
            Message::SignInFinished(origin, generation, profile, result) => {
                return self.sign_in_finished(origin, generation, profile, result);
            }
            Message::Saved(id, generation, result) => {
                if let Some(a) = self.account_mut(&id)
                    && a.generation == generation
                {
                    a.notice = result.err();
                }
            }
            Message::Refresh(id) => return self.load_resources(&id),
            Message::Discovered(id, generation, result) => {
                self.discovered(&id, generation, result);
            }
            Message::SignOut(id) => return self.sign_out(&id),
            Message::SignOutSelected => {
                if let Some(Page::Account(id)) = self.selected_page() {
                    return self.sign_out(&id);
                }
            }
            Message::SignOutFinished(result) => {
                if let Err(e) = result {
                    self.notice = Some(e);
                }
            }
            Message::Connect(key) => return self.prepare_connection(&key),
            Message::ConnectionFileReady(id, epoch, key, result) => {
                let Some(account) = self.account_mut(&id).filter(|a| a.epoch == epoch) else {
                    return Task::none();
                };
                let result = result.map(|(connection, renewed)| {
                    if let AccountState::Ready { signed_in, .. } = &mut account.state
                        && !Arc::ptr_eq(signed_in, &renewed)
                    {
                        *signed_in = renewed;
                    }
                    connection
                });
                return self.connection_file_ready(&id, key, result);
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
            Message::Session(id, epoch, key, event) => {
                if self.account(&id).is_some_and(|a| a.epoch == epoch) {
                    self.session_event(&key, event);
                }
            }
            Message::Disconnect(key) => {
                if let Some(s) = self.running.get(&key) {
                    s.disconnect();
                }
            }
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
            Message::WorkspaceMoved(key, result) => {
                self.movers.remove(&key);
                // Not worth interrupting the user: the desktop opened, only
                // not on a new workspace.
                if let Err(e) = result {
                    crate::app_log!("open on a new workspace: not moved ({e})");
                }
            }
        }
        Task::none()
    }
}

#[derive(Clone, Copy)]
enum Level {
    Account,
    Desktop,
}

impl AppModel {
    pub fn update_title(&mut self) -> Task<cosmic::Action<Message>> {
        let title = fl!("app-title");
        match self.core.main_window_id() {
            Some(id) => self.set_window_title(title, id),
            None => Task::none(),
        }
    }

    fn next(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    fn account_mut(&mut self, id: &str) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.id == id)
    }

    fn selected_page(&self) -> Option<Page> {
        self.nav.data::<Page>(self.nav.active()).cloned()
    }

    fn selected_account(&self) -> Option<&Account> {
        match self.selected_page()? {
            Page::Account(id) => self.account(&id),
            Page::Add => None,
        }
    }

    /// The account whose desktop list contains `key`.
    fn account_of_resource(&self, key: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.resource(key).is_some())
    }

    /// Rebuilds the sidebar: accounts in order, then "Add account".
    fn rebuild_nav(&mut self, select: &Page) {
        self.nav.clear();
        for a in &self.accounts {
            let page = Page::Account(a.id.clone());
            let entry = self
                .nav
                .insert()
                .text(a.label())
                .icon(widget::icon::from_name("avatar-default-symbolic").icon())
                .data(page.clone());
            if &page == select {
                entry.activate();
            }
        }
        let add = self
            .nav
            .insert()
            .text(fl!("add-account"))
            .icon(widget::icon::from_name("list-add-symbolic").icon())
            .data(Page::Add);
        if select == &Page::Add || self.accounts.is_empty() {
            add.activate();
        }
    }

    /// Changes one account's or desktop's settings and saves the config.
    /// Empty entries are removed.
    fn edit_settings(&mut self, level: Level, key: &str, f: impl FnOnce(&mut ConnectionSettings)) {
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
    fn effective_settings(&self, account: &str, key: &str) -> settings::Effective {
        settings::effective(
            self.config.account_settings.get(account),
            self.config.desktop_settings.get(key),
        )
    }

    fn account_settings_view(&self, id: &str) -> Element<'_, Message> {
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
                    .control(widget::spin_button(
                        fl!("seconds", value = current.timeout_secs),
                        fl!("setting-timeout"),
                        current.timeout_secs,
                        10,
                        settings::TIMEOUT_MIN_SECS,
                        settings::TIMEOUT_MAX_SECS,
                        move |v| Message::AccountTimeout(timeout_id.clone(), v),
                    )),
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
            .add(toggle(Toggle::Clipboard));
        widget::settings::view_column(vec![
            widget::text::body(fl!("account-settings-hint")).into(),
            connection.into(),
            display.into(),
            devices.into(),
        ])
        .into()
    }

    fn desktop_settings_view(&self, key: &str) -> Element<'_, Message> {
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
            .push(widget::spin_button(
                fl!("seconds", value = current.timeout_secs),
                fl!("setting-timeout"),
                current.timeout_secs,
                10,
                settings::TIMEOUT_MIN_SECS,
                settings::TIMEOUT_MAX_SECS,
                move |v| Message::DesktopTimeout(timeout_key.clone(), Some(v)),
            ));
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
            .add(choice(Toggle::Clipboard));
        widget::settings::view_column(vec![
            widget::text::body(fl!("desktop-settings-hint", account = account.label())).into(),
            connection.into(),
            display.into(),
            devices.into(),
        ])
        .into()
    }

    fn check_dependencies(&mut self) -> Task<cosmic::Action<Message>> {
        self.deps = None;
        cosmic::task::future(async move {
            let checks = tokio::task::spawn_blocking(deps::check_all)
                .await
                .unwrap_or_default();
            Message::DependenciesChecked(checks)
        })
    }

    fn page_view(&self) -> Element<'_, Message> {
        if self.loading_saved {
            return centered(widget::text::body(fl!("loading-accounts")).into());
        }
        match self.selected_page() {
            Some(Page::Account(id)) => match self.account(&id) {
                Some(account) => self.account_view(account),
                None => self.add_view(),
            },
            _ => self.add_view(),
        }
    }

    fn add_view(&self) -> Element<'_, Message> {
        let space = cosmic::theme::spacing();
        if let Some(Adding {
            cancel: Some(_), ..
        }) = &self.adding
        {
            return waiting_view(Message::CancelSignIn(None));
        }
        let mut col = widget::column::with_capacity(5)
            .push(widget::text::title1(if self.accounts.is_empty() {
                fl!("app-title")
            } else {
                fl!("add-account")
            }))
            .push(widget::text::body(fl!("unofficial-notice")).width(Length::Fixed(420.0)))
            .push(widget::button::suggested(fl!("sign-in")).on_press(Message::AddAccount))
            .spacing(space.space_m)
            .align_x(Alignment::Center);
        if let Some(Adding { error: Some(e), .. }) = &self.adding {
            col = col.push(widget::text::body(e.clone()).width(Length::Fixed(420.0)));
        }
        centered(col.into())
    }

    fn account_view<'a>(&'a self, account: &'a Account) -> Element<'a, Message> {
        let space = cosmic::theme::spacing();
        match &account.state {
            AccountState::Restoring => centered(
                widget::column::with_capacity(2)
                    .push(widget::text::title3(fl!("restoring")))
                    .push(widget::text::body(fl!("restoring-hint")))
                    .spacing(space.space_s)
                    .align_x(Alignment::Center)
                    .into(),
            ),
            AccountState::NeedsSignIn { error } => {
                let mut col = widget::column::with_capacity(5)
                    .push(widget::text::title3(account.label()))
                    .push(
                        widget::text::body(error.clone().unwrap_or_else(|| fl!("sign-in-needed")))
                            .width(Length::Fixed(420.0)),
                    )
                    .push(
                        widget::button::suggested(fl!("sign-in"))
                            .on_press(Message::SignInAccount(account.id.clone())),
                    )
                    .spacing(space.space_m)
                    .align_x(Alignment::Center);
                if account.can_retry {
                    col = col.push(
                        widget::button::standard(fl!("try-again"))
                            .on_press(Message::RetryRestore(account.id.clone())),
                    );
                }
                col = col.push(
                    widget::button::text(fl!("sign-out"))
                        .on_press(Message::SignOut(account.id.clone())),
                );
                centered(col.into())
            }
            AccountState::SigningIn { .. } => {
                waiting_view(Message::CancelSignIn(Some(account.id.clone())))
            }
            AccountState::Ready {
                loading,
                workspaces,
                error,
                ..
            } => resources_view(
                *loading,
                workspaces,
                error.as_deref(),
                account.notice.as_deref(),
                &self.connections,
            ),
        }
    }

    /// Start: one entry per saved sign-in, each restored in the background.
    fn saved_loaded(
        &mut self,
        result: Result<Vec<Stored>, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.loading_saved = false;
        let saved = match result {
            Ok(saved) => saved,
            Err(e) => {
                self.notice = Some(e);
                Vec::new()
            }
        };
        let mut tasks = Vec::new();
        for stored in saved {
            let id = accounts::account_id(&stored.identity);
            if self.account(&id).is_some() {
                continue;
            }
            accounts::adopt_legacy_profile(&id);
            let epoch = self.next();
            self.accounts.push(Account {
                id: id.clone(),
                identity: stored.identity.clone(),
                state: AccountState::Restoring,
                stored: Some(stored),
                notice: None,
                can_retry: false,
                generation: 0,
                epoch,
            });
            tasks.push(self.start_restore(&id));
        }
        let select = self
            .accounts
            .first()
            .map_or(Page::Add, |a| Page::Account(a.id.clone()));
        self.rebuild_nav(&select);
        Task::batch(tasks)
    }

    /// Signs in with the account's saved refresh token.
    fn start_restore(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let generation = self.next();
        let Some(account) = self.account_mut(id) else {
            return Task::none();
        };
        let Some(stored) = account.stored.clone() else {
            account.state = AccountState::NeedsSignIn { error: None };
            return Task::none();
        };
        account.generation = generation;
        account.state = AccountState::Restoring;
        account.can_retry = false;
        let http = self.http.clone();
        let id = id.to_owned();
        cosmic::task::future(async move {
            let outcome = tokio::task::spawn_blocking(move || accounts::restore(&http, &stored))
                .await
                .unwrap_or_else(|_| Restore::Retry(fl!("error-generic")));
            Message::Restored(id, generation, outcome)
        })
    }

    fn restored(
        &mut self,
        id: &str,
        generation: u64,
        outcome: Restore,
    ) -> Task<cosmic::Action<Message>> {
        let Some(account) = self.account_mut(id).filter(|a| a.generation == generation) else {
            return Task::none();
        };
        match outcome {
            Restore::Ready(signed_in, notice) => {
                account.identity = signed_in.identity.clone();
                account.notice = notice;
                account.state = AccountState::Ready {
                    signed_in,
                    loading: true,
                    workspaces: Vec::new(),
                    error: None,
                };
                self.load_resources(id)
            }
            Restore::SignInAgain(error) => {
                account.stored = None;
                account.state = AccountState::NeedsSignIn { error: Some(error) };
                Task::none()
            }
            Restore::Retry(error) => {
                account.can_retry = true;
                account.state = AccountState::NeedsSignIn { error: Some(error) };
                Task::none()
            }
        }
    }

    /// Opens the sign-in window for an account not in the list yet, with a
    /// fresh cookie profile.
    fn start_add(&mut self) -> Task<cosmic::Action<Message>> {
        if matches!(
            self.adding,
            Some(Adding {
                cancel: Some(_),
                ..
            })
        ) {
            return Task::none();
        }
        self.rebuild_nav(&Page::Add);
        let generation = self.next();
        let profile = accounts::new_profile_dir();
        let error = |e| Adding {
            generation,
            cancel: None,
            error: Some(e),
        };
        let (Some(helper), Some(profile)) = (signin::find_helper(), profile) else {
            self.adding = Some(error(fl!("error-helper-missing")));
            return Task::none();
        };
        let cancel = Cancel::default();
        self.adding = Some(Adding {
            generation,
            cancel: Some(cancel.clone()),
            error: None,
        });
        self.run_sign_in(None, generation, helper, profile, None, cancel)
    }

    /// Signs in again to an account in the list, with its own cookies and
    /// its user name preselected.
    fn start_account_sign_in(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let generation = self.next();
        let (Some(helper), Some(profile)) = (signin::find_helper(), accounts::profile_dir(id))
        else {
            if let Some(a) = self.account_mut(id) {
                a.state = AccountState::NeedsSignIn {
                    error: Some(fl!("error-helper-missing")),
                };
            }
            return Task::none();
        };
        let Some(account) = self.account_mut(id) else {
            return Task::none();
        };
        let cancel = Cancel::default();
        account.generation = generation;
        account.state = AccountState::SigningIn {
            cancel: cancel.clone(),
        };
        let hint = account.identity.username.clone();
        self.run_sign_in(
            Some(id.to_owned()),
            generation,
            helper,
            profile,
            hint,
            cancel,
        )
    }

    fn run_sign_in(
        &self,
        origin: Option<String>,
        generation: u64,
        helper: PathBuf,
        profile: PathBuf,
        login_hint: Option<String>,
        cancel: Cancel,
    ) -> Task<cosmic::Action<Message>> {
        let http = self.http.clone();
        cosmic::task::future(async move {
            let used = profile.clone();
            let result = tokio::task::spawn_blocking(move || {
                signin::sign_in(
                    &http,
                    &ClientConfig::public_remote_desktop(),
                    &helper,
                    &profile,
                    login_hint.as_deref(),
                    &cancel,
                )
            })
            .await
            .unwrap_or(Err(SignInError::HelperFailed("task".into())));
            Message::SignInFinished(
                origin,
                generation,
                used,
                result.map(Arc::new).map_err(describe_sign_in_error),
            )
        })
    }

    fn sign_in_finished(
        &mut self,
        origin: Option<String>,
        generation: u64,
        profile: PathBuf,
        result: Result<Arc<SignedIn>, SignInFailure>,
    ) -> Task<cosmic::Action<Message>> {
        let current = match &origin {
            None => self
                .adding
                .as_ref()
                .is_some_and(|a| a.generation == generation),
            Some(id) => self.account(id).is_some_and(|a| a.generation == generation),
        };
        if !current {
            return Task::none();
        }
        let signed_in = match result {
            Ok(s) => s,
            Err(SignInFailure(error)) => {
                match &origin {
                    None => {
                        self.adding = error.map(|e| Adding {
                            generation,
                            cancel: None,
                            error: Some(e),
                        });
                    }
                    Some(id) => {
                        if let Some(a) = self.account_mut(id) {
                            a.state = AccountState::NeedsSignIn { error };
                        }
                    }
                }
                return Task::none();
            }
        };
        let id = accounts::account_id(&signed_in.identity);
        match &origin {
            None => {
                self.adding = None;
                accounts::adopt_profile(&profile, &id);
            }
            // Signed in to a different account in that account's window:
            // the original stays signed out, the new one is added.
            Some(old) if *old != id => {
                if let Some(a) = self.account_mut(old) {
                    a.state = AccountState::NeedsSignIn { error: None };
                }
            }
            Some(_) => {}
        }
        let generation = self.next();
        let epoch = self.next();
        let state = AccountState::Ready {
            signed_in: signed_in.clone(),
            loading: true,
            workspaces: Vec::new(),
            error: None,
        };
        match self.account_mut(&id) {
            Some(a) => {
                a.identity = signed_in.identity.clone();
                a.state = state;
                a.notice = None;
                a.can_retry = false;
                a.generation = generation;
            }
            None => self.accounts.push(Account {
                id: id.clone(),
                identity: signed_in.identity.clone(),
                state,
                stored: None,
                notice: None,
                can_retry: false,
                generation,
                epoch,
            }),
        }
        self.rebuild_nav(&Page::Account(id.clone()));
        let started = accounts::sign_out_count(&id);
        let save_id = id.clone();
        let save = cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || accounts::save(&signed_in, started))
                .await
                .unwrap_or_else(|_| Err(fl!("error-generic")));
            Message::Saved(save_id, generation, result)
        });
        Task::batch([save, self.load_resources(&id)])
    }

    fn load_resources(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let generation = self.next();
        let Some(account) = self.account_mut(id) else {
            return Task::none();
        };
        let AccountState::Ready {
            signed_in, loading, ..
        } = &mut account.state
        else {
            return Task::none();
        };
        *loading = true;
        account.generation = generation;
        let signed_in = signed_in.clone();
        let http = self.http.clone();
        let id = id.to_owned();
        cosmic::task::future(async move {
            let acc = signed_in.clone();
            let result = tokio::task::spawn_blocking(move || {
                discovery::discover(&http, &acc.tokens.access_token)
            })
            .await;
            let result = match result {
                Ok(Ok(feeds)) => Ok(feeds
                    .into_iter()
                    .map(|f| workspace_view(&signed_in, f))
                    .collect()),
                Ok(Err(e)) => Err(describe_fetch_error(&e)),
                Err(_) => Err(fl!("error-generic")),
            };
            Message::Discovered(id, generation, result)
        })
    }

    fn discovered(
        &mut self,
        id: &str,
        generation: u64,
        result: Result<Vec<WorkspaceView>, String>,
    ) {
        let Some(account) = self.account_mut(id).filter(|a| a.generation == generation) else {
            return;
        };
        if let AccountState::Ready {
            loading,
            workspaces,
            error,
            ..
        } = &mut account.state
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

    /// Signs one account out: ends its sessions, removes it from the list,
    /// deletes its cookies and keyring item (spec §6).
    fn sign_out(&mut self, id: &str) -> Task<cosmic::Action<Message>> {
        let Some(pos) = self.accounts.iter().position(|a| a.id == id) else {
            return Task::none();
        };
        let account = self.accounts.remove(pos);
        if let AccountState::SigningIn { cancel } = &account.state {
            cancel.cancel();
        }
        for key in account.resource_keys() {
            if let Some(s) = self.running.remove(&key) {
                s.disconnect();
            }
            self.connections.remove(&key);
            self.pending_logon.remove(&key);
            self.last_failure.remove(&key);
            self.entra_failed.remove(&key);
        }
        if self.dialog.as_ref().is_some_and(|d| d.account == id) {
            self.dialog = None;
        }
        if accounts::remove_files(id).is_err() {
            self.notice = Some(fl!("error-sign-out-incomplete"));
        }
        let select = self
            .accounts
            .first()
            .map_or(Page::Add, |a| Page::Account(a.id.clone()));
        self.rebuild_nav(&select);
        // The keyring may block (for example on an unlock prompt), so not on
        // the UI thread.
        let identity = account.identity;
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || accounts::delete(&identity))
                .await
                .unwrap_or_else(|_| Err(fl!("error-sign-out-incomplete")));
            Message::SignOutFinished(result)
        })
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
        let Some(account) = self.account_of_resource(key) else {
            return Task::none();
        };
        let (Some(signed_in), Some(resource)) =
            (account.signed_in().cloned(), account.resource(key))
        else {
            return Task::none();
        };
        let (id, epoch) = (account.id.clone(), account.epoch);
        let Some(url) = resource.connection_file.clone() else {
            self.connections.insert(
                key.to_owned(),
                Connection::Failed(fl!("error-no-connection-file")),
            );
            return Task::none();
        };
        self.connections
            .insert(key.to_owned(), Connection::Preparing);
        let http = self.http.clone();
        let key = key.to_owned();
        cosmic::task::future(async move {
            let result = tokio::task::spawn_blocking(move || {
                // The access token also goes to FreeRDP, so it must stay
                // valid through the connection.
                let signed_in = accounts::renew_if_needed(&http, signed_in)?;
                let resp = http
                    .get(
                        &url,
                        Role::ConnectionFile,
                        Some(&signed_in.tokens.access_token),
                    )
                    .map_err(|e| describe_fetch_error(&FetchError::Http(e)))?;
                ConnectionFile::parse(&resp.body)
                    .map(|c| (Arc::new(c), signed_in))
                    .map_err(|e| fl!("error-bad-connection-file", detail = format!("{e:?}")))
            })
            .await
            .unwrap_or_else(|_| Err(fl!("error-generic")));
            Message::ConnectionFileReady(id, epoch, key, result)
        })
    }

    /// Step 2: Entra sign-in first; the Windows password only when chosen or
    /// after Entra sign-in failed.
    fn connection_file_ready(
        &mut self,
        account: &str,
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
            return self.launch(account, &key, &connection, LogonSetting::EntraSignIn, None);
        }
        let remembered = match remembered {
            Some(LogonSetting::EntraSignIn) | None => LogonSetting::Automatic,
            Some(other) => other,
        };
        let title = self
            .account(account)
            .and_then(|a| a.resource(&key))
            .map(|r| r.title.clone())
            .unwrap_or_default();
        let (logon, custom_domain) = logon_index(&remembered);
        let error = self.last_failure.remove(&key);
        self.dialog = Some(PasswordDialog {
            account: account.to_owned(),
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
        self.launch(&d.account, &d.key, &d.connection, logon, password)
    }

    /// Step 3: start FreeRDP and forward its events to the app.
    fn launch(
        &mut self,
        account: &str,
        key: &str,
        connection: &Arc<ConnectionFile>,
        logon: LogonSetting,
        password: Option<Secret>,
    ) -> Task<cosmic::Action<Message>> {
        let Some((signed_in, epoch)) = self
            .account(account)
            .and_then(|a| Some((a.signed_in()?.clone(), a.epoch)))
        else {
            return Task::none();
        };
        let (Some(helper), Some(profile)) = (signin::find_helper(), accounts::profile_dir(account))
        else {
            self.connections.insert(
                key.to_owned(),
                Connection::Failed(fl!("error-helper-missing")),
            );
            return Task::none();
        };
        let settings = self.effective_settings(account, key);
        let window_title = self
            .account(account)
            .and_then(|a| Some(format!("{} – {}", a.resource(key)?.title, a.label())))
            .unwrap_or_default();
        let spec = LaunchSpec {
            connection: (**connection).clone(),
            username: signed_in.identity.username.clone(),
            password,
            logon: logon.clone(),
            helper,
            profile,
            gateway_token: (!signed_in.tokens.needs_refresh(Instant::now()))
                .then(|| signed_in.tokens.access_token.clone()),
            settings,
            window_title: window_title.clone(),
            app_id: <Self as cosmic::Application>::APP_ID.to_owned(),
        };
        // No secrets here: the setting and, for a custom domain, its name.
        crate::app_log!("Starting FreeRDP with logon setting {logon:?}");
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
                let events = {
                    let (account, key) = (account.to_owned(), key.to_owned());
                    cosmic::task::stream(
                        rx.map(move |e| Message::Session(account.clone(), epoch, key.clone(), e)),
                    )
                };
                if !settings.new_workspace {
                    return events;
                }
                let stop = Arc::new(AtomicBool::new(false));
                self.movers.insert(key.to_owned(), stop.clone());
                let title = launcher::safe_title(&window_title);
                // The window appears after authentication: wait as long as
                // the connection may take, plus the sign-in page.
                let wait = std::time::Duration::from_secs(u64::from(settings.timeout_secs) + 180);
                let key = key.to_owned();
                let mover = cosmic::task::future(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        crate::workspace::move_to_new_workspace(
                            <AppModel as cosmic::Application>::APP_ID,
                            &title,
                            wait,
                            &stop,
                        )
                    })
                    .await;
                    let result = match result {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(e)) => Err(e.to_string()),
                        Err(_) => Err("task".to_owned()),
                    };
                    Message::WorkspaceMoved(key, result)
                });
                Task::batch([events, mover])
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
                if let Some(stop) = self.movers.remove(key) {
                    stop.store(true, Ordering::SeqCst);
                }
                let pending = self.pending_logon.remove(key);
                if !log.is_empty() && end != SessionEnd::Closed {
                    crate::app_log!("FreeRDP ended ({end:?}); last log lines:");
                    for line in &log {
                        crate::app_log!("  {line}");
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

/// "Waiting for sign-in" with a Cancel button.
fn waiting_view(cancel: Message) -> Element<'static, Message> {
    centered(
        widget::column::with_capacity(3)
            .push(widget::text::title3(fl!("signing-in")))
            .push(widget::text::body(fl!("signing-in-hint")))
            .push(widget::button::standard(fl!("cancel")).on_press(cancel))
            .spacing(cosmic::theme::spacing().space_s)
            .align_x(Alignment::Center)
            .into(),
    )
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    Dependencies,
    AccountSettings,
    DesktopSettings,
}

/// Name and description of an on/off setting.
fn toggle_text(t: Toggle) -> (String, String) {
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
        Toggle::Clipboard => (fl!("setting-clipboard"), fl!("setting-clipboard-desc")),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
    Dependencies,
    AddAccount,
    SignOut,
    OpenLogFolder,
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
            MenuAction::Dependencies => Message::ToggleContextPage(ContextPage::Dependencies),
            MenuAction::AddAccount => Message::AddAccount,
            MenuAction::SignOut => Message::SignOutSelected,
            MenuAction::OpenLogFolder => Message::OpenLogFolder,
        }
    }
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
        Dependency::CosmicWindows => (fl!("dep-cosmic-windows"), fl!("dep-cosmic-windows-hint")),
    }
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
    let settings = widget::button::icon(widget::icon::from_name("preferences-system-symbolic"))
        .tooltip(fl!("desktop-settings"))
        .on_press(Message::OpenDesktopSettings(r.key.clone()));
    widget::settings::item::builder(r.title.clone())
        .description(description)
        .control(
            widget::row::with_capacity(2)
                .push(settings)
                .push(control)
                .spacing(cosmic::theme::spacing().space_xs)
                .align_y(Alignment::Center),
        )
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
