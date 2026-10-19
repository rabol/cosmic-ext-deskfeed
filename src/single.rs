// SPDX-License-Identifier: MIT

//! One Deskfeed per user (ADR 0017). A second start asks the running one
//! to show its window, then ends.
//!
//! libcosmic's `single-instance` can't be used: it serves at the app ID
//! with only `.` replaced (`/io/github/rabol/cosmic-ext-deskfeed`), and `-`
//! isn't allowed in a D-Bus object path, so it exits at start. This is
//! the standard `org.freedesktop.Application` interface instead, at the
//! path its specification gives (`-` becomes `_`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cosmic::iced::futures::channel::mpsc;
use zbus::zvariant::{OwnedValue, Value};

const PATH: &str = "/io/github/rabol/cosmic_ext_deskfeed";
const INTERFACE: &str = "org.freedesktop.Application";

/// Activations for the app: the launcher's activation token, if any.
static ACTIVATIONS: OnceLock<Mutex<Option<mpsc::UnboundedReceiver<Option<String>>>>> =
    OnceLock::new();

/// Kept for the app's lifetime: the connection holds the name, and runs
/// on its own small runtime (zbus is built for tokio here, and the app's
/// runtime doesn't exist yet when `main` starts).
static CONNECTION: OnceLock<(tokio::runtime::Runtime, zbus::Connection)> = OnceLock::new();

struct Application {
    tx: mpsc::UnboundedSender<Option<String>>,
}

fn token(platform_data: &HashMap<String, OwnedValue>) -> Option<String> {
    let value = platform_data
        .get("activation-token")
        .or_else(|| platform_data.get("desktop-startup-id"))?;
    let token = String::try_from(value.try_clone().ok()?).ok()?;
    (!token.is_empty() && token.len() <= 1024).then_some(token)
}

#[zbus::interface(name = "org.freedesktop.Application")]
impl Application {
    fn activate(&self, platform_data: HashMap<String, OwnedValue>) {
        let _ = self.tx.unbounded_send(token(&platform_data));
    }
    /// Deskfeed opens no files or URLs: just show the window.
    fn open(&self, _uris: Vec<String>, platform_data: HashMap<String, OwnedValue>) {
        let _ = self.tx.unbounded_send(token(&platform_data));
    }
    fn activate_action(
        &self,
        _action: String,
        _parameter: Vec<OwnedValue>,
        platform_data: HashMap<String, OwnedValue>,
    ) {
        let _ = self.tx.unbounded_send(token(&platform_data));
    }
}

/// What `main` does next.
pub enum Start {
    /// This is the only Deskfeed: run the app.
    Run,
    /// Another Deskfeed was asked to show its window: end.
    HandedOver,
}

/// Claims the app's name, or hands over to the Deskfeed that has it.
/// Without a session bus, Deskfeed just runs.
pub fn start(app_id: &str) -> Start {
    let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("deskfeed-single")
        .enable_all()
        .build()
    else {
        return Start::Run;
    };
    let (tx, rx) = mpsc::unbounded();
    let claimed = rt.block_on(async {
        let conn = zbus::Connection::session().await.ok()?;
        conn.object_server()
            .at(PATH, Application { tx })
            .await
            .ok()?;
        let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
        let owner = matches!(
            conn.request_name_with_flags(app_id, flags).await,
            Ok(zbus::fdo::RequestNameReply::PrimaryOwner)
        );
        Some((conn, owner))
    });
    match claimed {
        Some((conn, true)) => {
            let _ = ACTIVATIONS.set(Mutex::new(Some(rx)));
            let _ = CONNECTION.set((rt, conn));
            Start::Run
        }
        Some((conn, false)) => {
            if rt.block_on(hand_over(&conn, app_id)) {
                Start::HandedOver
            } else {
                Start::Run
            }
        }
        None => Start::Run,
    }
}

/// Asks the running Deskfeed to show its window, passing on this start's
/// activation token so the window may take focus.
async fn hand_over(conn: &zbus::Connection, app_id: &str) -> bool {
    let mut platform_data: HashMap<&str, Value<'_>> = HashMap::new();
    for (env, key) in [
        ("XDG_ACTIVATION_TOKEN", "activation-token"),
        ("DESKTOP_STARTUP_ID", "desktop-startup-id"),
    ] {
        if let Ok(v) = std::env::var(env) {
            platform_data.insert(key, Value::from(v));
        }
    }
    let ok = conn
        .call_method(
            Some(app_id),
            PATH,
            Some(INTERFACE),
            "Activate",
            &(platform_data,),
        )
        .await
        .is_ok();
    crate::app_log!(
        "already running: {}",
        if ok {
            "asked it to show its window"
        } else {
            "it didn't answer; starting anyway"
        }
    );
    ok
}

/// Activations from later starts, for a subscription (once).
pub fn activations() -> impl cosmic::iced::futures::Stream<Item = Option<String>> {
    use cosmic::iced::futures::StreamExt;
    let rx = ACTIVATIONS.get().and_then(|m| m.lock().ok()?.take());
    match rx {
        Some(rx) => rx.chain(cosmic::iced::futures::stream::pending()).boxed(),
        None => cosmic::iced::futures::stream::pending().boxed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_is_a_valid_object_path() {
        assert!(zbus::zvariant::ObjectPath::try_from(PATH).is_ok());
        // libcosmic's own path for this app ID isn't.
        assert!(
            zbus::zvariant::ObjectPath::try_from("/io/github/rabol/cosmic-ext-deskfeed").is_err()
        );
    }

    #[test]
    fn token_is_read_and_bounded() {
        let mut data = HashMap::new();
        data.insert(
            "activation-token".to_owned(),
            OwnedValue::try_from(Value::from("abc")).unwrap(),
        );
        assert_eq!(token(&data).as_deref(), Some("abc"));
        data.insert(
            "activation-token".to_owned(),
            OwnedValue::try_from(Value::from("x".repeat(2000))).unwrap(),
        );
        assert_eq!(token(&data), None);
    }
}
