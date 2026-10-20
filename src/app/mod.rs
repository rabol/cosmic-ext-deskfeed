// SPDX-License-Identifier: MIT

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime};

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

use crate::accounts;
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
use crate::service::icons;
use crate::sessions::launcher::{self, LaunchSpec, LogonSetting, SessionEnd, SessionEvent};
use crate::sessions::rdpfile::{CameraPolicy, ConnectionFile, RdpError};
use crate::settings::{self, Choice, ConnectionSettings, Limited, Number, Toggle};

/// Sets one of `Config`'s fields and saves it: through cosmic-config's
/// generated setter `$set`, which also sets it in memory, or in memory only
/// when there is no settings store (it couldn't be opened; tests). A write
/// that fails is logged.
macro_rules! save_config {
    ($app:expr, $field:ident, $set:ident, $value:expr) => {{
        let value = $value;
        match &$app.config_handler {
            Some(handler) => {
                if let Err(e) = $app.config.$set(handler, value) {
                    crate::app_log!("settings: {} not saved ({e})", stringify!($field));
                }
            }
            None => $app.config.$field = value,
        }
    }};
}

mod account_flow;
mod background;
mod certificate;
mod connect;
mod decide;
mod desk;
mod diagnostics;
mod errors;
mod launch;
mod model;
mod password;
mod quit;
mod rdp;
mod session;
mod settings_ui;
mod views;

// One type split across files: the submodules share the parent's items.
#[allow(clippy::wildcard_imports)]
use errors::*;
#[allow(clippy::wildcard_imports)]
use model::*;
#[allow(clippy::wildcard_imports)]
use views::*;

/// The tray's events, for the subscription (a plain `fn`, so iced keeps
/// one tray running while background operation stays on).
fn tray_events() -> impl cosmic::iced::futures::Stream<Item = crate::tray::TrayEvent> {
    crate::tray::run(crate::tray::Labels {
        title: fl!("app-title"),
        show: fl!("tray-show"),
        quit: fl!("tray-quit"),
    })
}

/// The app ID (also the tray item's ID and icon name).
pub const APP_ID: &str = "io.github.rabol.cosmic-ext-deskfeed";
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
    /// The accounts' saved sign-ins and the work on them (`accounts`).
    sign_ins: Arc<accounts::Accounts>,
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
    /// Everything tracked per desktop or app (`desk.rs`).
    desks: desk::Desks,
    /// Accounts whose sign-out is still ending their processes and deleting
    /// their files; they can't be signed in again until it's done.
    signing_out: HashSet<String>,
    /// The account whose sign-out waits for an answer: it has desktops or
    /// apps open (spec §6).
    sign_out_asking: Option<(String, Leaving)>,
    /// What the last sign-out couldn't remove.
    cleanup_retry: Option<CleanupRetry>,
    /// The last list check (monotonic and wall clock), to notice sleep.
    last_check: Option<(Instant, SystemTime)>,
    /// The search field's text; filters the desktop list.
    search: String,
    /// The "Add connection" form (`rdp.rs`).
    connection_form: ConnectionForm,
    /// The saved connections as list rows, in title order; rebuilt when
    /// they change (`rdp.rs`).
    connection_rows: Vec<ResourceView>,
    /// The tray, while background operation is on and a tray answers.
    tray: Option<crate::tray::Handle>,
    /// Why there is no tray, when background operation is on.
    tray_unavailable: Option<String>,
    /// The menu the tray shows now (sent again only when it changes).
    tray_menu: Option<Vec<crate::tray::TrayItem>>,
    /// "Start when you log in" (its autostart entry is there).
    autostart: bool,
    /// The diagnostics report while its page is open (`None`: building).
    diagnostics: Option<String>,
    /// Where the report was saved, or why it wasn't.
    diagnostics_status: Option<String>,
    /// Where offline desktop lists live (`None` without a cache folder).
    cache: Option<crate::cache::Store>,
    /// Resource icons per account and URL; the feed versions its icon
    /// URLs (`?v=`), so a fetched one stays valid.
    icons: HashMap<(String, Url), widget::icon::Handle>,
    quit: Quit,
    /// The password dialog shown, and the ones waiting their turn: each
    /// resource keeps its own, none replaces another.
    dialogs: decide::DialogQueue<PasswordDialog>,
    certificate_dialog: Option<CertificateDialog>,
    logon_labels: Vec<String>,
    /// Last dependency check; `None` while it runs.
    deps: Option<Vec<Check>>,
    /// Account ID or resource key the settings drawer shows.
    settings_target: Option<String>,
    /// Desktop setting choices when the account's value is on / off.
    follow_app_on: Vec<String>,
    follow_app_off: Vec<String>,
    follow_account_on: Vec<String>,
    follow_account_off: Vec<String>,
}

/// How often to check whether a desktop list is due for its refresh
/// (hourly by default, spec §7; the "Refresh lists" setting). Checking the
/// wall clock this often also catches a resume from sleep soon.
const REFRESH_CHECK: Duration = Duration::from_mins(5);

/// A page in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Page {
    Account(String),
    Add,
    /// The saved plain RDP connections (ADR 0029).
    Connections,
    /// The "Add connection" form, or editing one.
    AddConnection,
}

/// Adding an account (the Add page).
enum Adding {
    /// The sign-in window is open; `generation` tells its answer from a
    /// cancelled one's.
    SigningIn { generation: u64, cancel: Cancel },
    /// It didn't work; shown on the Add page.
    Failed(String),
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

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
        let config_handler = cosmic_config::Config::new(Self::APP_ID, Config::VERSION).ok();
        let config = config_handler
            .as_ref()
            .map(|context| match Config::get_entry(context) {
                Ok(config) | Err((_, config)) => config,
            })
            .unwrap_or_default();
        crate::log::set_max_size_mib(config.log_max_mib.unwrap_or(crate::log::MAX_SIZE_MIB.2));
        let mut app = AppModel::new(
            core,
            config,
            config_handler,
            Http::new(EndpointPolicy::azure_public()),
            accounts::Accounts::new(ClientConfig::public_remote_desktop()),
            crate::cache::Store::user(),
            crate::autostart::enabled(),
        );
        app.rebuild_nav(&Page::Add);
        let title = app.update_title();
        let sign_ins = app.sign_ins.clone();
        let load = cosmic::task::future(async move {
            let result = off_ui(
                move || {
                    sign_ins
                        .load_saved()
                        .map_err(|e| fl!("error-keyring-load", detail = keyring_detail(&e)))
                },
                Err(fl!("error-generic")),
            )
            .await;
            Message::Account(AccountMessage::SavedLoaded(result))
        });
        let deps = app.check_dependencies();
        let theme = settings_ui::apply_theme(app.config.app_theme.unwrap_or(0));
        // Started at login without a window (`--background`): it stays
        // hidden only with background operation on; the tray's answer
        // decides the rest (`tray_event`).
        let window = if app.core.main_window_id().is_none() && !app.config.background {
            crate::app_log!("started in the background, but background operation is off");
            app.show_window()
        } else {
            Task::none()
        };
        (app, Task::batch([title, load, deps, theme, window]))
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<cosmic::Action<Self::Message>> {
        self.nav.activate(id);
        if self.nav.data::<Page>(id) == Some(&Page::AddConnection) {
            self.connection_form = ConnectionForm::default();
        }
        // "Add account" (or "Sign in") reads as an action, so a click starts
        // the sign-in, also when its page is already shown: with no
        // account it's the only entry, and selecting it again did nothing.
        if self.nav.data::<Page>(id) == Some(&Page::Add) {
            return self.start_add();
        }
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
                        menu::Item::Divider,
                        menu::Item::Button(fl!("quit"), None, MenuAction::Quit),
                    ],
                ),
            ),
            menu::Tree::with_children(
                menu::root(fl!("view")).apply(Element::from),
                menu::items(
                    &self.key_binds,
                    vec![
                        menu::Item::Button(fl!("settings"), None, MenuAction::Settings),
                        menu::Item::Button(fl!("dependencies"), None, MenuAction::Dependencies),
                        menu::Item::Button(fl!("open-log-folder"), None, MenuAction::OpenLogFolder),
                        menu::Item::Button(fl!("diagnostics"), None, MenuAction::Diagnostics),
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
                    .on_press(Message::Settings(SettingsMessage::OpenAccountSettings(
                        account.id.clone(),
                    )))
                    .into(),
            ];
        };
        let mut refresh = widget::button::text(fl!("refresh"));
        if !loading {
            refresh = refresh.on_press(Message::Account(AccountMessage::Refresh(
                account.id.clone(),
            )));
        }
        vec![
            refresh.into(),
            widget::button::icon(widget::icon::from_name("preferences-system-symbolic"))
                .tooltip(fl!("account-settings"))
                .on_press(Message::Settings(SettingsMessage::OpenAccountSettings(
                    account.id.clone(),
                )))
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
                |url| Message::App(AppMessage::LaunchUrl(url.to_string())),
                Message::App(AppMessage::ToggleContextPage(ContextPage::About)),
            ),
            ContextPage::Dependencies => context_drawer::context_drawer(
                dependencies_view(self.deps.as_deref()),
                Message::App(AppMessage::ToggleContextPage(ContextPage::Dependencies)),
            )
            .title(fl!("dependencies"))
            .actions({
                let mut b = widget::button::standard(fl!("check-again"));
                if self.deps.is_some() {
                    b = b.on_press(Message::App(AppMessage::CheckDependencies));
                }
                b
            }),
            ContextPage::Diagnostics => {
                let space = cosmic::theme::spacing();
                let mut col = widget::column::with_capacity(3)
                    .spacing(space.space_s)
                    .push(widget::text::body(fl!("diagnostics-hint")));
                if let Some(status) = &self.diagnostics_status {
                    col = col.push(widget::text::body(status.clone()));
                }
                col = col.push(match &self.diagnostics {
                    None => Element::from(widget::text::body(fl!("diagnostics-building"))),
                    Some(text) => widget::text::monotext(text.clone()).size(12).into(),
                });
                let mut save = widget::button::suggested(fl!("diagnostics-save"));
                if self.diagnostics.is_some() {
                    save = save.on_press(Message::App(AppMessage::SaveDiagnostics));
                }
                context_drawer::context_drawer(
                    col,
                    Message::App(AppMessage::ToggleContextPage(ContextPage::Diagnostics)),
                )
                .title(fl!("diagnostics"))
                .actions(save)
            }
            ContextPage::AppSettings => context_drawer::context_drawer(
                self.app_settings_view(),
                Message::App(AppMessage::ToggleContextPage(ContextPage::AppSettings)),
            )
            .title(fl!("settings")),
            ContextPage::AccountSettings => {
                let id = self.settings_target.clone().unwrap_or_default();
                let title = self.account(&id).map(Account::label).unwrap_or_default();
                context_drawer::context_drawer(
                    self.account_settings_view(&id),
                    Message::App(AppMessage::ToggleContextPage(ContextPage::AccountSettings)),
                )
                .title(title)
                .actions(
                    widget::button::destructive(fl!("sign-out"))
                        .on_press(Message::Account(AccountMessage::SignOut(id))),
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
                    Message::App(AppMessage::ToggleContextPage(ContextPage::DesktopSettings)),
                )
                .title(title)
            }
        })
    }

    /// The window's close button.
    fn on_app_exit(&mut self) -> Option<Self::Message> {
        Some(Message::Quit(QuitMessage::Requested))
    }

    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        if self.quit == Quit::Asking {
            return Some(self.quit_dialog());
        }
        if let Some((id, leaving)) = &self.sign_out_asking {
            return Some(self.sign_out_dialog(id, *leaving));
        }
        if let Some(d) = &self.certificate_dialog {
            return Some(certificate::certificate_dialog(d));
        }
        let d = self.dialogs.shown()?;
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
                    Some(Message::Connection(
                        ConnectionMessage::TogglePasswordVisible,
                    )),
                    d.password_hidden,
                )
                .on_input(|s| {
                    Message::Connection(ConnectionMessage::PasswordInput(PasswordText(s)))
                })
                .on_submit(|_| Message::Connection(ConnectionMessage::DialogConnect)),
            );
        }
        controls = controls.push(
            widget::column::with_capacity(2)
                .push(widget::text::caption(fl!("logon-setting")))
                .push(widget::dropdown(
                    &self.logon_labels,
                    Some(d.logon),
                    |logon| Message::Connection(ConnectionMessage::LogonSelected(logon)),
                ))
                .spacing(space.space_xxs),
        );
        if d.logon == LOGON_CUSTOM {
            controls = controls.push(
                widget::text_input(fl!("domain-placeholder"), d.custom_domain.as_str()).on_input(
                    |domain| Message::Connection(ConnectionMessage::CustomDomainInput(domain)),
                ),
            );
        }
        if d.logon != LOGON_ENTRA {
            controls = controls.push(
                widget::checkbox(d.remember)
                    .label(fl!("remember-password"))
                    .on_toggle(|on| {
                        Message::Connection(ConnectionMessage::ToggleRememberPassword(on))
                    }),
            );
        }
        controls = controls.push(widget::text::caption(fl!("password-explainer")));

        Some(
            widget::dialog()
                .title(fl!("connect-to", title = d.title.clone()))
                .control(controls)
                .primary_action(
                    widget::button::suggested(fl!("connect"))
                        .on_press(Message::Connection(ConnectionMessage::DialogConnect)),
                )
                .secondary_action(
                    widget::button::standard(fl!("cancel"))
                        .on_press(Message::Connection(ConnectionMessage::DialogCancel)),
                )
                .into(),
        )
    }

    /// Deskfeed has only its main window. After it closed for background
    /// operation, the closing window may still be drawn once: nothing.
    fn view_window(&self, _id: cosmic::iced::window::Id) -> Element<'_, Self::Message> {
        widget::Space::new().into()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let space = cosmic::theme::spacing();
        let mut col = widget::column::with_capacity(3).spacing(space.space_s);
        // Missing required dependencies: a warning above everything, with
        // the details in the context drawer.
        if let Some(checks) = &self.deps
            && !deps::required_ok(checks)
        {
            col = col.push(widget::warning(fl!("deps-missing")).on_close(Message::App(
                AppMessage::ToggleContextPage(ContextPage::Dependencies),
            )));
        }
        if let Some(notice) = &self.notice {
            col = col.push(
                widget::warning(notice.clone()).on_close(Message::App(AppMessage::DismissNotice)),
            );
        }
        if self.cleanup_retry.is_some() {
            col = col.push(
                widget::row::with_capacity(2)
                    .push(
                        widget::container(
                            widget::warning(fl!("error-sign-out-incomplete"))
                                .on_close(Message::Account(AccountMessage::DismissSignOutCleanup)),
                        )
                        .width(Length::Fill),
                    )
                    .push(
                        widget::button::standard(fl!("try-again"))
                            .on_press(Message::Account(AccountMessage::RetrySignOutCleanup)),
                    )
                    .spacing(cosmic::theme::spacing().space_xs)
                    .align_y(Alignment::Center),
            );
        }
        col.push(self.page_view()).into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch([
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| Message::App(AppMessage::UpdateConfig(Box::new(update.config)))),
            // Checks the wall clock, so lists are also refreshed soon after
            // a resume from suspend (the timer itself stops while
            // suspended).
            cosmic::iced::time::every(REFRESH_CHECK)
                .map(|_| Message::Account(AccountMessage::RefreshDue)),
            // Closing from the desktop (window menu, keyboard shortcut,
            // dock); the window's own close button goes to `on_app_exit`.
            cosmic::iced::window::close_requests().map(|_| Message::Quit(QuitMessage::Requested)),
            // Shortcuts (`key_binds`) while the window has the keyboard.
            cosmic::iced::event::listen_with(|event, _status, _window| {
                let cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                    key,
                    modifiers,
                    physical_key,
                    ..
                }) = event
                else {
                    return None;
                };
                key_binds()
                    .into_iter()
                    .find(|(bind, _)| bind.matches(modifiers, &key, Some(&physical_key)))
                    .map(|(_, action)| menu::action::MenuAction::message(&action))
            }),
            // Full-screen sessions that keep the screen on (`awake.rs`).
            self.awake_subscription(),
            // Started again: show the window (`single.rs`).
            Subscription::run(crate::single::activations)
                .map(|token| Message::App(AppMessage::StartedAgain(token))),
            if self.config.background {
                Subscription::run(tray_events).map(|event| Message::App(AppMessage::Tray(event)))
            } else {
                Subscription::none()
            },
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        let task = self.route(message);
        // With the window closed (background operation): open it when
        // something needs the user, and keep the tray's menu current.
        let window = self.window_needed();
        let tray = self.update_tray();
        // Desks with nothing left to remember go.
        self.desks.tidy();
        Task::batch([task, window, tray])
    }
}

/// The About page.
fn about() -> About {
    About::default()
        .name(fl!("app-title"))
        .icon(widget::icon::from_raster_bytes(APP_ICON))
        .version(env!("CARGO_PKG_VERSION"))
        .links([(fl!("repository"), REPOSITORY)])
        .license(env!("CARGO_PKG_LICENSE"))
}

impl AppModel {
    /// The app with nothing loaded yet. `init` adds what starting does
    /// (cleaning up, loading saved sign-ins); tests use it as it is.
    fn new(
        core: cosmic::Core,
        config: Config,
        config_handler: Option<cosmic_config::Config>,
        http: Http,
        sign_ins: accounts::Accounts,
        cache: Option<crate::cache::Store>,
        autostart: bool,
    ) -> Self {
        let mut app = AppModel {
            core,
            context_page: ContextPage::default(),
            about: about(),
            key_binds: key_binds(),
            config,
            config_handler,
            http: Arc::new(http),
            sign_ins: Arc::new(sign_ins),
            nav: nav_bar::Model::default(),
            accounts: Vec::new(),
            loading_saved: true,
            adding: None,
            notice: None,
            counter: 0,
            desks: desk::Desks::default(),
            signing_out: HashSet::new(),
            sign_out_asking: None,
            cleanup_retry: None,
            last_check: None,
            search: String::new(),
            connection_form: ConnectionForm::default(),
            connection_rows: Vec::new(),
            tray: None,
            tray_unavailable: None,
            tray_menu: None,
            autostart,
            diagnostics: None,
            diagnostics_status: None,
            cache,
            icons: HashMap::new(),
            quit: Quit::No,
            dialogs: decide::DialogQueue::default(),
            certificate_dialog: None,
            logon_labels: vec![
                fl!("logon-automatic"),
                fl!("logon-empty-domain"),
                fl!("logon-custom-domain"),
                fl!("logon-entra"),
            ],
            deps: None,
            settings_target: None,
            follow_app_on: vec![fl!("follow-app-on"), fl!("on"), fl!("off")],
            follow_app_off: vec![fl!("follow-app-off"), fl!("on"), fl!("off")],
            follow_account_on: vec![fl!("follow-account-on"), fl!("on"), fl!("off")],
            follow_account_off: vec![fl!("follow-account-off"), fl!("on"), fl!("off")],
        };
        app.refresh_connection_rows();
        app
    }

    /// Hands each message to the part of the app it belongs to.
    fn route(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::Account(m) => self.account_message(m),
            Message::Connection(m) => self.connection_message(m),
            Message::Settings(m) => self.settings_message(m),
            Message::Quit(m) => self.quit_message(m),
            Message::App(m) => self.app_message(m),
            Message::Rdp(m) => self.rdp_message(m),
        }
    }

    /// App-wide messages: dependencies, the context drawer, links, notices, the log. Routed here by `route`.
    // One `match` over this handler's messages; splitting it would only
    // spread the routing out.
    #[allow(clippy::too_many_lines)]
    pub(super) fn app_message(&mut self, message: AppMessage) -> Task<cosmic::Action<Message>> {
        match message {
            AppMessage::Tray(event) => return self.tray_event(event),
            AppMessage::Awake(event) => match event {
                crate::awake::AwakeEvent::Held(true) => {
                    crate::app_log!("screen kept on: a full-screen desktop is in front");
                }
                crate::awake::AwakeEvent::Held(false) => {
                    crate::app_log!("screen may turn off again");
                }
                crate::awake::AwakeEvent::Unavailable(why) => {
                    crate::app_log!("can't keep the screen on: {why}");
                }
            },
            AppMessage::StartedAgain(token) => {
                crate::app_log!("started again: showing the window");
                return self.tray_event(crate::tray::TrayEvent::Show(token));
            }
            AppMessage::SearchInput(text) => self.search = text,
            AppMessage::OpenDiagnostics => {
                self.context_page = ContextPage::Diagnostics;
                self.core.window.show_context = true;
                self.diagnostics = None;
                self.diagnostics_status = None;
                let inputs = self.diagnostics_inputs();
                return cosmic::task::future(async move {
                    let text = off_ui(
                        move || diagnostics::build(&inputs),
                        "The report couldn't be built; see the log.".to_owned(),
                    )
                    .await;
                    Message::App(AppMessage::DiagnosticsReady(text))
                });
            }
            AppMessage::DiagnosticsReady(text) => self.diagnostics = Some(text),
            AppMessage::SaveDiagnostics => {
                // Saved exactly as shown.
                let Some(text) = self.diagnostics.clone() else {
                    return Task::none();
                };
                return cosmic::task::future(async move {
                    let dialog = cosmic::dialog::file_chooser::save::Dialog::new()
                        .title(fl!("diagnostics-save"))
                        .file_name("deskfeed-diagnostics.txt".to_owned());
                    let result = match dialog.save_file().await {
                        Ok(response) => match response.url().and_then(|u| u.to_file_path().ok()) {
                            Some(path) => {
                                off_ui(
                                    move || {
                                        diagnostics::save(&path, &text)
                                            .map(|()| path.display().to_string())
                                            .map_err(|e| Some(e.kind().to_string()))
                                    },
                                    Err(Some("task".into())),
                                )
                                .await
                            }
                            None => Err(Some("no file chosen".into())),
                        },
                        Err(cosmic::dialog::file_chooser::Error::Cancelled) => Err(None),
                        Err(e) => Err(Some(e.to_string())),
                    };
                    Message::App(AppMessage::DiagnosticsSaved(result))
                });
            }
            AppMessage::DiagnosticsSaved(result) => {
                self.diagnostics_status = match result {
                    Ok(path) => Some(fl!("diagnostics-saved", path = path)),
                    Err(Some(e)) => Some(fl!("diagnostics-not-saved", detail = e)),
                    Err(None) => None,
                };
            }
            AppMessage::ToggleFavorite(key) => {
                let mut favorites = self.config.favorites.clone();
                if !favorites.remove(&key) {
                    favorites.insert(key);
                }
                save_config!(self, favorites, set_favorites, favorites);
            }
            AppMessage::CheckDependencies => return self.check_dependencies(),
            AppMessage::DependenciesChecked(checks) => {
                for c in &checks {
                    if !c.status.is_ok() {
                        crate::app_log!("dependency {:?}: {:?}", c.dependency, c.status);
                    }
                }
                let missing = checks.iter().any(|c| !c.status.is_ok());
                self.deps = Some(checks);
                // Started hidden: a missing dependency needs the window.
                if missing {
                    return self.show_window();
                }
            }
            AppMessage::ToggleContextPage(page) => {
                if self.context_page == page {
                    self.core.window.show_context = !self.core.window.show_context;
                } else {
                    self.context_page = page;
                    self.core.window.show_context = true;
                }
            }
            AppMessage::UpdateConfig(config) => {
                // Saved connections changed elsewhere (another instance, or
                // the settings file edited): the list and sidebar follow.
                let connections_changed = config.connections != self.config.connections;
                self.config = *config;
                if connections_changed {
                    // Saving a form for a connection removed elsewhere
                    // would bring it back.
                    if let Some(id) = &self.connection_form.editing
                        && !self.config.connections.contains_key(id)
                    {
                        self.connection_form = ConnectionForm::default();
                    }
                    self.refresh_connection_rows();
                    let page = self.selected_page().unwrap_or(Page::Add);
                    self.rebuild_nav(&page);
                }
            }
            AppMessage::LaunchUrl(url) => {
                if let Err(err) = open::that_detached(&url) {
                    crate::app_log!("failed to open link: {err}");
                }
            }
            AppMessage::DismissNotice => self.notice = None,
            AppMessage::OpenLogFolder => {
                if let Some(dir) = crate::log::dir() {
                    // Make sure it exists even before the first line.
                    crate::app_log!("log folder opened");
                    if let Err(err) = open::that_detached(&dir) {
                        crate::app_log!("failed to open the log folder: {err}");
                    }
                }
            }
        }
        Task::none()
    }

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
            Page::Add | Page::Connections | Page::AddConnection => None,
        }
    }

    /// The account whose desktop list contains `key`.
    fn account_of_resource(&self, key: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.resource(key).is_some())
    }

    /// Shows the first account, or "Add account" when there is none.
    fn select_first_account(&mut self) {
        let select = self
            .accounts
            .first()
            .map_or(Page::Add, |a| Page::Account(a.id.clone()));
        self.rebuild_nav(&select);
    }

    /// Rebuilds the sidebar: accounts in order, the saved connections'
    /// page (when there are some), then "Add account" and "Add connection".
    /// `select` is shown, or the first entry when it isn't there.
    fn rebuild_nav(&mut self, select: &Page) {
        self.nav.clear();
        let mut entries: Vec<(String, &str, Page, bool)> = self
            .accounts
            .iter()
            .map(|a| {
                let page = Page::Account(a.id.clone());
                (a.label(), "avatar-default-symbolic", page, false)
            })
            .collect();
        if !self.config.connections.is_empty() {
            entries.push((
                fl!("connections"),
                "network-server-symbolic",
                Page::Connections,
                !self.accounts.is_empty(),
            ));
        }
        // With no account (first start, or after removing the only one)
        // there is nothing to add to: it's how you sign in.
        let add = if self.accounts.is_empty() {
            fl!("sign-in")
        } else {
            fl!("add-account")
        };
        let divider = !entries.is_empty();
        entries.push((add, "list-add-symbolic", Page::Add, divider));
        entries.push((
            fl!("add-connection"),
            "list-add-symbolic",
            Page::AddConnection,
            false,
        ));
        let shown = entries
            .iter()
            .position(|(_, _, page, _)| page == select)
            .unwrap_or(0);
        for (i, (text, icon, page, divider)) in entries.into_iter().enumerate() {
            let entry = self
                .nav
                .insert()
                .text(text)
                .icon(widget::icon::from_name(icon).icon())
                .divider_above(divider)
                .data(page);
            if i == shown {
                entry.activate();
            }
        }
    }

    fn check_dependencies(&mut self) -> Task<cosmic::Action<Message>> {
        self.deps = None;
        cosmic::task::future(async move {
            // A check that crashed reports every dependency as unusable,
            // never an empty (all-clear) list.
            let checks = off_ui(
                deps::check_all,
                deps::not_checked(&fl!("deps-check-failed")),
            )
            .await;
            Message::App(AppMessage::DependenciesChecked(checks))
        })
    }
}

/// Runs blocking work off the UI thread. If it panics, the panic is logged
/// and `crashed` is returned: a crash never passes for a normal result.
async fn off_ui<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, crashed: T) -> T {
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|e| {
        crate::app_log!("bug: background work failed: {e}");
        crashed
    })
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    Dependencies,
    AppSettings,
    Diagnostics,
    AccountSettings,
    DesktopSettings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
    Settings,
    Diagnostics,
    Dependencies,
    AddAccount,
    SignOut,
    OpenLogFolder,
    Quit,
}

/// Keyboard shortcuts, shown in the menu (`menu::items`) and handled in
/// `subscription`. Ctrl+Q quits, as in COSMIC's own apps (Files, Text
/// Editor, Terminal).
fn key_binds() -> HashMap<menu::KeyBind, MenuAction> {
    use menu::key_bind::Modifier;
    HashMap::from([(
        menu::KeyBind {
            modifiers: vec![Modifier::Ctrl],
            key: cosmic::iced::keyboard::Key::Character("q".into()),
        },
        MenuAction::Quit,
    )])
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::App(AppMessage::ToggleContextPage(ContextPage::About)),
            MenuAction::Settings => {
                Message::App(AppMessage::ToggleContextPage(ContextPage::AppSettings))
            }
            MenuAction::Diagnostics => Message::App(AppMessage::OpenDiagnostics),
            MenuAction::Dependencies => {
                Message::App(AppMessage::ToggleContextPage(ContextPage::Dependencies))
            }
            MenuAction::AddAccount => Message::Account(AccountMessage::AddAccount),
            MenuAction::SignOut => Message::Account(AccountMessage::SignOutSelected),
            MenuAction::OpenLogFolder => Message::App(AppMessage::OpenLogFolder),
            MenuAction::Quit => Message::Quit(QuitMessage::Chosen),
        }
    }
}

#[cfg(test)]
mod tests;
