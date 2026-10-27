// SPDX-License-Identifier: MIT

//! Checks with real FreeRDP windows in a nested COSMIC session, against
//! research/rdp-test-server: `just session-checks` (scripts/session-checks.sh)
//! starts both and runs each check by name. They took over the parts of
//! manual tests 6, 21, 23, 26 and 27 that don't need Microsoft, Windows or a
//! second monitor (docs/manualtest/README.md).
//!
//! They switch workspaces and open windows, so they refuse to run outside
//! that nested session (`DESKFEED_NESTED=1`).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::tests::{TEST_APP_ID, for_test_server, start_real, test_server_spec};
use super::{Session, SessionEvent, stop_all};
use crate::settings::{DEFAULTS, Effective, network};
use crate::workspace::observe::{Look, Observer};

/// How long a local connection may take; the test server answers at once.
const CONNECT: Duration = Duration::from_secs(30);
/// How long COSMIC may take to show or change a window.
const SHOW: Duration = Duration::from_secs(10);

/// The test server's socket and log; refuses to run on a real session.
fn nested() -> (String, PathBuf) {
    assert_eq!(
        std::env::var("DESKFEED_NESTED").as_deref(),
        Ok("1"),
        "only inside the nested session: just session-checks"
    );
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name}"));
    (
        var("DESKFEED_TEST_SERVER"),
        PathBuf::from(var("DESKFEED_TEST_SERVER_LOG")),
    )
}

/// A connected session; ended when dropped.
struct Running {
    session: Session,
    rx: std::sync::mpsc::Receiver<SessionEvent>,
    _dir: tempfile::TempDir,
}

impl Drop for Running {
    fn drop(&mut self) {
        stop_all(std::slice::from_ref(&self.session), super::STOP_GRACE);
    }
}

/// Starts a session with `title` and `settings` and waits until it is
/// connected.
fn connect(title: &str, settings: Effective) -> Running {
    let (socket, _) = nested();
    connect_to(&socket, title, settings)
}

/// [`connect`] to the test server at `socket`.
fn connect_to(socket: &str, title: &str, settings: Effective) -> Running {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("run")).unwrap();
    let spec = test_server_spec(socket, title, settings);
    let (session, rx) = start_real(&spec, dir.path(), for_test_server(true));
    let until = Instant::now() + CONNECT;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(SessionEvent::Connected) => break,
            Ok(SessionEvent::Stage(_)) => {}
            other => panic!("{title}: not connected: {other:?}"),
        }
    }
    Running {
        session,
        rx,
        _dir: dir,
    }
}

/// Waits until COSMIC lists `title`'s window on a workspace.
fn shown(observer: &mut Observer, title: &str) -> Look {
    let look = observer.until(SHOW, |l| {
        l.window(title).is_some_and(|w| !w.workspaces.is_empty())
    });
    assert!(look.window(title).is_some(), "{title}: no window: {look:?}");
    look
}

/// A window on the workspace in use, so it isn't empty; returns that
/// workspace's name.
fn occupy(observer: &mut Observer, title: &str, settings: Effective) -> (Running, String) {
    let running = connect(title, settings);
    let look = shown(observer, title);
    let active = look.active.clone().expect("an active workspace");
    assert_eq!(
        look.window(title).unwrap().workspaces,
        std::slice::from_ref(&active)
    );
    (running, active)
}

/// Test 26/27: with other windows on the workspace in use, COSMIC switches
/// to an empty one as the connection starts, and the desktop's window opens
/// there without showing on the old one first.
#[test]
#[ignore = "inside the nested COSMIC session: just session-checks"]
fn nested_new_workspace_without_flash() {
    nested();
    let mut observer = Observer::new();
    let (_other, first) = occupy(&mut observer, "Other window", DEFAULTS);

    // As the app does (`app/launch.rs`): the move starts, and FreeRDP once
    // the switch is done.
    let title = "New window";
    let cancel = Arc::new(AtomicBool::new(false));
    let (switched, gate) = std::sync::mpsc::channel::<()>();
    let mover = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            crate::workspace::move_to_new_workspace(
                TEST_APP_ID,
                title,
                CONNECT,
                &cancel,
                move || {
                    let _ = switched.send(());
                },
            )
        })
    };
    let _ = gate.recv_timeout(crate::workspace::SWITCH_GATE);
    let _new = connect(title, DEFAULTS);
    // Every workspace the window is seen on until the move is done.
    let mut seen = BTreeSet::new();
    while !mover.is_finished() {
        if let Some(w) = observer.look().window(title) {
            seen.extend(w.workspaces.iter().cloned());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let placed = mover.join().unwrap();
    let end = observer.look();
    println!("placed: {placed:?}; seen on {seen:?}; {end:?}");
    assert!(
        matches!(placed, Ok(crate::workspace::Placed::OpenedThere)),
        "{placed:?}"
    );
    assert!(!seen.contains(&first), "shown on {first} first: {seen:?}");
    let window = end.window(title).unwrap();
    assert_ne!(window.workspaces, std::slice::from_ref(&first));
    assert_eq!(Some(&window.workspaces[0]), end.active.as_ref());
    assert!(window.activated, "not focused: {end:?}");
}

/// Test 27 step 4: when no window comes, COSMIC switches back to the
/// workspace the user was on.
#[test]
#[ignore = "inside the nested COSMIC session: just session-checks"]
fn nested_switch_back_when_nothing_opens() {
    nested();
    let mut observer = Observer::new();
    let (_other, first) = occupy(&mut observer, "Other window", DEFAULTS);
    let cancel = Arc::new(AtomicBool::new(false));
    let result = crate::workspace::move_to_new_workspace(
        TEST_APP_ID,
        "A window that never opens",
        Duration::from_secs(3),
        &cancel,
        || {},
    );
    let end = observer.look();
    println!("result: {result:?}; {end:?}");
    assert!(
        matches!(result, Err(crate::workspace::MoveError::WindowNotFound)),
        "{result:?}"
    );
    assert_eq!(end.active, Some(first));
}

/// The Full screen setting: on, COSMIC shows the window full screen; off,
/// it doesn't.
#[test]
#[ignore = "inside the nested COSMIC session: just session-checks"]
fn nested_full_screen_follows_the_setting() {
    nested();
    let mut observer = Observer::new();
    {
        let title = "Windowed";
        let _s = connect(title, DEFAULTS);
        shown(&mut observer, title);
        // FreeRDP asks for full screen right after mapping, if at all.
        std::thread::sleep(Duration::from_secs(2));
        let look = observer.look();
        assert!(!look.window(title).unwrap().fullscreen, "{look:?}");
    }
    let title = "Full screen";
    let _s = connect(
        title,
        Effective {
            fullscreen: true,
            ..DEFAULTS
        },
    );
    let look = observer.until(SHOW, |l| l.window(title).is_some_and(|w| w.fullscreen));
    assert!(look.window(title).is_some_and(|w| w.fullscreen), "{look:?}");
}

/// Test 23: window size, color depth and network type reach the desktop.
/// The test server logs what the client asked for at activation.
#[test]
#[ignore = "inside the nested COSMIC session: just session-checks"]
fn nested_window_size_depth_network() {
    let (_, log) = nested();
    let mut observer = Observer::new();
    let output = observer.look().output.expect("an output size");
    let from = std::fs::metadata(&log).map_or(0, |m| m.len());
    let title = "Size, depth, network";
    let _s = connect(
        title,
        Effective {
            window_size: 50,
            color_depth: 16,
            network: network::LAN,
            ..DEFAULTS
        },
    );
    shown(&mut observer, title);
    let text = std::fs::read(&log).unwrap();
    let text = String::from_utf8_lossy(&text[usize::try_from(from).unwrap()..]);
    let asked = text
        .lines()
        .find_map(|l| {
            l.split_once("DESKFEED-TEST client asked for ")
                .map(|(_, r)| r)
        })
        .unwrap_or_else(|| panic!("no request logged:\n{text}"));
    println!("output {output:?}; asked for {asked}");
    let (size, rest) = asked.split_once(", ").unwrap();
    let (w, h) = size.split_once('x').unwrap();
    let (w, h): (i32, i32) = (w.parse().unwrap(), h.parse().unwrap());
    // Half the monitor each way; FreeRDP rounds.
    assert!((w - output.0 / 2).abs() <= 8, "width {w} for {output:?}");
    assert!((h - output.1 / 2).abs() <= 8, "height {h} for {output:?}");
    assert!(rest.starts_with("16 bpp, "), "{asked}");
    // MS-RDPBCGR `CONNECTION_TYPE_LAN`.
    assert!(rest.contains("connection type 6,"), "{asked}");
}

/// Test 21: when the machine goes away mid-session, FreeRDP's end is read
/// as a lost connection, which is what the app reconnects after
/// (`app/decide.rs` `reconnects`). Uses a test server of its own, stopped
/// here, so the others' server keeps running.
#[test]
#[ignore = "inside the nested COSMIC session: just session-checks"]
fn nested_lost_connection_is_recognised() {
    nested();
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name}"));
    let port = 33898;
    let socket = format!("/tmp/tfreerdp-server.{port}");
    let _ = std::fs::remove_file(&socket);
    let mut server = std::process::Command::new(var("DESKFEED_TEST_SERVER_BIN"))
        .current_dir(var("DESKFEED_TEST_SERVER_DIR"))
        .args([
            "--local-only",
            &format!("--port={port}"),
            "--cert=test.crt",
            "--key=test.key",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !std::path::Path::new(&socket).exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    let running = connect_to(&socket, "Lost connection", DEFAULTS);
    server.kill().unwrap();
    let _ = server.wait();
    let ended = loop {
        match running.rx.recv_timeout(Duration::from_secs(20)) {
            Ok(SessionEvent::Stage(_) | SessionEvent::Connected) => {}
            other => break other,
        }
    };
    println!("{ended:?}");
    assert!(
        matches!(
            ended,
            Ok(SessionEvent::Ended {
                end: super::SessionEnd::ConnectionLost,
                ..
            })
        ),
        "{ended:?}"
    );
}

/// The `org.freedesktop.ScreenSaver` cosmic-idle serves, on the nested
/// session's own bus: counts what Deskfeed asks.
#[derive(Default, Clone)]
struct FakeScreenSaver {
    held: Arc<std::sync::Mutex<Vec<u32>>>,
    asked: Arc<std::sync::atomic::AtomicU32>,
    /// The application name each `Inhibit` gave.
    callers: Arc<std::sync::Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl FakeScreenSaver {
    fn inhibit(&self, application: &str, reason: &str) -> u32 {
        println!("Inhibit({application:?}, {reason:?})");
        self.callers.lock().unwrap().push(application.to_owned());
        let cookie = self.asked.fetch_add(1, Ordering::SeqCst) + 1;
        self.held.lock().unwrap().push(cookie);
        cookie
    }

    fn un_inhibit(&self, cookie: u32) {
        self.held.lock().unwrap().retain(|c| *c != cookie);
    }
}

/// Test 6: the screen is held on while a full-screen desktop is in front,
/// and let go when it ends; a desktop in a window doesn't hold it.
/// (That COSMIC's idle daemon then doesn't blank the screen is COSMIC's.)
#[test]
#[ignore = "inside the nested COSMIC session: just session-checks"]
fn nested_keep_awake() {
    nested();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let fake = FakeScreenSaver::default();
    let _server = rt
        .block_on(
            zbus::connection::Builder::session()
                .unwrap()
                .name("org.freedesktop.ScreenSaver")
                .unwrap()
                .serve_at("/org/freedesktop/ScreenSaver", fake.clone())
                .unwrap()
                .build(),
        )
        .unwrap();
    let held = || fake.held.lock().unwrap().len();
    let wait_held = |want: usize| {
        let until = Instant::now() + SHOW;
        while held() != want && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        held()
    };

    let (windowed, full) = ("Awake windowed", "Awake full screen");
    let titles = vec![windowed.to_owned(), full.to_owned()];
    let alive = Arc::new(AtomicBool::new(true));
    let (tx, _rx) = cosmic::iced::futures::channel::mpsc::unbounded();
    let bus = rt
        .block_on(zbus::Connection::session())
        .ok()
        .map(|c| (rt.handle().clone(), c));
    let watcher = {
        let alive = alive.clone();
        std::thread::spawn(move || crate::awake::run(TEST_APP_ID, &titles, &alive, &tx, bus))
    };

    let mut observer = Observer::new();
    {
        let _s = connect(windowed, DEFAULTS);
        shown(&mut observer, windowed);
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(held(), 0, "held for a desktop in a window");
    }
    let session = connect(
        full,
        Effective {
            fullscreen: true,
            ..DEFAULTS
        },
    );
    assert_eq!(wait_held(1), 1, "not held in full screen");
    drop(session);
    assert_eq!(wait_held(0), 0, "still held after the desktop ended");
    assert_eq!(fake.asked.load(Ordering::SeqCst), 1);
    assert_eq!(*fake.callers.lock().unwrap(), ["Deskfeed"]);
    alive.store(false, Ordering::Relaxed);
    watcher.join().unwrap();
}

#[test]
fn refuses_outside_the_nested_session() {
    if std::env::var("DESKFEED_NESTED").is_ok() {
        return;
    }
    assert!(std::panic::catch_unwind(nested).is_err());
}
