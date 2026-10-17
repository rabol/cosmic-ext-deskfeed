// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cosmic::app::context_drawer;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::alignment::{Horizontal, Vertical};
use cosmic::iced::{Alignment, Length, Subscription};
use cosmic::prelude::*;
use cosmic::widget::{self, about::About, menu};

use crate::auth::attempt::ClientConfig;
use crate::auth::signin::{self, Cancel, SignInError, SignedIn};
use crate::auth::token::TokenError;
use crate::config::Config;
use crate::fl;
use crate::service::discovery::{self, FetchError};
use crate::service::endpoint::EndpointPolicy;
use crate::service::feed::ResourceKind;
use crate::service::http::{Http, HttpError};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const APP_ICON: &[u8] = include_bytes!("../resources/icons/hicolor/scalable/apps/icon.svg");

pub struct AppModel {
    core: cosmic::Core,
    context_page: ContextPage,
    about: About,
    key_binds: HashMap<menu::KeyBind, MenuAction>,
    config: Config,
    http: Arc<Http>,
    session: Session,
    /// Increases on every sign-in and sign-out; results tagged with an older
    /// value are stale and ignored (spec §8).
    generation: u64,
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
        notice: Option<String>,
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
    id: String,
    title: String,
    kind: ResourceKind,
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
}

/// Sign-in failures, cheap to clone for messages.
#[derive(Debug, Clone)]
pub struct SignInFailure(Option<String>);

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
        let about = About::default()
            .name(fl!("app-title"))
            .icon(widget::icon::from_svg_bytes(APP_ICON))
            .version(env!("CARGO_PKG_VERSION"))
            .links([(fl!("repository"), REPOSITORY)])
            .license(env!("CARGO_PKG_LICENSE"));

        let mut app = AppModel {
            core,
            context_page: ContextPage::default(),
            about,
            key_binds: HashMap::new(),
            config: cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
                .map(|context| match Config::get_entry(&context) {
                    Ok(config) | Err((_, config)) => config,
                })
                .unwrap_or_default(),
            http: Arc::new(Http::new(EndpointPolicy::azure_public())),
            session: Session::SignedOut { error: None },
            generation: 0,
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

    fn view(&self) -> Element<'_, Self::Message> {
        let content: Element<_> = match &self.session {
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
                notice,
                ..
            } => resources_view(*loading, workspaces, error.as_deref(), notice.as_deref()),
        };
        content
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
                            notice: None,
                        };
                        return self.load_resources();
                    }
                    Err(SignInFailure(error)) => self.session = Session::SignedOut { error },
                }
            }
            Message::Refresh => return self.load_resources(),
            Message::Discovered(generation, result) => {
                if generation != self.generation {
                    return Task::none();
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
            Message::SignOut => self.sign_out(),
            Message::Connect(_id) => {
                if let Session::Ready { notice, .. } = &mut self.session {
                    *notice = Some(fl!("connect-not-ready"));
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
            let result = tokio::task::spawn_blocking(move || {
                discovery::discover(&http, &account.tokens.access_token)
            })
            .await;
            let result = match result {
                Ok(Ok(feeds)) => Ok(feeds.into_iter().map(workspace_view).collect()),
                Ok(Err(e)) => Err(describe_fetch_error(&e)),
                Err(_) => Err(fl!("error-generic")),
            };
            Message::Discovered(generation, result)
        })
    }

    fn sign_out(&mut self) {
        // Invalidate everything in flight, then forget the account and its
        // sign-in cookies (spec §6).
        self.generation += 1;
        if let Session::SigningIn { cancel } = &self.session {
            cancel.cancel();
        }
        let mut error = None;
        if let Some(dir) = signin_profile_dir()
            && dir.exists()
            && std::fs::remove_dir_all(&dir).is_err()
        {
            error = Some(fl!("error-sign-out-incomplete"));
        }
        self.session = Session::SignedOut { error };
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

fn workspace_view(feed: discovery::WorkspaceFeed) -> WorkspaceView {
    let fallback = feed
        .tenant
        .workspace_name
        .clone()
        .or_else(|| feed.tenant.tenant_display_name.clone())
        .unwrap_or_else(|| fl!("unnamed-workspace"));
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
                    id: r.id,
                    title: r.title,
                    kind: r.kind,
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
        ) => fl!("error-network"),
        FetchError::Http(HttpError::Tls) => fl!("error-tls"),
        other => fl!("error-feed", detail = other.to_string()),
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

fn resources_view<'a>(
    loading: bool,
    workspaces: &'a [WorkspaceView],
    error: Option<&'a str>,
    notice: Option<&'a str>,
) -> Element<'a, Message> {
    let space = cosmic::theme::spacing();
    let mut col = widget::column::with_capacity(workspaces.len() + 3).spacing(space.space_m);
    if loading {
        col = col.push(widget::text::body(fl!("loading-desktops")));
    }
    if let Some(error) = error {
        col = col.push(widget::text::body(error.to_owned()));
    }
    if let Some(notice) = notice {
        col = col.push(widget::text::body(notice.to_owned()));
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
            let kind = match r.kind {
                ResourceKind::Desktop => fl!("kind-desktop"),
                ResourceKind::RemoteApp => fl!("kind-remoteapp"),
                ResourceKind::Other => fl!("kind-other"),
            };
            let mut connect = widget::button::suggested(fl!("connect"));
            if r.kind == ResourceKind::Desktop {
                connect = connect.on_press(Message::Connect(r.id.clone()));
            }
            section = section.add(
                widget::settings::item::builder(r.title.clone())
                    .description(kind)
                    .control(connect),
            );
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
