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
use crate::service::icons;
use crate::sessions::launcher::{self, LaunchSpec, LogonSetting, SessionEnd, SessionEvent};
use crate::sessions::rdpfile::{CameraPolicy, ConnectionFile, RdpError};
use crate::settings::{self, Choice, ConnectionSettings, Limited, Number, Toggle};

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
    sign_out_asking: Option<String>,
    /// What the last sign-out couldn't remove.
    cleanup_retry: Option<CleanupRetry>,
    /// The last list check (monotonic and wall clock), to notice sleep.
    last_check: Option<(Instant, SystemTime)>,
    /// The search field's text; filters the desktop list.
    search: String,
    /// The tray, while background operation is on and a tray answers.
    tray: Option<crate::tray::Handle>,
    /// Why there is no tray, when background operation is on.
    tray_unavailable: Option<String>,
    /// The tray's Quit: really quit, not close to the background.
    quit_from_tray: bool,
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
    dialog: Option<PasswordDialog>,
    /// Password dialogs waiting their turn behind `dialog` (review F06:
    /// each resource keeps its own, none replaces another).
    waiting_dialogs: std::collections::VecDeque<PasswordDialog>,
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

/// Desktop lists are refreshed this often (spec §7: hourly)...
/// ...checked this often.
const REFRESH_CHECK: Duration = Duration::from_mins(5);

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
        crate::log::set_max_size_mib(config.log_max_mib.unwrap_or(crate::log::MAX_SIZE_MIB.2));

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
            desks: desk::Desks::default(),
            signing_out: HashSet::new(),
            sign_out_asking: None,
            cleanup_retry: None,
            last_check: None,
            search: String::new(),
            tray: None,
            tray_unavailable: None,
            quit_from_tray: false,
            tray_menu: None,
            autostart: crate::autostart::enabled(),
            diagnostics: None,
            diagnostics_status: None,
            cache: crate::cache::Store::user(),
            icons: HashMap::new(),
            quit: Quit::No,
            dialog: None,
            waiting_dialogs: std::collections::VecDeque::new(),
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
        app.rebuild_nav(&Page::Add);
        let title = app.update_title();
        let load = cosmic::task::future(async {
            let result = tokio::task::spawn_blocking(accounts::load_saved)
                .await
                .unwrap_or_else(|_| Err(fl!("error-generic")));
            Message::SavedLoaded(result)
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
                    save = save.on_press(Message::SaveDiagnostics);
                }
                context_drawer::context_drawer(
                    col,
                    Message::ToggleContextPage(ContextPage::Diagnostics),
                )
                .title(fl!("diagnostics"))
                .actions(save)
            }
            ContextPage::AppSettings => context_drawer::context_drawer(
                self.app_settings_view(),
                Message::ToggleContextPage(ContextPage::AppSettings),
            )
            .title(fl!("settings")),
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

    /// The window's close button.
    fn on_app_exit(&mut self) -> Option<Self::Message> {
        Some(Message::QuitRequested)
    }

    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        if self.quit == Quit::Asking {
            return Some(self.quit_dialog());
        }
        if let Some(id) = &self.sign_out_asking {
            return Some(self.sign_out_dialog(id));
        }
        if let Some(d) = &self.certificate_dialog {
            return Some(certificate::certificate_dialog(d));
        }
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
                widget::column::with_capacity(2)
                    .push(widget::text::caption(fl!("windows-user-name")))
                    .push(
                        widget::text_input(
                            fl!("windows-user-name-placeholder"),
                            d.username.as_str(),
                        )
                        .on_input(Message::WindowsUserInput),
                    )
                    .spacing(space.space_xxs),
            );
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
        if d.logon != LOGON_ENTRA {
            controls = controls.push(
                widget::checkbox(d.remember)
                    .label(fl!("remember-password"))
                    .on_toggle(Message::ToggleRememberPassword),
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
            col = col.push(
                widget::warning(fl!("deps-missing"))
                    .on_close(Message::ToggleContextPage(ContextPage::Dependencies)),
            );
        }
        if let Some(notice) = &self.notice {
            col = col.push(widget::warning(notice.clone()).on_close(Message::DismissNotice));
        }
        if self.cleanup_retry.is_some() {
            col = col.push(
                widget::row::with_capacity(2)
                    .push(
                        widget::container(
                            widget::warning(fl!("error-sign-out-incomplete"))
                                .on_close(Message::DismissSignOutCleanup),
                        )
                        .width(Length::Fill),
                    )
                    .push(
                        widget::button::standard(fl!("try-again"))
                            .on_press(Message::RetrySignOutCleanup),
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
                .map(|update| Message::UpdateConfig(update.config)),
            // Checks the wall clock, so lists are also refreshed soon after
            // a resume from suspend (the timer itself stops while
            // suspended).
            cosmic::iced::time::every(REFRESH_CHECK).map(|_| Message::RefreshDue),
            // Closing from the desktop (window menu, keyboard shortcut,
            // dock); the window's own close button goes to `on_app_exit`.
            cosmic::iced::window::close_requests().map(|_| Message::QuitRequested),
            // Full-screen sessions that keep the screen on (`awake.rs`).
            self.awake_subscription(),
            // Started again: show the window (`single.rs`).
            Subscription::run(crate::single::activations).map(Message::StartedAgain),
            if self.config.background {
                Subscription::run(tray_events).map(Message::Tray)
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

/// A message `route` sent to a handler that doesn't handle it: a bug.
/// Debug builds stop; release builds log the message's name only (its
/// fields may hold a password or token).
fn misrouted(message: &Message) {
    let text = format!("{message:?}");
    let name = text.split(['(', ' ', '{']).next().unwrap_or("?");
    crate::app_log!("bug: message {name} routed to the wrong handler");
    debug_assert!(false, "message routed to the wrong handler: {name}");
}

impl AppModel {
    /// Hands each message to the part of the app it belongs to.
    fn route(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::SavedLoaded(..)
            | Message::Restored(..)
            | Message::RetryRestore(..)
            | Message::AddAccount
            | Message::SignInAccount(..)
            | Message::CancelSignIn(..)
            | Message::SignInFinished(..)
            | Message::Saved(..)
            | Message::Refresh(..)
            | Message::RefreshDue
            | Message::Discovered(..)
            | Message::IconsLoaded(..)
            | Message::CacheLoaded(..)
            | Message::SignOut(..)
            | Message::RetrySave(..)
            | Message::SignOutSelected
            | Message::SignOutConfirm
            | Message::SignOutCancel
            | Message::SignOutFinished(..)
            | Message::RetrySignOutCleanup
            | Message::DismissSignOutCleanup
            | Message::SignOutFilesRemoved(..) => self.account_message(message),
            Message::Connect(..)
            | Message::ConnectionFileReady(..)
            | Message::PasswordInput(..)
            | Message::WindowsUserInput(..)
            | Message::TogglePasswordVisible
            | Message::LogonSelected(..)
            | Message::CustomDomainInput(..)
            | Message::DialogCancel
            | Message::DialogConnect
            | Message::Launched(..)
            | Message::Session(..)
            | Message::ConnectDeadline(..)
            | Message::ReconnectDue(..)
            | Message::ShowSession(..)
            | Message::ToggleRememberPassword(..)
            | Message::SavedPasswordLoaded(..)
            | Message::PasswordSaved(..)
            | Message::ForgetPassword(..)
            | Message::TrustCertificate
            | Message::CloseCertificateDialog
            | Message::ForgetCertificate(..)
            | Message::SessionShown(..)
            | Message::Disconnect(..)
            | Message::WorkspaceMoved(..)
            | Message::QuitRequested
            | Message::QuitToBackground
            | Message::QuitConfirm
            | Message::QuitCancel
            | Message::ReadyToQuit => self.connection_message(message),
            Message::OpenAccountSettings(..)
            | Message::OpenDesktopSettings(..)
            | Message::AppToggle(..)
            | Message::AppNumber(..)
            | Message::AppLogSize(..)
            | Message::AppTheme(..)
            | Message::AppBackground(..)
            | Message::AppAutostart(..)
            | Message::AppChoice(..)
            | Message::AccountChoice(..)
            | Message::DesktopChoice(..)
            | Message::AccountToggle(..)
            | Message::AccountNumber(..)
            | Message::DesktopToggle(..)
            | Message::DesktopNumber(..) => self.settings_message(message),
            other => self.app_message(other),
        }
    }

    /// App-wide messages: dependencies, the context drawer, links, notices, the log. Routed here by `update`.
    // One `match` over this handler's messages; splitting it would only
    // spread the routing out.
    #[allow(clippy::too_many_lines)]
    pub(super) fn app_message(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::Tray(event) => return self.tray_event(event),
            Message::Awake(event) => match event {
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
            Message::StartedAgain(token) => {
                crate::app_log!("started again: showing the window");
                return self.tray_event(crate::tray::TrayEvent::Show(token));
            }
            Message::SearchInput(text) => self.search = text,
            Message::OpenDiagnostics => {
                self.context_page = ContextPage::Diagnostics;
                self.core.window.show_context = true;
                self.diagnostics = None;
                self.diagnostics_status = None;
                let inputs = self.diagnostics_inputs();
                return cosmic::task::future(async move {
                    let text = tokio::task::spawn_blocking(move || diagnostics::build(&inputs))
                        .await
                        .unwrap_or_default();
                    Message::DiagnosticsReady(text)
                });
            }
            Message::DiagnosticsReady(text) => self.diagnostics = Some(text),
            Message::SaveDiagnostics => {
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
                            Some(path) => tokio::task::spawn_blocking(move || {
                                diagnostics::save(&path, &text)
                                    .map(|()| path.display().to_string())
                                    .map_err(|e| Some(e.kind().to_string()))
                            })
                            .await
                            .unwrap_or_else(|_| Err(Some("task".into()))),
                            None => Err(Some("no file chosen".into())),
                        },
                        Err(cosmic::dialog::file_chooser::Error::Cancelled) => Err(None),
                        Err(e) => Err(Some(e.to_string())),
                    };
                    Message::DiagnosticsSaved(result)
                });
            }
            Message::DiagnosticsSaved(result) => {
                self.diagnostics_status = match result {
                    Ok(path) => Some(fl!("diagnostics-saved", path = path)),
                    Err(Some(e)) => Some(fl!("diagnostics-not-saved", detail = e)),
                    Err(None) => None,
                };
            }
            Message::ToggleFavorite(key) => {
                let mut favorites = self.config.favorites.clone();
                if !favorites.remove(&key) {
                    favorites.insert(key);
                }
                match &self.config_handler {
                    Some(handler) => {
                        let _ = self.config.set_favorites(handler, favorites);
                    }
                    None => self.config.favorites = favorites,
                }
            }
            Message::CheckDependencies => return self.check_dependencies(),
            Message::DependenciesChecked(checks) => {
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
            // `update` routes only this handler's messages here.
            other => misrouted(&other),
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
            Page::Add => None,
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
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
            MenuAction::Settings => Message::ToggleContextPage(ContextPage::AppSettings),
            MenuAction::Diagnostics => Message::OpenDiagnostics,
            MenuAction::Dependencies => Message::ToggleContextPage(ContextPage::Dependencies),
            MenuAction::AddAccount => Message::AddAccount,
            MenuAction::SignOut => Message::SignOutSelected,
            MenuAction::OpenLogFolder => Message::OpenLogFolder,
        }
    }
}
