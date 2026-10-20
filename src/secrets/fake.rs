// SPDX-License-Identifier: MIT

//! A fake Secret Service for tests, on a private `dbus-daemon`, so the
//! keyring code runs against the real D-Bus messages without touching the
//! desktop's keyring. It does what the specification's parts we use say:
//! `OpenSession` ("plain"), `SearchItems`, `Unlock` (with a prompt while
//! locked), `ReadAlias`, `CreateItem` (replacing an item with the same
//! attributes), an item's `Attributes`, `GetSecret` and `Delete`, and
//! prompts that send `Completed`.
//!
//! A test chooses how the keyring behaves: locked or not, and what the
//! user does with an unlock prompt (unlock, cancel, or never answer).

// The method signatures are the D-Bus interface's (zbus generates the
// dispatch from them): `self` and every argument stay, used or not.
#![allow(
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::used_underscore_binding
)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::{BUS_NAME, SERVICE_PATH, TEST_BUS};

const COLLECTION_PATH: &str = "/org/freedesktop/secrets/collection/login";
const SESSION_PATH: &str = "/org/freedesktop/secrets/session/1";

/// What the user does with an unlock prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Unlock,
    Cancel,
    /// The prompt stays open until the app gives up and dismisses it.
    Never,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub attributes: HashMap<String, String>,
    pub label: String,
    pub secret: Vec<u8>,
}

#[derive(Debug)]
struct State {
    items: BTreeMap<u32, Item>,
    next: u32,
    locked: bool,
    answer: Answer,
    /// No default keyring at all (`ReadAlias` answers "/").
    no_default: bool,
    prompts: u32,
    dismissed: u32,
}

/// The fake keyring, alive until dropped. While it lives, the keyring code
/// on this test thread talks to it instead of the desktop's.
pub struct FakeKeyring {
    state: Arc<Mutex<State>>,
    /// Kept alive with the keyring; ends the private bus when dropped.
    _bus: crate::test_util::PrivateBus,
    _runtime: tokio::runtime::Runtime,
    _connection: zbus::Connection,
}

impl FakeKeyring {
    /// An empty, unlocked keyring whose prompts are answered with `answer`.
    pub fn start(answer: Answer) -> Self {
        let bus = crate::test_util::PrivateBus::start();
        let state = Arc::new(Mutex::new(State {
            items: BTreeMap::new(),
            next: 1,
            locked: false,
            answer,
            no_default: false,
            prompts: 0,
            dismissed: 0,
        }));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        let connection = runtime.block_on(async {
            zbus::connection::Builder::address(bus.address())
                .unwrap()
                .name(BUS_NAME)
                .unwrap()
                .serve_at(SERVICE_PATH, Service(state.clone()))
                .unwrap()
                .serve_at(COLLECTION_PATH, Collection(state.clone()))
                .unwrap()
                .build()
                .await
                .expect("fake keyring on the private bus")
        });
        TEST_BUS.with(|b| *b.borrow_mut() = Some(bus.address().to_owned()));
        Self {
            state,
            _bus: bus,
            _runtime: runtime,
            _connection: connection,
        }
    }

    pub fn set_locked(&self, locked: bool) {
        self.state.lock().unwrap().locked = locked;
    }

    pub fn set_no_default(&self) {
        self.state.lock().unwrap().no_default = true;
    }

    pub fn locked(&self) -> bool {
        self.state.lock().unwrap().locked
    }

    /// Unlock prompts shown, and how many the app dismissed itself.
    pub fn prompts(&self) -> (u32, u32) {
        let s = self.state.lock().unwrap();
        (s.prompts, s.dismissed)
    }

    pub fn items(&self) -> Vec<Item> {
        self.state.lock().unwrap().items.values().cloned().collect()
    }
}

impl Drop for FakeKeyring {
    fn drop(&mut self) {
        TEST_BUS.with(|b| *b.borrow_mut() = None);
    }
}

fn item_path(n: u32) -> OwnedObjectPath {
    OwnedObjectPath::try_from(format!("{COLLECTION_PATH}/{n}")).expect("valid path")
}

fn root() -> OwnedObjectPath {
    OwnedObjectPath::try_from("/").expect("valid path")
}

struct Service(Arc<Mutex<State>>);

#[zbus::interface(name = "org.freedesktop.Secret.Service")]
impl Service {
    fn open_session(
        &self,
        algorithm: &str,
        _input: Value<'_>,
    ) -> zbus::fdo::Result<(OwnedValue, OwnedObjectPath)> {
        if algorithm != "plain" {
            return Err(zbus::fdo::Error::NotSupported(algorithm.into()));
        }
        Ok((
            OwnedValue::try_from(Value::from("")).expect("value"),
            OwnedObjectPath::try_from(SESSION_PATH).expect("valid path"),
        ))
    }

    fn search_items(
        &self,
        attributes: HashMap<String, String>,
    ) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
        let s = self.0.lock().unwrap();
        let found: Vec<OwnedObjectPath> = s
            .items
            .iter()
            .filter(|(_, item)| {
                attributes
                    .iter()
                    .all(|(k, v)| item.attributes.get(k) == Some(v))
            })
            .map(|(n, _)| item_path(*n))
            .collect();
        if s.locked {
            (Vec::new(), found)
        } else {
            (found, Vec::new())
        }
    }

    async fn unlock(
        &self,
        objects: Vec<OwnedObjectPath>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
        let n = {
            let mut s = self.0.lock().unwrap();
            if !s.locked {
                return (objects, root());
            }
            s.prompts += 1;
            s.prompts
        };
        let path = OwnedObjectPath::try_from(format!("/org/freedesktop/secrets/prompt/{n}"))
            .expect("valid path");
        server
            .at(
                path.clone(),
                Prompt {
                    state: self.0.clone(),
                    unlocks: objects,
                },
            )
            .await
            .expect("prompt object");
        (Vec::new(), path)
    }

    fn read_alias(&self, name: &str) -> OwnedObjectPath {
        let s = self.0.lock().unwrap();
        if name == "default" && !s.no_default {
            OwnedObjectPath::try_from(COLLECTION_PATH).expect("valid path")
        } else {
            root()
        }
    }
}

struct Collection(Arc<Mutex<State>>);

#[zbus::interface(name = "org.freedesktop.Secret.Collection")]
impl Collection {
    #[zbus(property)]
    fn locked(&self) -> bool {
        self.0.lock().unwrap().locked
    }

    async fn create_item(
        &self,
        properties: HashMap<String, OwnedValue>,
        secret: (OwnedObjectPath, Vec<u8>, Vec<u8>, String),
        replace: bool,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        let attributes: HashMap<String, String> = properties
            .get("org.freedesktop.Secret.Item.Attributes")
            .and_then(|v| HashMap::<String, String>::try_from(v.try_clone().ok()?).ok())
            .unwrap_or_default();
        let label: String = properties
            .get("org.freedesktop.Secret.Item.Label")
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
            .unwrap_or_default();
        let (n, new) = {
            let mut s = self.0.lock().unwrap();
            if s.locked {
                return Err(zbus::fdo::Error::AccessDenied(
                    "collection is locked".into(),
                ));
            }
            let same = s
                .items
                .iter()
                .find(|(_, i)| i.attributes == attributes)
                .map(|(n, _)| *n);
            let item = Item {
                attributes,
                label,
                secret: secret.2,
            };
            match same {
                Some(n) if replace => {
                    s.items.insert(n, item);
                    (n, false)
                }
                _ => {
                    let n = s.next;
                    s.next += 1;
                    s.items.insert(n, item);
                    (n, true)
                }
            }
        };
        if new {
            server
                .at(
                    item_path(n),
                    ItemObject {
                        state: self.0.clone(),
                        n,
                    },
                )
                .await
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        }
        Ok((item_path(n), root()))
    }
}

struct ItemObject {
    state: Arc<Mutex<State>>,
    n: u32,
}

#[zbus::interface(name = "org.freedesktop.Secret.Item")]
impl ItemObject {
    #[zbus(property)]
    fn attributes(&self) -> zbus::fdo::Result<HashMap<String, String>> {
        let s = self.state.lock().unwrap();
        s.items
            .get(&self.n)
            .map(|i| i.attributes.clone())
            .ok_or_else(|| zbus::fdo::Error::UnknownObject("deleted".into()))
    }

    fn get_secret(
        &self,
        session: OwnedObjectPath,
    ) -> zbus::fdo::Result<(OwnedObjectPath, Vec<u8>, Vec<u8>, String)> {
        let s = self.state.lock().unwrap();
        if s.locked {
            return Err(zbus::fdo::Error::AccessDenied("locked".into()));
        }
        let item = s
            .items
            .get(&self.n)
            .ok_or_else(|| zbus::fdo::Error::UnknownObject("deleted".into()))?;
        Ok((
            session,
            Vec::new(),
            item.secret.clone(),
            "text/plain".into(),
        ))
    }

    async fn delete(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        {
            let mut s = self.state.lock().unwrap();
            if s.locked {
                return Err(zbus::fdo::Error::AccessDenied("locked".into()));
            }
            s.items.remove(&self.n);
        }
        let _ = server.remove::<ItemObject, _>(item_path(self.n)).await;
        Ok(root())
    }
}

struct Prompt {
    state: Arc<Mutex<State>>,
    unlocks: Vec<OwnedObjectPath>,
}

#[zbus::interface(name = "org.freedesktop.Secret.Prompt")]
impl Prompt {
    async fn prompt(&self, _window_id: &str, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        let answer = self.state.lock().unwrap().answer;
        match answer {
            Answer::Unlock => {
                self.state.lock().unwrap().locked = false;
                let paths: Vec<ObjectPath<'_>> = self.unlocks.iter().map(|p| p.as_ref()).collect();
                let _ = Self::completed(&emitter, false, Value::from(paths)).await;
            }
            Answer::Cancel => {
                let _ = Self::completed(&emitter, true, Value::from("")).await;
            }
            Answer::Never => {}
        }
    }

    async fn dismiss(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.state.lock().unwrap().dismissed += 1;
        let _ = Self::completed(&emitter, true, Value::from("")).await;
    }

    #[zbus(signal)]
    async fn completed(
        emitter: &SignalEmitter<'_>,
        dismissed: bool,
        result: Value<'_>,
    ) -> zbus::Result<()>;
}
