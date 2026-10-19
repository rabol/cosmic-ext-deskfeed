// SPDX-License-Identifier: MIT

//! The tray icon for background operation (spec §5, ADR 0017): a
//! StatusNotifierItem with a small `com.canonical.dbusmenu` menu, served on
//! the session bus with zbus (already in the tree, ADR 0009).
//!
//! COSMIC's status area (cosmic-applets `cosmic-applet-status-area`) calls
//! `Activate` on a click, after `ProvideXdgActivationToken`, and shows the
//! menu otherwise. Clicking opens Deskfeed; the menu has "Show Deskfeed",
//! one "Show" entry per connected desktop, and "Quit".
//!
//! The item exists only while a StatusNotifierWatcher with a host answers;
//! otherwise the app is told the tray is unavailable, so it never runs
//! without a way back (spec §5).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cosmic::iced::futures::{SinkExt, StreamExt, channel::mpsc};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Structure, StructureBuilder, Value};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";

/// Menu item IDs; desktops follow from `FIRST_DESKTOP`.
const SHOW: i32 = 1;
const QUIT: i32 = 2;
const SEPARATOR_1: i32 = 3;
const SEPARATOR_2: i32 = 4;
const FIRST_DESKTOP: i32 = 100;
/// More aren't listed (each is a menu line).
const MAX_DESKTOPS: usize = 20;

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
    /// The connected desktops: (resource key, title). Titles stay on this
    /// computer's session bus, like the window titles.
    pub async fn set_desktops(&self, desktops: Vec<(String, String)>) {
        let revision = {
            let Ok(mut m) = self.menu.lock() else { return };
            let desktops: Vec<_> = desktops.into_iter().take(MAX_DESKTOPS).collect();
            if m.desktops == desktops {
                return;
            }
            m.desktops = desktops;
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
    desktops: Vec<(String, String)>,
    /// Labels in the user's language.
    show_label: String,
    quit_label: String,
    show_desktop_label: String,
}

struct Item {
    events: mpsc::UnboundedSender<TrayEvent>,
    token: Arc<Mutex<Option<String>>>,
    title: String,
}

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

    fn activate(&self, _x: i32, _y: i32) {
        let token = self.token.lock().ok().and_then(|mut t| t.take());
        let _ = self.events.unbounded_send(TrayEvent::Show(token));
    }
    fn secondary_activate(&self, x: i32, y: i32) {
        self.activate(x, y);
    }
    fn context_menu(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}
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
}

type Props = HashMap<String, OwnedValue>;
/// `(width, height, ARGB32 data)` (StatusNotifierItem icon pixmaps).
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

fn props(pairs: &[(&str, Value<'_>)]) -> Props {
    pairs
        .iter()
        .filter_map(|(k, v)| Some(((*k).to_owned(), v.try_to_owned().ok()?)))
        .collect()
}

fn entry(id: i32, props: Props) -> Structure<'static> {
    StructureBuilder::new()
        .add_field(id)
        .add_field(props)
        .add_field(Vec::<Value<'static>>::new())
        .build()
        .expect("non-empty structure")
}

impl Menu {
    /// `(id, properties, children)` for the whole menu.
    fn layout(&self) -> (i32, Props, Vec<OwnedValue>) {
        let Ok(m) = self.state.lock() else {
            return (0, Props::new(), Vec::new());
        };
        let label = |s: &str| Value::from(s.to_owned());
        let separator = |id| entry(id, props(&[("type", Value::from("separator"))]));
        let mut children = vec![entry(SHOW, props(&[("label", label(&m.show_label))]))];
        if !m.desktops.is_empty() {
            children.push(separator(SEPARATOR_1));
            for (i, (_, title)) in m.desktops.iter().enumerate() {
                let text = m.show_desktop_label.replace("{title}", title);
                let id = FIRST_DESKTOP + i32::try_from(i).unwrap_or(0);
                children.push(entry(id, props(&[("label", label(&text))])));
            }
        }
        children.push(separator(SEPARATOR_2));
        children.push(entry(QUIT, props(&[("label", label(&m.quit_label))])));
        let children = children
            .into_iter()
            .filter_map(|c| OwnedValue::try_from(Value::from(c)).ok())
            .collect();
        (
            0,
            props(&[("children-display", Value::from("submenu"))]),
            children,
        )
    }

    fn clicked(&self, id: i32) {
        let event = match id {
            SHOW => Some(TrayEvent::Show(None)),
            QUIT => Some(TrayEvent::Quit),
            id if id >= FIRST_DESKTOP => self.state.lock().ok().and_then(|m| {
                let i = usize::try_from(id - FIRST_DESKTOP).ok()?;
                m.desktops
                    .get(i)
                    .map(|(key, _)| TrayEvent::ShowDesktop(key.clone()))
            }),
            _ => None,
        };
        if let Some(e) = event {
            let _ = self.events.unbounded_send(e);
        }
    }
}

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
        _parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> (u32, (i32, Props, Vec<OwnedValue>)) {
        let revision = self.state.lock().map_or(0, |m| m.revision);
        (revision, self.layout())
    }

    fn get_group_properties(
        &self,
        _ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, Props)> {
        Vec::new()
    }

    fn get_property(&self, _id: i32, _name: String) -> OwnedValue {
        OwnedValue::from(0u32)
    }

    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
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

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

/// The labels the tray shows, in the user's language.
pub struct Labels {
    pub title: String,
    pub show: String,
    pub quit: String,
    /// With `{title}` for the desktop's title.
    pub show_desktop: String,
}

/// The tray as a stream of events, for an iced subscription. Ends with
/// `Unavailable` when there is no tray.
pub fn run(labels: Labels) -> impl cosmic::iced::futures::Stream<Item = TrayEvent> {
    cosmic::iced::stream::channel(16, move |mut out: mpsc::Sender<TrayEvent>| async move {
        let (events, mut incoming) = mpsc::unbounded();
        match serve(labels, events).await {
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

async fn serve(labels: Labels, events: mpsc::UnboundedSender<TrayEvent>) -> Result<Handle, String> {
    let err = |e: zbus::Error| e.to_string();
    let menu = Arc::new(Mutex::new(MenuState {
        show_label: labels.show,
        quit_label: labels.quit,
        show_desktop_label: labels.show_desktop,
        ..MenuState::default()
    }));
    let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    let conn = zbus::connection::Builder::session()
        .map_err(err)?
        .name(name.as_str())
        .map_err(err)?
        .serve_at(
            ITEM_PATH,
            Item {
                events: events.clone(),
                token: Arc::default(),
                title: labels.title,
            },
        )
        .map_err(err)?
        .serve_at(
            MENU_PATH,
            Menu {
                state: menu.clone(),
                events,
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
    // Register again whenever the panel (the watcher) restarts.
    {
        let conn = conn.clone();
        let name = name.clone();
        tokio::spawn(async move {
            let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else {
                return;
            };
            let Ok(mut changes) = dbus.receive_name_owner_changed().await else {
                return;
            };
            while let Some(change) = changes.next().await {
                let Ok(args) = change.args() else { continue };
                if args.name() == WATCHER
                    && args.new_owner().is_some()
                    && let Ok(w) = zbus::Proxy::new(&conn, WATCHER, WATCHER_PATH, WATCHER).await
                {
                    let _ = w
                        .call::<_, _, ()>("RegisterStatusNotifierItem", &(name.as_str(),))
                        .await;
                }
            }
        });
    }
    Ok(Handle { menu, conn })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(desktops: &[(&str, &str)]) -> Menu {
        let (events, _rx) = mpsc::unbounded();
        Menu {
            state: Arc::new(Mutex::new(MenuState {
                desktops: desktops
                    .iter()
                    .map(|(k, t)| ((*k).to_owned(), (*t).to_owned()))
                    .collect(),
                show_label: "Show Deskfeed".into(),
                quit_label: "Quit".into(),
                show_desktop_label: "Show {title}".into(),
                ..MenuState::default()
            })),
            events,
        }
    }

    #[test]
    fn layout_lists_show_desktops_and_quit() {
        let (_, _, children) = menu(&[]).layout();
        // Show, separator, Quit.
        assert_eq!(children.len(), 3);
        let (_, _, children) = menu(&[("k1", "Win 11"), ("k2", "Dev")]).layout();
        // Show, separator, two desktops, separator, Quit.
        assert_eq!(children.len(), 6);
    }

    #[test]
    fn clicks_map_to_events() {
        let (events, mut rx) = mpsc::unbounded();
        let mut m = menu(&[("k1", "Win 11")]);
        m.events = events;
        m.clicked(FIRST_DESKTOP);
        m.clicked(QUIT);
        m.clicked(FIRST_DESKTOP + 5); // gone meanwhile: nothing
        assert!(matches!(rx.try_recv(), Ok(TrayEvent::ShowDesktop(k)) if k == "k1"));
        assert!(matches!(rx.try_recv(), Ok(TrayEvent::Quit)));
        assert!(rx.try_recv().is_err());
    }
}
