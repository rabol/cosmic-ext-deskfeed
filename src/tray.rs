// SPDX-License-Identifier: MIT

//! The tray icon for background operation (spec §5, ADR 0017): a
//! `StatusNotifierItem` with a menu, served by `ksni` (ADR 0003).
//!
//! COSMIC's status area (cosmic-applets `cosmic-applet-status-area`) calls
//! `Activate` on a click and shows the menu otherwise. Before each click,
//! on the icon or on a menu line, it sends an activation token
//! (`ProvideXdgActivationToken`) and waits for the answer; ksni hands that
//! token to the click (`ksni::current_xdg_activation_token`), so the
//! window may take focus. COSMIC draws labels, separators, disabled lines,
//! check marks and one level of submenus. It shows no tooltip or title,
//! so the menu is where sessions show (a count, check marks).
//!
//! The app builds the menu ([`TrayItem`]); this module serves it.
//!
//! The item exists only while a `StatusNotifierWatcher` answers; otherwise
//! the app is told the tray is unavailable, so it never runs without a way
//! back (spec §5).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cosmic::iced::futures::{SinkExt, StreamExt, channel::mpsc};
use ksni::TrayMethods as _;
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};

/// One menu line, built by the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayItem {
    /// A line that does something.
    Action {
        label: String,
        action: TrayAction,
        /// Shown with a check mark (a connected desktop).
        checked: bool,
        enabled: bool,
    },
    /// A line that only informs (disabled).
    Note(String),
    Separator,
    /// One level of submenu (an account's desktops).
    Submenu {
        label: String,
        items: Vec<TrayItem>,
    },
}

/// What a menu line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Quit,
    /// Bring this desktop (resource key) to the front.
    ShowDesktop(String),
    /// Connect this desktop (resource key).
    Connect(String),
}

/// What the tray reports to the app.
#[derive(Debug, Clone)]
pub enum TrayEvent {
    /// The item is registered; the handle updates its menu.
    Ready(Handle),
    /// No tray to show an item in (or it went away).
    Unavailable(String),
    /// Open or focus the window, with COSMIC's activation token if any.
    Show(Option<String>),
    /// Bring this desktop (resource key) to the front.
    ShowDesktop(String),
    /// Connect this desktop (resource key).
    Connect(String),
    Quit,
}

/// Updates the tray's menu from the app.
#[derive(Clone)]
pub struct Handle(Arc<ksni::Handle<Tray>>);

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tray::Handle")
    }
}

impl Handle {
    /// Replaces the menu. ksni sends the panel only what changed. Desktop
    /// titles stay on this computer's session bus, like the window titles.
    pub async fn set_menu(&self, items: Vec<TrayItem>) {
        self.0
            .update(|tray| {
                if tray.items != items {
                    tray.items = items;
                }
            })
            .await;
    }
}

/// The labels the tray shows before the app sends its menu.
pub struct Labels {
    pub title: String,
    pub show: String,
    pub quit: String,
}

/// The item ksni serves.
struct Tray {
    title: String,
    items: Vec<TrayItem>,
    events: mpsc::UnboundedSender<TrayEvent>,
    /// Set once ksni has started, for `Ready` when the panel's tray returns.
    handle: Arc<OnceLock<Handle>>,
    /// Counts the panel's tray going and coming, so a grace timer knows
    /// whether the absence it started for is still the current one.
    changes: Arc<AtomicU64>,
    grace: Duration,
}

impl Tray {
    fn send(&self, event: TrayEvent) {
        let _ = self.events.unbounded_send(event);
    }

    fn act(&self, action: &TrayAction) {
        self.send(match action {
            TrayAction::Show => TrayEvent::Show(ksni::current_xdg_activation_token()),
            TrayAction::Quit => TrayEvent::Quit,
            TrayAction::ShowDesktop(key) => TrayEvent::ShowDesktop(key.clone()),
            TrayAction::Connect(key) => TrayEvent::Connect(key.clone()),
        });
    }
}

/// The app's menu lines as ksni menu items.
fn menu_items(items: &[TrayItem]) -> Vec<ksni::MenuItem<Tray>> {
    items
        .iter()
        .map(|item| match item {
            TrayItem::Action {
                label,
                action,
                checked,
                enabled,
            } => {
                // ksni runs the action even if a click arrives for a
                // disabled line; such a line does nothing.
                let action = enabled.then(|| action.clone());
                let activate = Box::new(move |tray: &mut Tray| {
                    if let Some(action) = &action {
                        tray.act(action);
                    }
                });
                if *checked {
                    CheckmarkItem {
                        label: label.clone(),
                        enabled: *enabled,
                        checked: true,
                        activate,
                        ..Default::default()
                    }
                    .into()
                } else {
                    StandardItem {
                        label: label.clone(),
                        enabled: *enabled,
                        activate,
                        ..Default::default()
                    }
                    .into()
                }
            }
            TrayItem::Note(label) => StandardItem {
                label: label.clone(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            TrayItem::Separator => ksni::MenuItem::Separator,
            TrayItem::Submenu { label, items } => SubMenu {
                label: label.clone(),
                submenu: menu_items(items),
                ..Default::default()
            }
            .into(),
        })
        .collect()
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        crate::app::APP_ID.into()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    /// The app's installed icon (`resources/icons`, installed by `just`).
    fn icon_name(&self) -> String {
        crate::app::APP_ID.into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.title.clone(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.act(&TrayAction::Show);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.act(&TrayAction::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        menu_items(&self.items)
    }

    fn watcher_online(&self) {
        self.changes.fetch_add(1, Ordering::SeqCst);
        crate::app_log!("tray: the panel's tray came back");
        if let Some(handle) = self.handle.get() {
            self.send(TrayEvent::Ready(handle.clone()));
        }
    }

    /// The panel's tray went away. Gone for `grace` (a panel restart takes
    /// a moment), the app hears `Unavailable` and shows its window, so it
    /// is never hidden without a way back (spec §5). Refusing to register
    /// us again is told at once. ksni keeps watching, and registers again
    /// when the tray returns.
    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        let change = self.changes.fetch_add(1, Ordering::SeqCst) + 1;
        if let ksni::OfflineReason::Error(e) = reason {
            self.send(TrayEvent::Unavailable(format!(
                "the panel's tray came back but refused ({e})"
            )));
            return true;
        }
        crate::app_log!("tray: the panel's tray went away");
        let (changes, events, grace) = (self.changes.clone(), self.events.clone(), self.grace);
        tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            if changes.load(Ordering::SeqCst) == change {
                let _ = events
                    .unbounded_send(TrayEvent::Unavailable("the panel's tray went away".into()));
            }
        });
        true
    }
}

/// The tray as a stream of events, for an iced subscription. Ends with
/// `Unavailable` when there is no tray.
pub fn run(labels: Labels) -> impl cosmic::iced::futures::Stream<Item = TrayEvent> {
    cosmic::iced::stream::channel(16, move |mut out: mpsc::Sender<TrayEvent>| async move {
        let (events, mut incoming) = mpsc::unbounded();
        match serve(labels, events, HOST_GRACE).await {
            Ok(handle) => {
                crate::app_log!("tray icon shown");
                let _ = out.send(TrayEvent::Ready(handle)).await;
                while let Some(e) = incoming.next().await {
                    if out.send(e).await.is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                crate::app_log!("tray unavailable: {e}");
                let _ = out.send(TrayEvent::Unavailable(e)).await;
            }
        }
        // Keep the subscription alive without spinning.
        std::future::pending::<()>().await;
    })
}

/// How long the panel's tray may be gone before the app is told (a panel
/// restart takes a moment).
const HOST_GRACE: Duration = Duration::from_secs(5);

/// Serves the item on the session bus and registers it with the panel's
/// tray, with "Show" and "Quit" until the app sends its menu.
async fn serve(
    labels: Labels,
    events: mpsc::UnboundedSender<TrayEvent>,
    grace: Duration,
) -> Result<Handle, String> {
    let handle = Arc::new(OnceLock::new());
    let tray = Tray {
        title: labels.title,
        items: vec![
            TrayItem::Action {
                label: labels.show,
                action: TrayAction::Show,
                checked: false,
                enabled: true,
            },
            TrayItem::Separator,
            TrayItem::Action {
                label: labels.quit,
                action: TrayAction::Quit,
                checked: false,
                enabled: true,
            },
        ],
        events,
        handle: handle.clone(),
        changes: Arc::default(),
        grace,
    };
    let served = Handle(Arc::new(tray.spawn().await.map_err(|e| e.to_string())?));
    let _ = handle.set(served.clone());
    Ok(served)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const WATCHER: &str = "org.kde.StatusNotifierWatcher";
    const WATCHER_PATH: &str = "/StatusNotifierWatcher";
    /// Set for the child process that runs a test on a private bus.
    const CHILD: &str = "DESKFEED_TRAY_TEST_CHILD";

    /// Runs the ignored test `name` in a child process whose session bus
    /// is a private `dbus-daemon`: ksni only uses the session bus, and the
    /// real panel must not be touched.
    fn on_private_bus(name: &str) {
        let bus = crate::test_util::PrivateBus::start();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                name,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
            .env(CHILD, "1")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{text}");
        assert!(text.contains("1 passed"), "{text}");
    }

    fn in_child() -> bool {
        std::env::var_os(CHILD).is_some()
    }

    fn action(label: &str, action: TrayAction) -> TrayItem {
        TrayItem::Action {
            label: label.into(),
            action,
            checked: false,
            enabled: true,
        }
    }

    fn sample() -> Vec<TrayItem> {
        vec![
            TrayItem::Note("1 desktop connected".into()),
            action("Show Deskfeed", TrayAction::Show),
            TrayItem::Separator,
            TrayItem::Submenu {
                label: "a@example.com".into(),
                items: vec![
                    TrayItem::Action {
                        label: "Win 11".into(),
                        action: TrayAction::ShowDesktop("k1".into()),
                        checked: true,
                        enabled: true,
                    },
                    TrayItem::Action {
                        label: "Dev · Connecting".into(),
                        action: TrayAction::Connect("k2".into()),
                        checked: false,
                        enabled: false,
                    },
                ],
            },
            action("Lab", TrayAction::Connect("k3".into())),
            TrayItem::Separator,
            action("Quit", TrayAction::Quit),
        ]
    }

    fn labels() -> Labels {
        Labels {
            title: "Deskfeed".into(),
            show: "Show".into(),
            quit: "Quit".into(),
        }
    }

    /// A stand-in for the panel's tray (the `StatusNotifierWatcher`).
    struct FakeWatcher {
        registered: Arc<Mutex<Vec<String>>>,
    }

    #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
    impl FakeWatcher {
        #[zbus(property)]
        #[allow(clippy::unused_self)]
        fn is_status_notifier_host_registered(&self) -> bool {
            true
        }
        fn register_status_notifier_item(&self, service: String) {
            self.registered.lock().unwrap().push(service);
        }
    }

    async fn watcher(registered: &Arc<Mutex<Vec<String>>>) -> zbus::Connection {
        zbus::connection::Builder::session()
            .unwrap()
            .name(WATCHER)
            .unwrap()
            .serve_at(
                WATCHER_PATH,
                FakeWatcher {
                    registered: registered.clone(),
                },
            )
            .unwrap()
            .build()
            .await
            .unwrap()
    }

    /// Waits until `n` registrations have reached the watcher: ksni tells
    /// the app the tray is back just before it registers again.
    async fn registered_soon(registered: &Arc<Mutex<Vec<String>>>, n: usize) {
        for _ in 0..300 {
            if registered.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(registered.lock().unwrap().len(), n);
    }

    async fn next(
        rx: &mut mpsc::UnboundedReceiver<TrayEvent>,
        within: Duration,
    ) -> Option<TrayEvent> {
        tokio::time::timeout(within, rx.next()).await.ok().flatten()
    }

    #[test]
    fn follows_the_panels_tray() {
        on_private_bus("tray::tests::follows_the_panels_tray_child");
    }

    /// The panel's tray restarting, going away and coming back.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "run by follows_the_panels_tray"]
    async fn follows_the_panels_tray_child() {
        if !in_child() {
            return;
        }
        let registered = Arc::new(Mutex::new(Vec::new()));
        let first = watcher(&registered).await;
        let (tx, mut rx) = mpsc::unbounded();
        let grace = Duration::from_millis(400);
        let _handle = serve(labels(), tx, grace).await.unwrap();
        assert_eq!(registered.lock().unwrap().len(), 1);

        // A quick restart: registered again, and no "unavailable".
        first.close().await.unwrap();
        let second = watcher(&registered).await;
        let event = next(&mut rx, Duration::from_secs(3)).await;
        assert!(matches!(event, Some(TrayEvent::Ready(_))), "{event:?}");
        assert!(next(&mut rx, grace * 2).await.is_none());
        registered_soon(&registered, 2).await;

        // Gone for good: the app hears it after the grace time.
        second.close().await.unwrap();
        let event = next(&mut rx, grace + Duration::from_secs(3)).await;
        assert!(
            matches!(event, Some(TrayEvent::Unavailable(_))),
            "{event:?}"
        );

        // Back later: the icon returns.
        let _third = watcher(&registered).await;
        let event = next(&mut rx, Duration::from_secs(3)).await;
        assert!(matches!(event, Some(TrayEvent::Ready(_))), "{event:?}");
        registered_soon(&registered, 3).await;
    }

    #[test]
    fn the_panel_sees_the_menu_and_clicks_reach_the_app() {
        on_private_bus("tray::tests::the_panel_sees_the_menu_and_clicks_reach_the_app_child");
    }

    type Props = HashMap<String, OwnedValue>;

    /// Every line of a dbusmenu layout, depth first: (id, properties).
    fn lines(children: Vec<OwnedValue>, out: &mut Vec<(i32, Props)>) {
        for child in children {
            let (id, props, children): (i32, Props, Vec<OwnedValue>) = child.try_into().unwrap();
            out.push((id, props));
            lines(children, out);
        }
    }

    fn label(props: &Props) -> String {
        props
            .get("label")
            .and_then(|v| String::try_from(v.clone()).ok())
            .unwrap_or_default()
    }

    /// The line labelled `text`.
    fn line(all: &[(i32, Props)], text: &str) -> (i32, Props) {
        all.iter()
            .find(|(_, p)| label(p) == text)
            .unwrap_or_else(|| panic!("no line {text:?} in {all:?}"))
            .clone()
    }

    /// The sample menu, as the panel reads it: notes and busy desktops
    /// disabled, a check mark on the connected one, the submenu, the
    /// separators.
    fn check_layout(all: &[(i32, Props)]) {
        let find = |text: &str| line(all, text);
        let enabled = |p: &Props| {
            p.get("enabled")
                .is_none_or(|v| bool::try_from(v.clone()).unwrap())
        };
        assert!(
            !enabled(&find("1 desktop connected").1),
            "a note is disabled"
        );
        assert!(!enabled(&find("Dev · Connecting").1));
        let checked = |p: &Props| {
            p.get("toggle-state")
                .map(|v| i32::try_from(v.clone()).unwrap())
        };
        assert_eq!(checked(&find("Win 11").1), Some(1));
        assert_eq!(checked(&find("Lab").1), None);
        let submenu = find("a@example.com").1;
        assert_eq!(
            submenu
                .get("children-display")
                .map(|v| String::try_from(v.clone()).unwrap()),
            Some("submenu".into())
        );
        let separators = all
            .iter()
            .filter(|(_, p)| {
                p.get("type")
                    .is_some_and(|v| String::try_from(v.clone()).unwrap() == "separator")
            })
            .count();
        assert_eq!(separators, 2);
    }

    /// The panel's side, done as COSMIC's status area does it: the menu
    /// read with `GetLayout`, and before each click (icon or line) an
    /// activation token, waited for, then the click.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "run by the_panel_sees_the_menu_and_clicks_reach_the_app"]
    async fn the_panel_sees_the_menu_and_clicks_reach_the_app_child() {
        if !in_child() {
            return;
        }
        let registered = Arc::new(Mutex::new(Vec::new()));
        let panel = watcher(&registered).await;
        let (tx, mut rx) = mpsc::unbounded();
        let handle = serve(labels(), tx, HOST_GRACE).await.unwrap();
        handle.set_menu(sample()).await;
        let name = registered.lock().unwrap()[0].clone();
        let item = zbus::Proxy::new(
            &panel,
            name.clone(),
            "/StatusNotifierItem",
            WATCHER.replace("Watcher", "Item"),
        )
        .await
        .unwrap();
        let soon = Duration::from_secs(3);

        // The icon: with a token, and the next click without one.
        item.call::<_, _, ()>("ProvideXdgActivationToken", &("icon-token",))
            .await
            .unwrap();
        item.call::<_, _, ()>("Activate", &(0i32, 0i32))
            .await
            .unwrap();
        let event = next(&mut rx, soon).await;
        assert!(
            matches!(&event, Some(TrayEvent::Show(Some(t))) if t == "icon-token"),
            "{event:?}"
        );
        item.call::<_, _, ()>("Activate", &(0i32, 0i32))
            .await
            .unwrap();
        let event = next(&mut rx, soon).await;
        assert!(matches!(event, Some(TrayEvent::Show(None))), "{event:?}");

        // The menu as the panel reads it.
        let path: OwnedObjectPath = item.get_property("Menu").await.unwrap();
        let menu = zbus::Proxy::new(&panel, name.clone(), path, "com.canonical.dbusmenu")
            .await
            .unwrap();
        let (_, (_, _, children)): (u32, (i32, Props, Vec<OwnedValue>)) = menu
            .call("GetLayout", &(0i32, -1i32, Vec::<String>::new()))
            .await
            .unwrap();
        let mut all = Vec::new();
        lines(children, &mut all);
        check_layout(&all);
        let find = |text: &str| line(&all, text);
        // Clicks on lines, each after a token as COSMIC sends them.
        let click = |text: &str, token: &str| {
            let id = find(text).0;
            let (item, menu, token) = (item.clone(), menu.clone(), token.to_owned());
            async move {
                item.call::<_, _, ()>("ProvideXdgActivationToken", &(token,))
                    .await
                    .unwrap();
                menu.call::<_, _, ()>("Event", &(id, "clicked", Value::from(0i32), 0u32))
                    .await
                    .unwrap();
            }
        };
        click("Show Deskfeed", "menu-token").await;
        let event = next(&mut rx, soon).await;
        assert!(
            matches!(&event, Some(TrayEvent::Show(Some(t))) if t == "menu-token"),
            "{event:?}"
        );
        click("Win 11", "t").await;
        let event = next(&mut rx, soon).await;
        assert!(
            matches!(&event, Some(TrayEvent::ShowDesktop(k)) if k == "k1"),
            "{event:?}"
        );
        click("Dev · Connecting", "t").await; // disabled: nothing
        click("Lab", "t").await;
        let event = next(&mut rx, soon).await;
        assert!(
            matches!(&event, Some(TrayEvent::Connect(k)) if k == "k3"),
            "{event:?}"
        );
        click("Quit", "t").await;
        let event = next(&mut rx, soon).await;
        assert!(matches!(event, Some(TrayEvent::Quit)), "{event:?}");
        assert!(next(&mut rx, Duration::from_millis(300)).await.is_none());
    }
}
