// SPDX-License-Identifier: MIT

//! The restricted sign-in web view and its window, shared by both modes.
//!
//! Rules (spec §6): HTTPS only, no pop-up windows, no downloads, no
//! permission prompts, no developer tools, no file access, no script
//! bridge, TLS errors are fatal, and the current host is always shown.

use std::cell::{Cell, RefCell};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use webkit6::prelude::*;
use webkit6::{
    CookieManager, CookiePersistentStorage, LoadEvent, NavigationPolicyDecision, NetworkSession,
    PolicyDecision, PolicyDecisionType, ResponsePolicyDecision, Settings, WebView,
};

use crate::protocol::{self, BROKER_SCHEME, Navigation, RedirectTarget, Reply};
use crate::session_jar::{self, StoredCookie};

/// In silent mode: show the window if the sign-in hasn't finished this long
/// after a page finished loading (the user has to act).
const SHOW_AFTER: Duration = Duration::from_millis(1500);

type OnResult = Box<dyn FnOnce(Reply)>;

/// When the sign-in window may appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// At once.
    Now,
    /// Only if the sign-in hasn't finished shortly after a page loaded
    /// (the user has to act).
    IfNeeded,
    /// Never: a `prompt=none` request, which must not show anything.
    Never,
}

struct Inner {
    webview: WebView,
    cookies: Option<CookieManager>,
    /// Session cookies shared with other helper processes.
    jar: Option<PathBuf>,
    window: gtk::Window,
    target: RefCell<RedirectTarget>,
    on_result: RefCell<Option<OnResult>>,
    show: Cell<Show>,
    timeout: RefCell<Option<glib::SourceId>>,
    show_timer: RefCell<Option<glib::SourceId>>,
}

/// One sign-in at a time; [`SignInView::navigate`] starts it and the
/// callback receives the outcome exactly once.
#[derive(Clone)]
pub struct SignInView(Rc<Inner>);

/// Creates `dir` (and parents) and makes sure only we can use it.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    if !fs::symlink_metadata(dir)?.is_dir() {
        return Err(std::io::Error::other("profile path is not a directory"));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

impl SignInView {
    /// `profile` holds this account's cookies; it must already exist.
    pub fn new(profile: &Path, title: &str) -> Self {
        let data_dir = profile.join("data");
        let cache_dir = profile.join("cache");
        let session = NetworkSession::new(data_dir.to_str(), cache_dir.to_str());
        let cookies = session.cookie_manager();
        if let Some(cookies) = &cookies {
            let path = data_dir.join("cookies.sqlite");
            cookies
                .set_persistent_storage(&path.to_string_lossy(), CookiePersistentStorage::Sqlite);
        }
        session.connect_download_started(|_, download| download.cancel());

        let settings = Settings::new();
        settings.set_javascript_can_open_windows_automatically(false);
        settings.set_enable_developer_extras(false);
        settings.set_allow_file_access_from_file_urls(false);
        settings.set_allow_universal_access_from_file_urls(false);
        settings.set_enable_back_forward_navigation_gestures(false);
        settings.set_enable_webaudio(false);
        settings.set_enable_webgl(false);
        settings.set_enable_media(false);

        let webview = WebView::builder()
            .network_session(&session)
            .settings(&settings)
            .build();
        let window = build_window(&webview, title);
        let jar = cookies.as_ref().and_then(|_| session_jar::jar_dir(profile));
        let view = Self(Rc::new(Inner {
            webview,
            cookies,
            jar,
            window,
            target: RefCell::new(RedirectTarget::NativeClient),
            on_result: RefCell::new(None),
            show: Cell::new(Show::Now),
            timeout: RefCell::new(None),
            show_timer: RefCell::new(None),
        }));
        view.connect_signals();
        view
    }

    fn connect_signals(&self) {
        let inner = &self.0;
        {
            let view = self.clone();
            inner
                .webview
                .connect_decide_policy(move |_, decision, kind| {
                    view.decide_policy(decision, kind);
                    true
                });
        }
        // No pop-up windows, no permission prompts.
        inner.webview.connect_create(|_, _| None);
        inner.webview.connect_permission_request(|_, request| {
            request.deny();
            true
        });
        {
            let view = self.clone();
            inner
                .webview
                .connect_load_failed_with_tls_errors(move |_, _, _, _| {
                    view.finish(Reply::Failed {
                        reason: "tls_error".into(),
                    });
                    true
                });
        }
        // FreeRDP's broker redirect uses a scheme WebKit doesn't know.
        // Registering it makes sure the redirect is always seen here, even
        // if it arrives as an HTTP redirect.
        if let Some(context) = inner.webview.context() {
            let view = self.clone();
            context.register_uri_scheme(BROKER_SCHEME, move |request| {
                let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
                let target = view.0.target.borrow().clone();
                match protocol::decide_for(&uri, &target) {
                    Navigation::Finished(reply) => view.finish(reply),
                    _ => view.finish(Reply::Failed {
                        reason: "unexpected_redirect".into(),
                    }),
                }
                let mut err = glib::Error::new(glib::FileError::Perm, "blocked");
                request.finish_error(&mut err);
            });
        }
        {
            let view = self.clone();
            inner.window.connect_close_request(move |w| {
                view.finish(Reply::Cancelled);
                w.set_visible(false);
                glib::Propagation::Stop
            });
        }
        {
            let view = self.clone();
            inner.webview.connect_load_changed(move |_, event| {
                if event == LoadEvent::Finished {
                    view.schedule_show();
                }
            });
        }
    }

    fn decide_policy(&self, decision: &PolicyDecision, kind: PolicyDecisionType) {
        match kind {
            PolicyDecisionType::NavigationAction => {
                let uri = decision
                    .downcast_ref::<NavigationPolicyDecision>()
                    .and_then(NavigationPolicyDecision::navigation_action)
                    .and_then(|a| a.request())
                    .and_then(|r| r.uri())
                    .map(|u| u.to_string())
                    .unwrap_or_default();
                let target = self.0.target.borrow().clone();
                match protocol::decide_for(&uri, &target) {
                    Navigation::Allow => decision.use_(),
                    Navigation::Block => decision.ignore(),
                    Navigation::Finished(reply) => {
                        // Stop before the request is sent, so the code never
                        // reaches Microsoft's warning page.
                        decision.ignore();
                        self.finish(reply);
                    }
                }
            }
            PolicyDecisionType::Response => {
                // Anything the view can't display would be a download.
                match decision.downcast_ref::<ResponsePolicyDecision>() {
                    Some(r) if r.is_mime_type_supported() => decision.use_(),
                    _ => decision.ignore(),
                }
            }
            // New windows (pop-ups) and anything unknown.
            _ => decision.ignore(),
        }
    }

    /// Starts a sign-in. Returns `false` if one is already running.
    pub fn navigate(
        &self,
        url: &str,
        target: RedirectTarget,
        show: Show,
        timeout: Duration,
        on_result: impl FnOnce(Reply) + 'static,
    ) -> bool {
        let inner = &self.0;
        if inner.on_result.borrow().is_some() {
            return false;
        }
        *inner.target.borrow_mut() = target;
        *inner.on_result.borrow_mut() = Some(Box::new(on_result));
        inner.show.set(show);
        let view = self.clone();
        *inner.timeout.borrow_mut() = Some(glib::timeout_add_local_once(timeout, move || {
            view.0.timeout.borrow_mut().take();
            view.finish(Reply::Timeout);
        }));
        if show == Show::Now {
            inner.window.present();
        }
        self.restore_then_load(url);
        true
    }

    /// Loads the shared session cookies into this web view, then `url`.
    fn restore_then_load(&self, url: &str) {
        let inner = &self.0;
        let stored = inner
            .jar
            .as_deref()
            .map(session_jar::load)
            .unwrap_or_default();
        // Diagnostics (count only): did this sign-in start with the shared
        // Microsoft session? FreeRDP passes our stderr to the app's log.
        eprintln!(
            "cosmic-ext-deskfeed-signin: saved session cookies: {}",
            stored.len()
        );
        let Some(manager) = inner.cookies.clone().filter(|_| !stored.is_empty()) else {
            inner.webview.load_uri(url);
            return;
        };
        let pending = Rc::new(Cell::new(stored.len()));
        for cookie in &stored {
            let (view, pending, url) = (self.clone(), pending.clone(), url.to_owned());
            manager.add_cookie(&cookie.to_soup(), None::<&gio::Cancellable>, move |_| {
                pending.set(pending.get() - 1);
                // Only if this sign-in is still running (not timed out).
                if pending.get() == 0 && view.0.on_result.borrow().is_some() {
                    view.0.webview.load_uri(&url);
                }
            });
        }
    }

    /// Ends the running sign-in (if any) with `reply`.
    pub fn finish(&self, reply: Reply) {
        let inner = &self.0;
        let Some(callback) = inner.on_result.borrow_mut().take() else {
            return;
        };
        eprintln!(
            "cosmic-ext-deskfeed-signin: sign-in page shown to the user: {}",
            if inner.window.is_visible() {
                "yes"
            } else {
                "no"
            }
        );
        if let Some(id) = inner.timeout.borrow_mut().take() {
            id.remove();
        }
        if let Some(id) = inner.show_timer.borrow_mut().take() {
            id.remove();
        }
        inner.webview.stop_loading();
        inner.window.set_visible(false);
        // Save the session cookies before replying: the process may exit
        // right after the reply.
        match (inner.cookies.clone(), inner.jar.clone()) {
            (Some(manager), Some(jar)) => {
                manager.all_cookies(None::<&gio::Cancellable>, move |all| {
                    if let Ok(all) = all {
                        let session: Vec<StoredCookie> =
                            all.iter().filter_map(StoredCookie::from_soup).collect();
                        session_jar::save(&jar, &session);
                    }
                    callback(reply);
                });
            }
            _ => callback(reply),
        }
    }

    fn schedule_show(&self) {
        let inner = &self.0;
        if inner.show.get() != Show::IfNeeded
            || inner.window.is_visible()
            || inner.on_result.borrow().is_none()
        {
            return;
        }
        if let Some(old) = inner.show_timer.borrow_mut().take() {
            old.remove();
        }
        let view = self.clone();
        *inner.show_timer.borrow_mut() =
            Some(glib::timeout_add_local_once(SHOW_AFTER, move || {
                view.0.show_timer.borrow_mut().take();
                if view.0.on_result.borrow().is_some() {
                    view.0.window.present();
                }
            }));
    }
}

/// The window; its title bar always shows the current host.
fn build_window(webview: &WebView, title: &str) -> gtk::Window {
    let host_label = gtk::Label::new(None);
    host_label.add_css_class("dim-label");
    let title_label = gtk::Label::new(Some(title));
    title_label.add_css_class("title");
    let title_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    title_box.append(&title_label);
    title_box.append(&host_label);
    let header = gtk::HeaderBar::new();
    header.set_title_widget(Some(&title_box));

    let window = gtk::Window::builder()
        .title(title)
        .default_width(480)
        .default_height(640)
        .child(webview)
        .build();
    window.set_titlebar(Some(&header));

    webview.connect_uri_notify(move |view| {
        let host = view
            .uri()
            .and_then(|u| url::Url::parse(&u).ok())
            .and_then(|u| {
                let host = u.host_str()?.to_owned();
                Some(if u.scheme() == "https" {
                    format!("🔒 {host}")
                } else {
                    host
                })
            })
            .unwrap_or_default();
        host_label.set_text(&host);
    });
    window
}
