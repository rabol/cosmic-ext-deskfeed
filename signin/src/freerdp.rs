// SPDX-License-Identifier: MIT

//! FreeRDP helper mode: FreeRDP's newline-delimited JSON-RPC protocol over
//! two pipes (`client/common/aad-helper-protocol.md` in FreeRDP 3.32.2).
//!
//! FreeRDP sends `hello`, then `navigate` with an authorize URL and the
//! redirect to wait for; we answer with the redirect URL rebuilt from only
//! `code` and `state`. FreeRDP exchanges the code itself (including the
//! session host's proof-of-possession key). `shutdown`/`exit` end the
//! process, as does FreeRDP closing the pipe.

use std::fs::File;
use std::io::{BufReader, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use futures_channel::mpsc;
use futures_util::StreamExt as _;

use gtk::glib;
use serde_json::{Value, json};

use crate::protocol::{self, RedirectTarget, Reply};
use crate::view::{self, Show, SignInView};

use common::helper::{PROFILE_ENV, TITLE_ENV};

/// The title from `TITLE_ENV`: one line, printable, bounded; English
/// without it.
fn window_title() -> String {
    let title = protocol::clean_title(&std::env::var(TITLE_ENV).unwrap_or_default());
    if title.is_empty() {
        "Sign in".to_owned()
    } else {
        title
    }
}
const MAX_LINE: usize = 64 * 1024;
/// Messages waiting for the main loop; FreeRDP sends one at a time.
const MAX_QUEUED: usize = 16;
const DEFAULT_TIMEOUT_MS: u64 = 180_000;
const MAX_TIMEOUT_MS: u64 = 15 * 60 * 1000;
/// How long the hidden `prompt=none` attempt may take before the normal
/// sign-in is used instead.
const SILENT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, PartialEq, Eq)]
pub enum Incoming {
    Hello {
        id: u64,
    },
    Navigate {
        id: u64,
        url: String,
        target: RedirectTarget,
        redirect_uri: String,
        timeout_ms: u64,
    },
    /// A `navigate` we refuse (bad URL or redirect).
    BadNavigate {
        id: u64,
    },
    Shutdown {
        id: u64,
    },
    Exit,
    Cancel,
    /// A request we don't know; answered with an error.
    Unknown {
        id: u64,
    },
    /// Unparsable lines and unknown notifications are ignored, as FreeRDP
    /// does on its side.
    Ignore,
}

/// Parses `--cmdInFd=P<hex>`: `P` marks an anonymous pipe (WinPR's
/// `winpr_exportHandleToString`), followed by the fd in lowercase hex.
pub fn fd_arg(args: &[String], name: &str) -> Option<RawFd> {
    let value = args
        .iter()
        .find_map(|a| a.strip_prefix(name)?.strip_prefix('='))?;
    let hex = value.strip_prefix('P')?;
    let fd = i32::from_str_radix(hex, 16).ok()?;
    (fd > 2).then_some(fd)
}

pub fn parse_incoming(line: &str) -> Incoming {
    if line.len() > MAX_LINE {
        return Incoming::Ignore;
    }
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return Incoming::Ignore;
    };
    let id = msg.get("id").and_then(Value::as_u64);
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = msg.get("params");
    match (method, id) {
        ("hello", Some(id)) => Incoming::Hello { id },
        ("navigate", Some(id)) => {
            parse_navigate(id, params).unwrap_or(Incoming::BadNavigate { id })
        }
        ("shutdown", Some(id)) => Incoming::Shutdown { id },
        ("exit", None) => Incoming::Exit,
        ("cancel", None) => Incoming::Cancel,
        (_, Some(id)) => Incoming::Unknown { id },
        (_, None) => Incoming::Ignore,
    }
}

fn parse_navigate(id: u64, params: Option<&Value>) -> Option<Incoming> {
    let p = params?;
    let url = p.get("url")?.as_str()?;
    protocol::valid_start_url(url)?;
    let redirect_uri = p.get("redirect_uri")?.as_str()?.trim().to_owned();
    let target = RedirectTarget::from_freerdp(&redirect_uri)?;
    let timeout_ms = match p.get("timeout_ms").and_then(Value::as_u64).unwrap_or(0) {
        0 => DEFAULT_TIMEOUT_MS,
        t => t.min(MAX_TIMEOUT_MS),
    };
    Some(Incoming::Navigate {
        id,
        url: url.to_owned(),
        target,
        redirect_uri,
        timeout_ms,
    })
}

pub fn result(id: u64, value: &Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": value }).to_string()
}

pub fn error(id: u64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": 1, "message": message } }).to_string()
}

/// The `navigate` response for a sign-in outcome.
pub fn navigate_response(id: u64, redirect_uri: &str, reply: Reply) -> String {
    match reply {
        Reply::Ok { code, state, .. } => {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("code", &code)
                .append_pair("state", &state)
                .finish();
            result(
                id,
                &json!({ "status": "ok", "redirect_url": format!("{redirect_uri}?{query}") }),
            )
        }
        Reply::Error {
            error: e,
            error_codes,
            ..
        } => match error_codes {
            Some(codes) => error(id, &format!("{e}: {codes}")),
            None => error(id, &e),
        },
        Reply::Cancelled => error(id, "user_cancelled"),
        Reply::Timeout => error(id, "timeout"),
        Reply::Failed { reason } => error(id, &reason),
    }
}

/// Runs FreeRDP helper mode until FreeRDP says `exit` or closes the pipe.
pub fn run(args: &[String]) -> ExitCode {
    let (Some(in_fd), Some(out_fd)) = (fd_arg(args, "--cmdInFd"), fd_arg(args, "--cmdOutFd"))
    else {
        eprintln!("cosmic-ext-deskfeed-signin: missing --cmdInFd/--cmdOutFd");
        return ExitCode::from(2);
    };
    // SAFETY: FreeRDP passes us the two pipe ends for our exclusive use.
    let (cmd_in, cmd_out) = unsafe { (File::from_raw_fd(in_fd), File::from_raw_fd(out_fd)) };
    // Everything runs on the main loop: no lock needed.
    let send = move |line: String| {
        let mut out = &cmd_out;
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };

    let profile = std::env::var_os(PROFILE_ENV).map(std::path::PathBuf::from);
    let usable = profile
        .as_deref()
        .is_some_and(|p| p.is_absolute() && view::private_dir(p).is_ok());
    // Never without the app's proxy (`proxy.rs`).
    let proxy = crate::proxy::from_env();
    if !usable || proxy.is_err() || view::init_gtk().is_err() {
        // Answer hello with an error, so FreeRDP reports a clear failure
        // instead of waiting.
        serve_errors(cmd_in, &send);
        return ExitCode::from(1);
    }
    let profile = profile.expect("checked above");

    let mut messages = messages(cmd_in);
    let main_loop = glib::MainLoop::new(None, false);
    let view = SignInView::new(&profile, &window_title(), proxy.ok().flatten().as_ref());
    let send = Rc::new(send);
    // The account's name for Microsoft's page (`login_hint.rs`).
    let login_hint: Rc<Option<String>> = Rc::new(crate::login_hint::from_env());
    {
        let main_loop = main_loop.clone();
        glib::spawn_future_local(async move {
            while let Some(line) = messages.next().await {
                if !handle(&parse_incoming(&line), &view, &send, &login_hint) {
                    main_loop.quit();
                    return;
                }
            }
            // FreeRDP closed the pipe: stop.
            view.finish(Reply::Cancelled);
            main_loop.quit();
        });
    }
    main_loop.run();
    ExitCode::SUCCESS
}

/// FreeRDP's messages as they arrive. A thread reads the pipe, in lines of
/// at most `MAX_LINE` bytes (an oversized one can't be a valid request and
/// is dropped), and queues them; at most `MAX_QUEUED` wait, the thread
/// waiting while the queue is full. The stream ends when FreeRDP closes the
/// pipe.
fn messages(cmd_in: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (mut queue, messages) = mpsc::channel(MAX_QUEUED);
    std::thread::spawn(move || {
        for line in common::lines::lines(BufReader::new(cmd_in), MAX_LINE) {
            if line.too_long {
                continue;
            }
            let room = futures_executor::block_on(std::future::poll_fn(|cx| queue.poll_ready(cx)));
            // The main loop has stopped.
            if room.is_err() || queue.start_send(line.text).is_err() {
                return;
            }
        }
    });
    messages
}

/// Handles one message. Returns `false` when the helper should exit.
fn handle(
    msg: &Incoming,
    view: &SignInView,
    send: &Rc<impl Fn(String) + 'static>,
    login_hint: &Rc<Option<String>>,
) -> bool {
    match msg {
        Incoming::Hello { id } => send(result(
            *id,
            &json!({
                "protocol_version": 1,
                "helper": concat!("cosmic-ext-deskfeed-signin/", env!("CARGO_PKG_VERSION")),
            }),
        )),
        Incoming::Navigate {
            id,
            url,
            target,
            redirect_uri,
            timeout_ms,
        } => {
            let (id, redirect_uri, reply_send) = (*id, redirect_uri.clone(), send.clone());
            let respond = move |reply| reply_send(navigate_response(id, &redirect_uri, reply));
            // The account's name as a hint for Microsoft's page.
            let url = &crate::login_hint::prepare(url, login_hint.as_deref());
            let timeout = Duration::from_millis(*timeout_ms);
            // First without any page (`prompt=none`); the normal sign-in only
            // if Microsoft says it needs the user.
            let started = match protocol::with_prompt_none(url) {
                Some(silent_url) => {
                    let (view2, url, target2) = (view.clone(), url.clone(), target.clone());
                    view.navigate(
                        &silent_url,
                        target.clone(),
                        Show::Never,
                        SILENT_TIMEOUT,
                        move |reply| {
                            // Status only: never the code or token.
                            let outcome = match &reply {
                                Reply::Ok { .. } => "ok, no page shown".to_owned(),
                                Reply::Error { error, .. } => format!("error {error}"),
                                other => format!("{other:?}"),
                            };
                            eprintln!("cosmic-ext-deskfeed-signin: silent token request (prompt=none): {outcome}");
                            if protocol::needs_interaction(&reply) {
                                view2.navigate(&url, target2, Show::IfNeeded, timeout, respond);
                            } else {
                                respond(reply);
                            }
                        },
                    )
                }
                None => view.navigate(url, target.clone(), Show::IfNeeded, timeout, respond),
            };
            if !started {
                send(error(id, "navigate_already_in_progress"));
            }
        }
        Incoming::BadNavigate { id } => send(error(*id, "invalid_request")),
        Incoming::Shutdown { id } => {
            view.finish(Reply::Failed {
                reason: "shutting_down".into(),
            });
            send(result(*id, &Value::Null));
        }
        Incoming::Exit => {
            view.finish(Reply::Failed {
                reason: "shutting_down".into(),
            });
            return false;
        }
        Incoming::Cancel => view.finish(Reply::Cancelled),
        Incoming::Unknown { id } => send(error(*id, "unknown_method")),
        Incoming::Ignore => {}
    }
    true
}

/// Without a usable profile or display: answer every request with an error
/// until FreeRDP gives up.
fn serve_errors(cmd_in: File, send: &impl Fn(String)) {
    for line in common::lines::lines(BufReader::new(cmd_in), MAX_LINE) {
        if line.too_long {
            continue;
        }
        match parse_incoming(&line.text) {
            Incoming::Exit => break,
            Incoming::Hello { id }
            | Incoming::Navigate { id, .. }
            | Incoming::BadNavigate { id }
            | Incoming::Unknown { id } => send(error(id, "helper_unavailable")),
            Incoming::Shutdown { id } => send(result(id, &Value::Null)),
            Incoming::Cancel | Incoming::Ignore => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(messages: &mut mpsc::Receiver<String>) -> Option<String> {
        futures_executor::block_on(messages.next())
    }

    #[test]
    fn messages_arrive_in_order_without_oversized_ones_and_end_with_the_pipe() {
        let (read, mut write) = std::io::pipe().unwrap();
        let mut messages = messages(read);
        let long = "x".repeat(MAX_LINE + 1);
        writeln!(write, "one\n{long}\ntwo").unwrap();
        drop(write);
        assert_eq!(next(&mut messages).as_deref(), Some("one"));
        assert_eq!(next(&mut messages).as_deref(), Some("two"));
        assert_eq!(next(&mut messages), None, "the pipe closed");
    }

    #[test]
    fn a_full_queue_waits_and_loses_nothing() {
        let (read, mut write) = std::io::pipe().unwrap();
        let mut messages = messages(read);
        // Many more than MAX_QUEUED, with nobody reading yet.
        for i in 0..100 {
            writeln!(write, "{i}").unwrap();
        }
        drop(write);
        std::thread::sleep(Duration::from_millis(100));
        for i in 0..100 {
            assert_eq!(next(&mut messages), Some(i.to_string()));
        }
        assert_eq!(next(&mut messages), None);
    }

    #[test]
    fn the_reader_ends_when_the_main_loop_has_stopped() {
        let (read, mut write) = std::io::pipe().unwrap();
        drop(messages(read));
        // The thread stops and closes the pipe: writing fails soon.
        let started = std::time::Instant::now();
        while writeln!(write, "hello").is_ok() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "reader still running"
            );
        }
    }

    const AUTH: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=a85cf173-4192-42f8-81fa-777a763e6e2c&response_type=code";
    const BROKER: &str =
        "ms-appx-web://Microsoft.AAD.BrokerPlugin/a85cf173-4192-42f8-81fa-777a763e6e2c";

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn fd_args() {
        let a = args(&["--cmdInFd=P1a", "--cmdOutFd=P1b"]);
        assert_eq!(fd_arg(&a, "--cmdInFd"), Some(26));
        assert_eq!(fd_arg(&a, "--cmdOutFd"), Some(27));
        for bad in [
            "--cmdInFd=1a",
            "--cmdInFd=Pzz",
            "--cmdInFd=P1",
            "--cmdInFd=",
        ] {
            assert_eq!(fd_arg(&args(&[bad]), "--cmdInFd"), None, "{bad}");
        }
    }

    #[test]
    fn parses_protocol_messages() {
        assert_eq!(
            parse_incoming(
                r#"{"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol_version":1,"client":"freerdp"}}"#
            ),
            Incoming::Hello { id: 1 }
        );
        let nav = json!({"jsonrpc":"2.0","id":2,"method":"navigate","params":{"title":"Sign in","url":AUTH,"redirect_uri":BROKER,"timeout_ms":0}});
        match parse_incoming(&nav.to_string()) {
            Incoming::Navigate {
                id,
                timeout_ms,
                target,
                ..
            } => {
                assert_eq!(id, 2);
                assert_eq!(timeout_ms, DEFAULT_TIMEOUT_MS);
                assert!(matches!(target, RedirectTarget::Broker { .. }));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#),
            Incoming::Shutdown { id: 3 }
        );
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            Incoming::Exit
        );
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","method":"cancel"}"#),
            Incoming::Cancel
        );
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","id":4,"method":"frobnicate"}"#),
            Incoming::Unknown { id: 4 }
        );
        assert_eq!(parse_incoming("not json"), Incoming::Ignore);
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","method":"log","params":{}}"#),
            Incoming::Ignore
        );
    }

    #[test]
    fn refuses_navigate_to_other_sites_or_redirects() {
        for (url, redirect) in [
            ("https://evil.example/authorize", BROKER),
            ("http://login.microsoftonline.com/x", BROKER),
            (AUTH, "https://evil.example/cb"),
            (AUTH, "ms-appx-web://evil/x"),
        ] {
            let nav = json!({"jsonrpc":"2.0","id":9,"method":"navigate","params":{"url":url,"redirect_uri":redirect}});
            assert_eq!(
                parse_incoming(&nav.to_string()),
                Incoming::BadNavigate { id: 9 },
                "{url} {redirect}"
            );
        }
    }

    #[test]
    fn navigate_responses() {
        let ok = navigate_response(
            2,
            BROKER,
            Reply::Ok {
                code: "c.1-x".into(),
                state: "s1".into(),
            },
        );
        let v: Value = serde_json::from_str(&ok).unwrap();
        assert_eq!(v["id"], 2);
        assert_eq!(v["result"]["status"], "ok");
        assert_eq!(
            v["result"]["redirect_url"],
            format!("{BROKER}?code=c.1-x&state=s1")
        );

        let denied = navigate_response(
            2,
            BROKER,
            Reply::Error {
                error: "access_denied".into(),
                error_codes: Some("53003".into()),
                state: None,
            },
        );
        let v: Value = serde_json::from_str(&denied).unwrap();
        assert_eq!(v["error"]["message"], "access_denied: 53003");
        let v: Value =
            serde_json::from_str(&navigate_response(2, BROKER, Reply::Cancelled)).unwrap();
        assert_eq!(v["error"]["message"], "user_cancelled");
        let v: Value = serde_json::from_str(&navigate_response(2, BROKER, Reply::Timeout)).unwrap();
        assert_eq!(v["error"]["message"], "timeout");
    }

    #[test]
    fn responses_are_single_lines() {
        assert!(!result(1, &json!({"a": "b\nc"})).contains('\n'));
    }

    /// FreeRDP's helper protocol lines and its command line, with random and
    /// mangled input (ADR 0023).
    #[test]
    fn random_freerdp_messages_never_break_the_reader() {
        let hello = r#"{"jsonrpc":"2.0","id":1,"method":"hello"}"#;
        let nav = json!({"jsonrpc":"2.0","id":2,"method":"navigate","params":{"title":"Sign in","url":AUTH,"redirect_uri":BROKER,"timeout_ms":5000}}).to_string();
        common::testkit::random_inputs(
            "freerdp messages",
            &[hello.as_bytes(), nav.as_bytes()],
            |input| {
                let line = String::from_utf8_lossy(input);
                if let Incoming::Navigate {
                    url, timeout_ms, ..
                } = parse_incoming(&line)
                {
                    // Only Microsoft's sign-in page, and a bounded wait.
                    assert!(crate::protocol::valid_start_url(&url).is_some(), "{url}");
                    assert!(timeout_ms <= MAX_TIMEOUT_MS);
                }
            },
        );
        common::testkit::random_inputs(
            "helper arguments",
            &[b"--cmdInFd=P1a --cmdOutFd=P1b"],
            |input| {
                let text = String::from_utf8_lossy(input);
                let args: Vec<String> = text.split(' ').map(str::to_owned).collect();
                for name in ["--cmdInFd", "--cmdOutFd"] {
                    if let Some(fd) = fd_arg(&args, name) {
                        assert!(fd >= 0);
                    }
                }
            },
        );
    }
}
