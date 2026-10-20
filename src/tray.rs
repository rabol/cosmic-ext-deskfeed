// SPDX-License-Identifier: MIT

//! The tray icon for background operation (spec §5, ADR 0017): a
//! `StatusNotifierItem` with a small `com.canonical.dbusmenu` menu, served on
//! the session bus with zbus (already in the tree, ADR 0009).
//!
//! COSMIC's status area (cosmic-applets `cosmic-applet-status-area`) calls
//! `Activate` on a click, after `ProvideXdgActivationToken`, and shows the
//! menu otherwise. It draws labels, separators, disabled items, a check
//! mark (`toggle-state` 1) and one level of submenus, and sends a fresh
//! activation token before each menu click. It shows no tooltip or title,
//! so the menu is where sessions show (a count, check marks).
//!
//! The app builds the menu ([`TrayItem`]); this module serves it.
//!
//! The item exists only while a `StatusNotifierWatcher` with a host answers;
//! otherwise the app is told the tray is unavailable, so it never runs
//! without a way back (spec §5).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cosmic::iced::futures::{SinkExt, StreamExt, channel::mpsc};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Structure, StructureBuilder, Value};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";

/// Menu item IDs are the position in the flattened menu, from 1 (0 is
/// the root). More items than this aren't served.
const MAX_ITEMS: usize = 200;

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
pub struct Handle {
    menu: Arc<Mutex<MenuState>>,
    conn: zbus::Connection,
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tray::Handle")
    }
}

impl Handle {
    /// Replaces the menu (when it changed). Desktop titles stay on this
    /// computer's session bus, like the window titles.
    pub async fn set_menu(&self, items: Vec<TrayItem>) {
        let revision = {
            let Ok(mut m) = self.menu.lock() else { return };
            if m.items == items {
                return;
            }
            m.items = items;
            m.revision += 1;
            m.revision
        };
        if let Ok(emitter) = SignalEmitter::new(&self.conn, MENU_PATH) {
            let _ = Menu::layout_updated(&emitter, revision, 0).await;
        }
    }
}

#[derive(Default)]
struct MenuState {
    revision: u32,
    items: Vec<TrayItem>,
}

struct Item {
    events: mpsc::UnboundedSender<TrayEvent>,
    token: Arc<Mutex<Option<String>>>,
    title: String,
}

// The method signatures are the D-Bus interface's (zbus generates the
// dispatch from them): `self` and every argument stay, used or not.
#[allow(
    unused_variables,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::needless_lifetimes,
    clippy::unnecessary_literal_bound
)]
#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        crate::app::APP_ID
    }
    #[zbus(property)]
    fn title(&self) -> String {
        self.title.clone()
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }
    /// The app's installed icon (`resources/icons`, installed by `just`).
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        crate::app::APP_ID
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        (String::new(), Vec::new(), self.title.clone(), String::new())
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(MENU_PATH).expect("valid path")
    }

    fn activate(&self, x: i32, y: i32) {
        let token = self.token.lock().ok().and_then(|mut t| t.take());
        let _ = self.events.unbounded_send(TrayEvent::Show(token));
    }
    fn secondary_activate(&self, x: i32, y: i32) {
        self.activate(x, y);
    }
    fn context_menu(&self, x: i32, y: i32) {}
    fn scroll(&self, delta: i32, orientation: &str) {}
    /// COSMIC sends this before `Activate`, so the window may take focus.
    fn provide_xdg_activation_token(&self, token: String) {
        if token.len() <= 1024
            && let Ok(mut t) = self.token.lock()
        {
            *t = Some(token);
        }
    }
}

struct Menu {
    state: Arc<Mutex<MenuState>>,
    events: mpsc::UnboundedSender<TrayEvent>,
    /// COSMIC's activation token for the next click, shared with the item.
    token: Arc<Mutex<Option<String>>>,
}

type Props = HashMap<String, OwnedValue>;
/// `(width, height, ARGB32 data)` (`StatusNotifierItem` icon pixmaps).
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

fn props(pairs: &[(&str, Value<'_>)]) -> Props {
    pairs
        .iter()
        .filter_map(|(k, v)| Some(((*k).to_owned(), v.try_to_owned().ok()?)))
        .collect()
}

fn entry(id: i32, props: Props, children: Vec<Value<'static>>) -> Structure<'static> {
    StructureBuilder::new()
        .add_field(id)
        .add_field(props)
        .add_field(children)
        .build()
        .expect("non-empty structure")
}

/// The menu as dbusmenu entries, numbered in order from `next`.
fn entries(items: &[TrayItem], next: &mut i32) -> Vec<Value<'static>> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if usize::try_from(*next).unwrap_or(usize::MAX) > MAX_ITEMS {
            break;
        }
        let id = *next;
        *next += 1;
        let text = |s: &str| Value::from(s.to_owned());
        let structure = match item {
            TrayItem::Action {
                label,
                checked,
                enabled,
                ..
            } => {
                let mut p = vec![("label", text(label)), ("enabled", Value::from(*enabled))];
                if *checked {
                    p.push(("toggle-type", Value::from("checkmark")));
                    p.push(("toggle-state", Value::from(1i32)));
                }
                entry(id, props(&p), Vec::new())
            }
            TrayItem::Note(label) => entry(
                id,
                props(&[("label", text(label)), ("enabled", Value::from(false))]),
                Vec::new(),
            ),
            TrayItem::Separator => {
                entry(id, props(&[("type", Value::from("separator"))]), Vec::new())
            }
            TrayItem::Submenu { label, items } => {
                let children = entries(items, next);
                entry(
                    id,
                    props(&[
                        ("label", text(label)),
                        ("children-display", Value::from("submenu")),
                    ]),
                    children,
                )
            }
        };
        out.push(Value::from(structure));
    }
    out
}

/// The action of the line numbered `id`, counted as [`entries`] does.
fn action_of(items: &[TrayItem], id: i32) -> Option<TrayAction> {
    fn walk(items: &[TrayItem], id: i32, next: &mut i32) -> Option<TrayAction> {
        for item in items {
            let this = *next;
            *next += 1;
            match item {
                TrayItem::Action {
                    action, enabled, ..
                } if this == id => return enabled.then(|| action.clone()),
                TrayItem::Submenu { items, .. } => {
                    if this == id {
                        return None;
                    }
                    if let Some(a) = walk(items, id, next) {
                        return Some(a);
                    }
                }
                _ if this == id => return None,
                _ => {}
            }
        }
        None
    }
    walk(items, id, &mut 1)
}

impl Menu {
    /// `(id, properties, children)` for the whole menu.
    fn layout(&self) -> (i32, Props, Vec<OwnedValue>) {
        let Ok(m) = self.state.lock() else {
            return (0, Props::new(), Vec::new());
        };
        let children = entries(&m.items, &mut 1)
            .into_iter()
            .filter_map(|c| OwnedValue::try_from(c).ok())
            .collect();
        (
            0,
            props(&[("children-display", Value::from("submenu"))]),
            children,
        )
    }

    fn clicked(&self, id: i32) {
        let action = self.state.lock().ok().and_then(|m| action_of(&m.items, id));
        let event = match action {
            Some(TrayAction::Show) => {
                let token = self.token.lock().ok().and_then(|mut t| t.take());
                TrayEvent::Show(token)
            }
            Some(TrayAction::Quit) => TrayEvent::Quit,
            Some(TrayAction::ShowDesktop(key)) => TrayEvent::ShowDesktop(key),
            Some(TrayAction::Connect(key)) => TrayEvent::Connect(key),
            None => return,
        };
        let _ = self.events.unbounded_send(event);
    }
}

// As for `Item`: the signatures are the D-Bus interface's.
#[allow(
    unused_variables,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::needless_lifetimes,
    clippy::unnecessary_literal_bound
)]
#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    /// The whole menu, whatever part was asked for: it is small.
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> (u32, (i32, Props, Vec<OwnedValue>)) {
        let revision = self.state.lock().map_or(0, |m| m.revision);
        (revision, self.layout())
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<String>,
    ) -> Vec<(i32, Props)> {
        Vec::new()
    }

    fn get_property(&self, id: i32, name: String) -> OwnedValue {
        OwnedValue::from(0u32)
    }

    fn event(&self, id: i32, event_id: String, data: OwnedValue, timestamp: u32) {
        if event_id == "clicked" {
            self.clicked(id);
        }
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event_id, _, _) in events {
            if event_id == "clicked" {
                self.clicked(id);
            }
        }
        Vec::new()
    }

    fn about_to_show(&self, id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

/// The labels the tray shows before the app sends its menu.
pub struct Labels {
    pub title: String,
    pub show: String,
    pub quit: String,
}

/// The tray as a stream of events, for an iced subscription. Ends with
/// `Unavailable` when there is no tray.
pub fn run(labels: Labels) -> impl cosmic::iced::futures::Stream<Item = TrayEvent> {
    cosmic::iced::stream::channel(16, move |mut out: mpsc::Sender<TrayEvent>| async move {
        let (events, mut incoming) = mpsc::unbounded();
        let bus = match zbus::connection::Builder::session() {
            Ok(bus) => bus,
            Err(e) => {
                let _ = out.send(TrayEvent::Unavailable(e.to_string())).await;
                std::future::pending::<()>().await;
                unreachable!()
            }
        };
        match serve(bus, labels, events, HOST_GRACE).await {
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

/// Serves the item on `bus` and registers it with the panel's tray.
async fn serve(
    bus: zbus::connection::Builder<'static>,
    labels: Labels,
    events: mpsc::UnboundedSender<TrayEvent>,
    grace: Duration,
) -> Result<Handle, String> {
    let err = |e: zbus::Error| e.to_string();
    let menu = Arc::new(Mutex::new(MenuState {
        revision: 0,
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
    }));
    let token: Arc<Mutex<Option<String>>> = Arc::default();
    let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    let conn = bus
        .name(name.as_str())
        .map_err(err)?
        .serve_at(
            ITEM_PATH,
            Item {
                events: events.clone(),
                token: token.clone(),
                title: labels.title,
            },
        )
        .map_err(err)?
        .serve_at(
            MENU_PATH,
            Menu {
                state: menu.clone(),
                events: events.clone(),
                token,
            },
        )
        .map_err(err)?
        .build()
        .await
        .map_err(err)?;
    let watcher = zbus::Proxy::new(&conn, WATCHER, WATCHER_PATH, WATCHER)
        .await
        .map_err(err)?;
    let host: bool = watcher
        .get_property("IsStatusNotifierHostRegistered")
        .await
        .map_err(|e| format!("no StatusNotifierWatcher ({e})"))?;
    if !host {
        return Err("no tray host in the panel".into());
    }
    watcher
        .call::<_, _, ()>("RegisterStatusNotifierItem", &(name.as_str(),))
        .await
        .map_err(err)?;
    let handle = Handle {
        menu,
        conn: conn.clone(),
    };
    tokio::spawn(follow_host(conn, name, events, handle.clone(), grace));
    Ok(handle)
}

/// Follows the panel's tray (the watcher): when it goes away and stays away
/// for `grace`, the app hears `Unavailable` and shows its window, so it is
/// never hidden without a way back (spec §5). When it comes back,
/// the item registers again and the app hears `Ready`.
async fn follow_host(
    conn: zbus::Connection,
    name: String,
    events: mpsc::UnboundedSender<TrayEvent>,
    handle: Handle,
    grace: Duration,
) {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else {
        return;
    };
    let Ok(mut changes) = dbus.receive_name_owner_changed().await else {
        return;
    };
    while let Some(change) = changes.next().await {
        let Ok(args) = change.args() else { continue };
        if args.name() != WATCHER {
            continue;
        }
        if args.new_owner().is_none() {
            crate::app_log!("tray: the panel's tray went away");
            let (dbus, events) = (dbus.clone(), events.clone());
            tokio::spawn(async move {
                tokio::time::sleep(grace).await;
                let back = dbus
                    .name_has_owner(WATCHER.try_into().expect("valid name"))
                    .await
                    .unwrap_or(false);
                if !back {
                    let _ = events.unbounded_send(TrayEvent::Unavailable(
                        "the panel's tray went away".into(),
                    ));
                }
            });
            continue;
        }
        let registered = match zbus::Proxy::new(&conn, WATCHER, WATCHER_PATH, WATCHER).await {
            Ok(w) => w
                .call::<_, _, ()>("RegisterStatusNotifierItem", &(name.as_str(),))
                .await
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        let event = match registered {
            Ok(()) => {
                crate::app_log!("tray: registered again with the panel's tray");
                TrayEvent::Ready(handle.clone())
            }
            Err(e) => {
                TrayEvent::Unavailable(format!("the panel's tray came back but refused ({e})"))
            }
        };
        let _ = events.unbounded_send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    action("Win 11", TrayAction::ShowDesktop("k1".into())),
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

    #[test]
    fn ids_follow_the_flattened_order() {
        let items = sample();
        // 1 note, 2 show, 3 separator, 4 submenu, 5 Win 11, 6 Dev,
        // 7 Lab, 8 separator, 9 Quit.
        assert_eq!(action_of(&items, 2), Some(TrayAction::Show));
        assert_eq!(action_of(&items, 4), None);
        assert_eq!(
            action_of(&items, 5),
            Some(TrayAction::ShowDesktop("k1".into()))
        );
        assert_eq!(action_of(&items, 6), None, "disabled");
        assert_eq!(action_of(&items, 7), Some(TrayAction::Connect("k3".into())));
        assert_eq!(action_of(&items, 9), Some(TrayAction::Quit));
        assert_eq!(action_of(&items, 1), None, "a note");
        assert_eq!(action_of(&items, 42), None);
        // The served entries count the same way.
        let top = entries(&items, &mut 1);
        assert_eq!(top.len(), 7);
    }

    #[test]
    fn clicks_map_to_events() {
        let (events, mut rx) = mpsc::unbounded();
        let token: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(Some("tok".into())));
        let m = Menu {
            state: Arc::new(Mutex::new(MenuState {
                revision: 0,
                items: sample(),
            })),
            events,
            token,
        };
        m.clicked(2);
        m.clicked(5);
        m.clicked(7);
        m.clicked(6); // disabled: nothing
        m.clicked(9);
        assert!(matches!(rx.try_recv(), Ok(TrayEvent::Show(Some(t))) if t == "tok"));
        assert!(matches!(rx.try_recv(), Ok(TrayEvent::ShowDesktop(k)) if k == "k1"));
        assert!(matches!(rx.try_recv(), Ok(TrayEvent::Connect(k)) if k == "k3"));
        assert!(matches!(rx.try_recv(), Ok(TrayEvent::Quit)));
        assert!(rx.try_recv().is_err());
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

    async fn watcher(address: &str, registered: &Arc<Mutex<Vec<String>>>) -> zbus::Connection {
        zbus::connection::Builder::address(address)
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

    async fn next(
        rx: &mut mpsc::UnboundedReceiver<TrayEvent>,
        within: Duration,
    ) -> Option<TrayEvent> {
        tokio::time::timeout(within, rx.next()).await.ok().flatten()
    }

    /// The panel's tray restarting, going away and coming back, on a
    /// private bus (`dbus-daemon`), so the real panel isn't touched.
    #[tokio::test(flavor = "multi_thread")]
    async fn follows_the_panels_tray() {
        /// Ends the private bus however the test ends, so a failing test
        /// can't leave it running (and holding the test's output open).
        struct Bus(std::process::Child);
        impl Drop for Bus {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut bus = Bus(std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("dbus-daemon (package dbus)"));
        let mut address = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(bus.0.stdout.take().unwrap()),
            &mut address,
        )
        .unwrap();
        let address = address.trim().to_owned();
        let registered = Arc::new(Mutex::new(Vec::new()));
        let first = watcher(&address, &registered).await;
        let (tx, mut rx) = mpsc::unbounded();
        let labels = Labels {
            title: "Deskfeed".into(),
            show: "Show".into(),
            quit: "Quit".into(),
        };
        let grace = Duration::from_millis(400);
        let _handle = serve(
            zbus::connection::Builder::address(address.as_str()).unwrap(),
            labels,
            tx,
            grace,
        )
        .await
        .unwrap();
        assert_eq!(registered.lock().unwrap().len(), 1);

        // A quick restart: registered again, and no "unavailable".
        first.close().await.unwrap();
        let second = watcher(&address, &registered).await;
        let event = next(&mut rx, Duration::from_secs(3)).await;
        assert!(matches!(event, Some(TrayEvent::Ready(_))), "{event:?}");
        assert!(next(&mut rx, grace * 2).await.is_none());
        assert_eq!(registered.lock().unwrap().len(), 2);

        // Gone for good: the app hears it after the grace time.
        second.close().await.unwrap();
        let event = next(&mut rx, grace + Duration::from_secs(3)).await;
        assert!(
            matches!(event, Some(TrayEvent::Unavailable(_))),
            "{event:?}"
        );

        // Back later: the icon returns.
        let _third = watcher(&address, &registered).await;
        let event = next(&mut rx, Duration::from_secs(3)).await;
        assert!(matches!(event, Some(TrayEvent::Ready(_))), "{event:?}");
        assert_eq!(registered.lock().unwrap().len(), 3);
    }
}
