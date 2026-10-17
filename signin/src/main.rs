// SPDX-License-Identifier: MIT

//! `cosmic-ext-deskfeed-signin`: the sign-in-only web view (spec §6, ADR 0007).
//!
//! Two modes:
//!
//! - **App mode:** `cosmic-ext-deskfeed-signin --profile <dir>`, one JSON request
//!   line on stdin, one JSON reply line on stdout. Closing stdin cancels.
//! - **FreeRDP mode:** started by FreeRDP through `/azure:auth-helper:<path>`
//!   with `--cmdInFd=…` and `--cmdOutFd=…`, speaking FreeRDP's helper
//!   protocol (ADR 0004). The profile comes from the environment variable
//!   `COSMIC_EXT_DESKFEED_SIGNIN_PROFILE`, which the app sets for FreeRDP, so the
//!   account's sign-in cookies make these requests silent when possible.

#[path = "../../src/bounded_lines.rs"]
mod bounded_lines;
mod freerdp;
mod protocol;
mod session_jar;
mod view;

use std::cell::Cell;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gtk::glib;

use protocol::{RedirectTarget, Reply, ValidRequest};
use view::{Show, SignInView};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Dependency check (app's `deps.rs`): reaching this line means the
    // process and its libraries (GTK, WebKitGTK) loaded.
    if args == ["--check"] {
        println!("ok");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a.starts_with("--cmdInFd=")) {
        return freerdp::run(&args);
    }
    let Some(profile) = profile_arg(&args) else {
        eprintln!("usage: cosmic-ext-deskfeed-signin --profile <dir>");
        return ExitCode::from(2);
    };
    let Some(request) = read_request() else {
        return fail("bad_request", 2);
    };
    if view::private_dir(&profile).is_err() {
        return fail("profile_dir", 1);
    }
    if gtk::init().is_err() {
        return fail("no_display", 1);
    }
    run_app_mode(&profile, &request)
}

fn run_app_mode(profile: &std::path::Path, request: &ValidRequest) -> ExitCode {
    let main_loop = glib::MainLoop::new(None, false);
    let title = if request.title.is_empty() {
        "Sign in"
    } else {
        &request.title
    };
    let view = SignInView::new(profile, title);
    let done = Rc::new(Cell::new(false));
    {
        let main_loop = main_loop.clone();
        let done = done.clone();
        view.navigate(
            request.url.as_str(),
            RedirectTarget::NativeClient,
            if request.silent_first {
                Show::IfNeeded
            } else {
                Show::Now
            },
            Duration::from_millis(request.timeout_ms),
            move |reply| {
                done.set(true);
                emit(&reply);
                main_loop.quit();
            },
        );
    }
    cancel_when_app_closes_pipe(&view);
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

fn profile_arg(args: &[String]) -> Option<PathBuf> {
    match args {
        [flag, dir] if flag == "--profile" => Some(PathBuf::from(dir)),
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

/// The app closing our stdin (or exiting) cancels the sign-in, so this
/// process never outlives it. A thread blocks on stdin; the main loop checks
/// its flag.
fn cancel_when_app_closes_pipe(view: &SignInView) {
    let closed = Arc::new(AtomicBool::new(false));
    let flag = closed.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        let mut stdin = std::io::stdin().lock();
        while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
        flag.store(true, Ordering::SeqCst);
    });
    let view = view.clone();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        if closed.load(Ordering::SeqCst) {
            view.finish(Reply::Cancelled);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}
