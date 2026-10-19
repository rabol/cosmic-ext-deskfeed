// SPDX-License-Identifier: MIT

//! "Keep the screen on in full screen" (spec §9, ADR 0019): while a session
//! started with the setting on is focused and full screen, Deskfeed asks
//! COSMIC not to blank the screen, lock or suspend.
//!
//! COSMIC's idle daemon (cosmic-idle) serves `org.freedesktop.ScreenSaver`.
//! `Inhibit` holds back screen-off and suspend until `UnInhibit`, or until
//! the caller's bus connection closes, so a crashed Deskfeed never leaves
//! the screen on. FreeRDP doesn't ask for this itself.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cosmic::iced::futures::channel::mpsc;

const SCREENSAVER: &str = "org.freedesktop.ScreenSaver";
const SCREENSAVER_PATH: &str = "/org/freedesktop/ScreenSaver";

/// For the log.
#[derive(Debug, Clone)]
pub enum AwakeEvent {
    /// The screen is kept on (true) or may blank again (false).
    Held(bool),
    /// Watching or asking failed; nothing is held.
    Unavailable(String),
}

/// Holds the screen on while a window with one of `titles` (Deskfeed's
/// app ID) is focused and full screen. For `Subscription::run_with`: it
/// runs while the titles stay the same, and lets go when dropped.
#[allow(clippy::ptr_arg)] // `Subscription::run_with` hands over `&Vec<String>`
pub fn watch(
    titles: &Vec<String>,
) -> impl cosmic::iced::futures::Stream<Item = AwakeEvent> + use<> {
    let titles = titles.clone();
    cosmic::iced::stream::channel(4, move |mut out: mpsc::Sender<AwakeEvent>| async move {
        use cosmic::iced::futures::{SinkExt, StreamExt};
        let (tx, mut rx) = mpsc::unbounded();
        let alive = Arc::new(AtomicBool::new(true));
        let thread_alive = alive.clone();
        let spawned = std::thread::Builder::new()
            .name("deskfeed-awake".into())
            .spawn(move || {
                run(
                    crate::app::APP_ID,
                    &titles,
                    &thread_alive,
                    &tx,
                    crate::single::bus(),
                )
            });
        if spawned.is_ok() {
            // Ends the thread when the subscription is dropped.
            let _guard = Stop(alive);
            while let Some(e) = rx.next().await {
                if out.send(e).await.is_err() {
                    return;
                }
            }
        }
        std::future::pending::<()>().await;
    })
}

/// Stops the watcher thread when the subscription goes.
struct Stop(Arc<AtomicBool>);

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

fn run(
    app_id: &str,
    titles: &[String],
    alive: &AtomicBool,
    events: &mpsc::UnboundedSender<AwakeEvent>,
    bus: Option<(tokio::runtime::Handle, zbus::Connection)>,
) {
    let Some((rt, conn)) = bus else {
        let _ = events.unbounded_send(AwakeEvent::Unavailable("no session bus".into()));
        return;
    };
    let mut cookie: Option<u32> = None;
    let result = crate::workspace::watch_fullscreen_focus(
        app_id,
        titles,
        || alive.load(Ordering::Relaxed) && !events.is_closed(),
        |hold| {
            if hold && cookie.is_none() {
                let reply = rt.block_on(conn.call_method(
                    Some(SCREENSAVER),
                    SCREENSAVER_PATH,
                    Some(SCREENSAVER),
                    "Inhibit",
                    &("Deskfeed", "A full-screen desktop is in use"),
                ));
                match reply.and_then(|m| m.body().deserialize::<u32>()) {
                    Ok(c) => {
                        cookie = Some(c);
                        let _ = events.unbounded_send(AwakeEvent::Held(true));
                    }
                    Err(e) => {
                        let _ = events.unbounded_send(AwakeEvent::Unavailable(e.to_string()));
                    }
                }
            } else if !hold && let Some(c) = cookie.take() {
                release(&rt, &conn, c);
                let _ = events.unbounded_send(AwakeEvent::Held(false));
            }
        },
    );
    if let Some(c) = cookie.take() {
        release(&rt, &conn, c);
        let _ = events.unbounded_send(AwakeEvent::Held(false));
    }
    if let Err(e) = result {
        let _ = events.unbounded_send(AwakeEvent::Unavailable(e.to_string()));
    }
}

fn release(rt: &tokio::runtime::Handle, conn: &zbus::Connection, cookie: u32) {
    let _ = rt.block_on(conn.call_method(
        Some(SCREENSAVER),
        SCREENSAVER_PATH,
        Some(SCREENSAVER),
        "UnInhibit",
        &(cookie,),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keeps the screen on while a real window is focused and full screen:
    /// `AWAKE_APP_ID=… AWAKE_TITLE=… AWAKE_SECS=20 cargo test --release
    /// keep_a_real_window_awake -- --ignored --nocapture`. Prints each
    /// change; `dbus-monitor` shows the `Inhibit` and `UnInhibit` calls.
    #[test]
    #[ignore = "uses the running COSMIC session"]
    fn keep_a_real_window_awake() {
        let app_id = std::env::var("AWAKE_APP_ID").unwrap();
        let title = std::env::var("AWAKE_TITLE").unwrap();
        let secs: u64 = std::env::var("AWAKE_SECS").map_or(20, |s| s.parse().unwrap());
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let conn = rt.block_on(zbus::Connection::session()).unwrap();
        let bus = Some((rt.handle().clone(), conn));
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, mut rx) = mpsc::unbounded();
        let thread_alive = alive.clone();
        let watcher = std::thread::spawn(move || {
            run(&app_id, &[title], &thread_alive, &tx, bus);
        });
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(secs) {
            while let Ok(e) = rx.try_recv() {
                println!("{:>5.1}s: {e:?}", start.elapsed().as_secs_f32());
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        alive.store(false, Ordering::Relaxed);
        watcher.join().unwrap();
        while let Ok(e) = rx.try_recv() {
            println!("  end: {e:?}");
        }
    }
}
