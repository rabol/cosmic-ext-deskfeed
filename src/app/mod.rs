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

mod account_flow;
mod connect;
mod decide;
mod errors;
mod model;
mod settings_ui;
mod views;

use errors::*;
use model::*;
use settings_ui::*;
use views::*;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const APP_ICON: &[u8] = include_bytes!("../../resources/icons/hicolor/256x256/apps/icon.png");

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
    /// Entra attempts that failed per resource since it last connected.
    entra_failures: HashMap<String, u32>,
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

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "io.github.rabol.cosmic-ext-deskfeed";

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
        crate::app_log!("Deskfeed {} started", env!("CARGO_PKG_VERSION"));
        launcher::clean_stale_runtime_dirs();
        accounts::clean_unfinished_profiles();
        let about = About::default()
            .name(fl!("app-title"))
            .icon(widget::icon::from_raster_bytes(APP_ICON))
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
            entra_failures: HashMap::new(),
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

    fn check_dependencies(&mut self) -> Task<cosmic::Action<Message>> {
        self.deps = None;
        cosmic::task::future(async move {
            let checks = tokio::task::spawn_blocking(deps::check_all)
                .await
                .unwrap_or_default();
            Message::DependenciesChecked(checks)
        })
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    Dependencies,
    AccountSettings,
    DesktopSettings,
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
