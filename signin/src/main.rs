// SPDX-License-Identifier: MIT

//! `cosmic-ext-avd-signin`: the sign-in-only web view (spec §6, ADR 0007).
//!
//! Usage: `cosmic-ext-avd-signin --profile <dir>`, then one JSON request
//! line on stdin; one JSON reply line comes back on stdout. `<dir>` holds
//! this account's cookies and is created with mode 0700.
//!
//! Restrictions: HTTPS only, no pop-up windows, no downloads, no
//! permission prompts (camera, microphone, location, …), no developer
//! tools, no file access, no script bridge to the app, TLS errors are
//! fatal. The current host is always shown in the title bar.

mod protocol;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::{BufRead, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use webkit6::prelude::*;
use webkit6::{
    CookiePersistentStorage, LoadEvent, NavigationPolicyDecision, NetworkSession, PolicyDecision,
    PolicyDecisionType, ResponsePolicyDecision, Settings, WebView,
};

use protocol::{Navigation, Reply, ValidRequest};

/// When `silent_first` is set: show the window if the sign-in hasn't
/// finished this long after a page finished loading.
const SHOW_AFTER: Duration = Duration::from_millis(1500);

/// Reports the result once and stops the main loop.
type Finish = Rc<dyn Fn(Reply)>;

fn main() -> ExitCode {
    let Some(profile) = profile_arg() else {
        eprintln!("usage: cosmic-ext-avd-signin --profile <dir>");
        return ExitCode::from(2);
    };
    let Some(request) = read_request() else {
        return fail("bad_request", 2);
    };
    let data_dir = profile.join("data");
    let cache_dir = profile.join("cache");
    if [&profile, &data_dir, &cache_dir]
        .iter()
        .any(|d| private_dir(d).is_err())
    {
        return fail("profile_dir", 1);
    }
    if gtk::init().is_err() {
        return fail("no_display", 1);
    }

    let main_loop = glib::MainLoop::new(None, false);
    let done = Rc::new(Cell::new(false));
    let finish: Finish = {
        let main_loop = main_loop.clone();
        let done = done.clone();
        Rc::new(move |reply: Reply| {
            if !done.replace(true) {
                emit(&reply);
                main_loop.quit();
            }
        })
    };

    let session = network_session(&data_dir, &cache_dir);
    let webview = web_view(&session, &finish);
    let window = window(&webview, &request, &finish);
    if request.silent_first {
        show_when_needed(&webview, &window, &done);
    } else {
        window.present();
    }
    {
        let finish = finish.clone();
        glib::timeout_add_local_once(Duration::from_millis(request.timeout_ms), move || {
            finish(Reply::Timeout);
        });
    }
    cancel_when_app_closes_pipe(&finish);

    webview.load_uri(request.url.as_str());
    main_loop.run();
    ExitCode::SUCCESS
}

fn emit(reply: &Reply) {
    let line = serde_json::to_string(reply).expect("serializable");
    let mut out = std::io::stdout().lock();
    // If the app is gone there is nobody to tell; just exit.
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn fail(reason: &str, code: u8) -> ExitCode {
    emit(&Reply::Failed {
        reason: reason.into(),
    });
    ExitCode::from(code)
}

fn profile_arg() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(dir), None) if flag == "--profile" => Some(PathBuf::from(dir)),
        _ => None,
    }
}

fn read_request() -> Option<ValidRequest> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .take(16 * 1024 + 1)
        .read_line(&mut line)
        .ok()?;
    protocol::parse_request(line.trim_end()).ok()
}

/// Creates `dir` (and parents) and makes sure only we can use it.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    if !fs::symlink_metadata(dir)?.is_dir() {
        return Err(std::io::Error::other("profile path is not a directory"));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// This account's own storage: cookies persist in the profile; downloads
/// are cancelled.
fn network_session(data_dir: &Path, cache_dir: &Path) -> NetworkSession {
    let session = NetworkSession::new(data_dir.to_str(), cache_dir.to_str());
    if let Some(cookies) = session.cookie_manager() {
        let path = data_dir.join("cookies.sqlite");
        cookies.set_persistent_storage(&path.to_string_lossy(), CookiePersistentStorage::Sqlite);
    }
    session.connect_download_started(|_, download| download.cancel());
    session
}

fn web_view(session: &NetworkSession, finish: &Finish) -> WebView {
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
        .network_session(session)
        .settings(&settings)
        .build();
    {
        let finish = finish.clone();
        webview.connect_decide_policy(move |_, decision, kind| {
            decide_policy(decision, kind, &finish);
            true
        });
    }
    // No pop-up windows, no permission prompts.
    webview.connect_create(|_, _| None);
    webview.connect_permission_request(|_, request| {
        request.deny();
        true
    });
    {
        let finish = finish.clone();
        webview.connect_load_failed_with_tls_errors(move |_, _, _, _| {
            finish(Reply::Failed {
                reason: "tls_error".into(),
            });
            true
        });
    }
    webview
}

fn decide_policy(decision: &PolicyDecision, kind: PolicyDecisionType, finish: &Finish) {
    match kind {
        PolicyDecisionType::NavigationAction => {
            let uri = decision
                .downcast_ref::<NavigationPolicyDecision>()
                .and_then(NavigationPolicyDecision::navigation_action)
                .and_then(|a| a.request())
                .and_then(|r| r.uri())
                .map(|u| u.to_string())
                .unwrap_or_default();
            match protocol::decide(&uri) {
                Navigation::Allow => decision.use_(),
                Navigation::Block => decision.ignore(),
                Navigation::Finished(reply) => {
                    // Stop before the page loads, so the code never reaches
                    // Microsoft's warning page.
                    decision.ignore();
                    finish(reply);
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

/// The window; its title bar always shows the current host.
fn window(webview: &WebView, request: &ValidRequest, finish: &Finish) -> gtk::Window {
    let host_label = gtk::Label::new(None);
    host_label.add_css_class("dim-label");
    let title = if request.title.is_empty() {
        "Sign in"
    } else {
        &request.title
    };
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
    {
        let finish = finish.clone();
        window.connect_close_request(move |_| {
            finish(Reply::Cancelled);
            glib::Propagation::Proceed
        });
    }
    window
}

/// Silent first: show the window only if the user has to act, that is when
/// a page has finished loading and the sign-in still isn't done shortly
/// after.
fn show_when_needed(webview: &WebView, window: &gtk::Window, done: &Rc<Cell<bool>>) {
    let window = window.clone();
    let done = done.clone();
    let pending = Rc::new(RefCell::new(None::<glib::SourceId>));
    webview.connect_load_changed(move |_, event| {
        if event != LoadEvent::Finished || window.is_visible() {
            return;
        }
        if let Some(old) = pending.borrow_mut().take() {
            old.remove();
        }
        let window = window.clone();
        let done = done.clone();
        let pending_inner = pending.clone();
        let id = glib::timeout_add_local_once(SHOW_AFTER, move || {
            pending_inner.borrow_mut().take();
            if !done.get() {
                window.present();
            }
        });
        *pending.borrow_mut() = Some(id);
    });
}

/// The app closing our stdin (or exiting) cancels the sign-in, so this
/// process never outlives it. A thread blocks on stdin; the main loop checks
/// its flag.
fn cancel_when_app_closes_pipe(finish: &Finish) {
    let closed = Arc::new(AtomicBool::new(false));
    let flag = closed.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        let mut stdin = std::io::stdin().lock();
        while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
        flag.store(true, Ordering::SeqCst);
    });
    let finish = finish.clone();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        if closed.load(Ordering::SeqCst) {
            finish(Reply::Cancelled);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}
