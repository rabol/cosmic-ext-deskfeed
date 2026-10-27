// SPDX-License-Identifier: MIT

//! Secret Service adapter (spec §6): keeps the refresh token in the
//! desktop keyring so the app stays signed in across restarts.
//!
//! - One item per account. Its attributes (searchable, not secret) are the
//!   app, client ID, authority, tenant and account object IDs, which keeps
//!   accounts, tenants and client identities apart. The secret holds the
//!   refresh token and the display names.
//! - Talks to `org.freedesktop.secrets` over D-Bus with zbus, which is
//!   already in the dependency tree (ADR 0009). Secrets travel with the
//!   specification's `plain` algorithm over the user's own session bus.
//! - Blocking: call from `spawn_blocking`, never on the UI thread.
//! - Never falls back to files (spec §6): errors go to the caller, which
//!   then runs a memory-only session.
//! - Windows passwords the user chose to remember (spec §8): one item per
//!   account and desktop, of their own type, so sign-in code never sees
//!   them; deleted with the account on sign-out.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use crate::auth::attempt::ClientConfig;
use crate::auth::id_token::Identity;
use crate::auth::token::Secret;

const BUS_NAME: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";
/// Identifies our items; the app ID.
const APPLICATION: &str = "dev.rabol.deskfeed";
const KIND: &str = "refresh-token";
const PASSWORD_KIND: &str = "windows-password";
/// A plain RDP connection's remembered password (ADR 0029).
const CONNECTION_PASSWORD_KIND: &str = "rdp-password";
/// Longer passwords aren't stored (Windows allows 256 characters).
const MAX_PASSWORD: usize = 1024;
const PAYLOAD_VERSION: u32 = 1;
const MAX_PAYLOAD: usize = 64 * 1024;

/// How long an unlock prompt may stay unanswered.
#[cfg(not(test))]
const PROMPT_WAIT: std::time::Duration = std::time::Duration::from_mins(2);
/// Short in tests, where an unanswered prompt is one of the cases.
#[cfg(test)]
const PROMPT_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

#[cfg(test)]
pub(crate) mod fake;

#[cfg(test)]
thread_local! {
    /// A private bus for this test thread (`fake.rs`), so tests never
    /// touch the desktop's keyring and don't see each other's.
    static TEST_BUS: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// Set only by the ignored tests that use the session's real keyring.
    static REAL_KEYRING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The session bus. In tests: this thread's fake keyring, the real one only
/// where a test asks for it, and otherwise none (every call fails as if no
/// Secret Service ran), so a test can never reach the desktop's keyring by
/// accident.
fn session_bus() -> zbus::Result<Connection> {
    #[cfg(test)]
    {
        if let Some(address) = TEST_BUS.with(|b| b.borrow().clone()) {
            return zbus::blocking::connection::Builder::address(address.as_str())?.build();
        }
        if !REAL_KEYRING.get() {
            return Err(zbus::Error::Address("no keyring in this test".into()));
        }
    }
    Connection::session()
}

/// One keyring call at a time, so the keyring shows at most one unlock
/// prompt. Held by every [`Keyring`] while it is open.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `(session, parameters, value, content type)` from the specification.
type SecretStruct = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsError {
    /// No Secret Service, no default keyring, or a D-Bus error.
    Unavailable(String),
    /// The user dismissed the keyring's unlock prompt.
    Dismissed,
    /// The unlock prompt got no answer in time; it was dismissed.
    NoAnswer,
    /// The stored item isn't one we can read.
    Malformed,
}

impl fmt::Display for SecretsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(e) => write!(f, "keyring unavailable: {e}"),
            Self::Dismissed => write!(f, "keyring unlock dismissed"),
            Self::NoAnswer => write!(f, "keyring unlock prompt not answered in time"),
            Self::Malformed => write!(f, "stored sign-in can't be read"),
        }
    }
}

impl From<zbus::Error> for SecretsError {
    fn from(e: zbus::Error) -> Self {
        // D-Bus error texts name methods and paths, never secret values.
        Self::Unavailable(e.to_string())
    }
}

/// A saved sign-in.
#[derive(Clone)]
pub struct Stored {
    pub identity: Identity,
    pub refresh_token: Secret,
}

impl fmt::Debug for Stored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stored")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct Payload {
    v: u32,
    refresh_token: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

/// Attributes for one account's item.
fn account_attributes<'a>(
    config: &'a ClientConfig,
    identity: &'a Identity,
) -> HashMap<&'a str, &'a str> {
    let mut a = app_attributes(config);
    a.insert("tenant", identity.tenant_id.as_str());
    a.insert("account", identity.object_id.as_str());
    a
}

/// Attributes shared by all our items for this client identity.
fn app_attributes(config: &ClientConfig) -> HashMap<&str, &str> {
    HashMap::from([
        ("application", APPLICATION),
        ("type", KIND),
        ("client", config.client_id.as_str()),
        ("authority", config.authority.as_str()),
    ])
}

fn encode(identity: &Identity, refresh_token: &Secret) -> Vec<u8> {
    serde_json::to_vec(&Payload {
        v: PAYLOAD_VERSION,
        refresh_token: refresh_token.expose().to_owned(),
        username: identity.username.clone(),
        display_name: identity.display_name.clone(),
    })
    .expect("serializable")
}

/// Rebuilds the sign-in from an item's attributes and secret.
fn decode(attributes: &HashMap<String, String>, value: &[u8]) -> Result<Stored, SecretsError> {
    if value.len() > MAX_PAYLOAD {
        return Err(SecretsError::Malformed);
    }
    let p: Payload = serde_json::from_slice(value).map_err(|_| SecretsError::Malformed)?;
    let (Some(tenant_id), Some(object_id)) = (attributes.get("tenant"), attributes.get("account"))
    else {
        return Err(SecretsError::Malformed);
    };
    if p.v != PAYLOAD_VERSION || p.refresh_token.is_empty() {
        return Err(SecretsError::Malformed);
    }
    Ok(Stored {
        identity: Identity {
            tenant_id: tenant_id.clone(),
            object_id: object_id.clone(),
            username: p.username,
            display_name: p.display_name,
        },
        refresh_token: Secret::new(p.refresh_token),
    })
}

struct Keyring {
    conn: Connection,
    session: OwnedObjectPath,
    _turn: std::sync::MutexGuard<'static, ()>,
}

impl Keyring {
    fn open() -> Result<Self, SecretsError> {
        // A call that panicked can't leave the keyring half-used: go on.
        let turn = ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let conn = session_bus()?;
        let service = Proxy::new(&conn, BUS_NAME, SERVICE_PATH, SERVICE)?;
        let (_, session): (OwnedValue, OwnedObjectPath) =
            service.call("OpenSession", &("plain", Value::from("")))?;
        Ok(Self {
            conn,
            session,
            _turn: turn,
        })
    }

    fn proxy<'a>(&'a self, path: &'a str, interface: &'a str) -> Result<Proxy<'a>, SecretsError> {
        Ok(Proxy::new(&self.conn, BUS_NAME, path, interface)?)
    }

    fn service(&self) -> Result<Proxy<'_>, SecretsError> {
        self.proxy(SERVICE_PATH, SERVICE)
    }

    /// Runs a prompt (for example "unlock the keyring") and waits for it,
    /// at most [`PROMPT_WAIT`]; then dismisses it (`Prompt.Dismiss`), so a
    /// forgotten prompt can't keep the app waiting.
    fn prompt(&self, path: &OwnedObjectPath) -> Result<OwnedValue, SecretsError> {
        let prompt = Proxy::new(&self.conn, BUS_NAME, path.clone().into_inner(), PROMPT)?;
        let mut completed = prompt.receive_signal("Completed")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(completed.next());
        });
        prompt.call_method("Prompt", &("",))?;
        let msg = match rx.recv_timeout(PROMPT_WAIT) {
            Ok(Some(msg)) => msg,
            Ok(None) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(SecretsError::Unavailable("prompt ended".into()));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // The service then sends Completed, which ends the thread.
                let _ = prompt.call_method("Dismiss", &());
                return Err(SecretsError::NoAnswer);
            }
        };
        let (dismissed, result): (bool, OwnedValue) = msg.body().deserialize()?;
        if dismissed {
            Err(SecretsError::Dismissed)
        } else {
            Ok(result)
        }
    }

    fn unlock(&self, paths: &[OwnedObjectPath]) -> Result<(), SecretsError> {
        if paths.is_empty() {
            return Ok(());
        }
        let refs: Vec<ObjectPath<'_>> = paths.iter().map(|p| p.as_ref()).collect();
        let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
            self.service()?.call("Unlock", &(refs,))?;
        if prompt.as_str() != "/" {
            self.prompt(&prompt)?;
        }
        Ok(())
    }

    /// Our items matching `attributes`, unlocked.
    fn search(
        &self,
        attributes: &HashMap<&str, &str>,
    ) -> Result<Vec<OwnedObjectPath>, SecretsError> {
        let (mut unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
            self.service()?.call("SearchItems", &(attributes,))?;
        self.unlock(&locked)?;
        unlocked.extend(locked);
        Ok(unlocked)
    }

    /// The default keyring's path; an error when there is none.
    fn default_alias(&self) -> Result<OwnedObjectPath, SecretsError> {
        let path: OwnedObjectPath = self.service()?.call("ReadAlias", &("default",))?;
        if path.as_str() == "/" {
            return Err(SecretsError::Unavailable("no default keyring".into()));
        }
        Ok(path)
    }

    /// The default keyring, unlocked.
    fn default_collection(&self) -> Result<OwnedObjectPath, SecretsError> {
        let path = self.default_alias()?;
        let locked: bool = self
            .proxy(path.as_str(), COLLECTION)?
            .get_property("Locked")?;
        if locked {
            self.unlock(std::slice::from_ref(&path))?;
        }
        Ok(path)
    }

    fn read(&self, item: &OwnedObjectPath) -> Result<Stored, SecretsError> {
        let attributes: HashMap<String, String> = self
            .proxy(item.as_str(), ITEM)?
            .get_property("Attributes")?;
        decode(&attributes, &self.read_secret(item)?)
    }

    /// An item's raw secret.
    fn read_secret(&self, item: &OwnedObjectPath) -> Result<Vec<u8>, SecretsError> {
        let (_, _, value, _): SecretStruct = self
            .proxy(item.as_str(), ITEM)?
            .call("GetSecret", &(&self.session,))?;
        Ok(value)
    }

    fn delete(&self, item: &OwnedObjectPath) -> Result<(), SecretsError> {
        let prompt: OwnedObjectPath = self.proxy(item.as_str(), ITEM)?.call("Delete", &())?;
        if prompt.as_str() != "/" {
            self.prompt(&prompt)?;
        }
        Ok(())
    }

    /// Deletes every item matching `attributes`.
    fn delete_matching(&self, attributes: &HashMap<&str, &str>) -> Result<(), SecretsError> {
        for item in self.search(attributes)? {
            self.delete(&item)?;
        }
        Ok(())
    }

    /// Stores `secret` in the default keyring, replacing an item with the
    /// same attributes.
    fn create_item(
        &self,
        label: String,
        attributes: &HashMap<&str, &str>,
        secret: Vec<u8>,
    ) -> Result<(), SecretsError> {
        let collection = self.default_collection()?;
        let properties: HashMap<&str, Value<'_>> = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from(label)),
            (
                "org.freedesktop.Secret.Item.Attributes",
                Value::from(attributes.clone()),
            ),
        ]);
        let secret = (
            self.session.as_ref(),
            Vec::<u8>::new(),
            secret,
            // `text/plain`: oo7 0.6 rejects other types such as
            // `application/json` (newer oo7 and gnome-keyring accept any).
            "text/plain",
        );
        let (item, prompt): (OwnedObjectPath, OwnedObjectPath) = self
            .proxy(collection.as_str(), COLLECTION)?
            .call("CreateItem", &(properties, secret, true))?;
        if item.as_str() == "/" && prompt.as_str() != "/" {
            self.prompt(&prompt)?;
        }
        Ok(())
    }
}

/// Whether a Secret Service answers and has a default keyring.
pub fn check() -> Result<(), SecretsError> {
    Keyring::open()?.default_alias().map(drop)
}

/// Loads every saved sign-in for this client identity, one per account.
/// Items that can't be read are skipped (and noted in the log without
/// contents); the user signs in to that account again.
pub fn load_all(config: &ClientConfig) -> Result<Vec<Stored>, SecretsError> {
    let keyring = Keyring::open()?;
    let items = keyring.search(&app_attributes(config))?;
    Ok(one_per_account(items.iter().map(|item| keyring.read(item))))
}

/// The readable items, the first one per account (tenant and object ID);
/// unreadable ones are skipped and logged without contents.
fn one_per_account(items: impl Iterator<Item = Result<Stored, SecretsError>>) -> Vec<Stored> {
    let mut accounts: Vec<Stored> = Vec::new();
    for item in items {
        match item {
            Ok(stored) => {
                let same = |s: &Stored| {
                    s.identity.tenant_id == stored.identity.tenant_id
                        && s.identity.object_id == stored.identity.object_id
                };
                if !accounts.iter().any(same) {
                    accounts.push(stored);
                }
            }
            Err(e) => crate::app_log!("saved sign-in: skipped an item ({e})"),
        }
    }
    accounts
}

/// Saves (or replaces) one account's refresh token. Other accounts' items
/// are left alone.
pub fn save(
    config: &ClientConfig,
    identity: &Identity,
    refresh_token: &Secret,
) -> Result<(), SecretsError> {
    let label = match identity.username.as_deref() {
        Some(user) => format!("Deskfeed sign-in ({user})"),
        None => "Deskfeed sign-in".to_owned(),
    };
    Keyring::open()?.create_item(
        label,
        &account_attributes(config, identity),
        encode(identity, refresh_token),
    )
}

/// Deletes one account's saved sign-in (sign-out of that account).
pub fn delete(config: &ClientConfig, identity: &Identity) -> Result<(), SecretsError> {
    Keyring::open()?.delete_matching(&account_attributes(config, identity))
}

/// Attributes of an account's remembered passwords; with `desktop`, of
/// one desktop's (`desktop` is the opaque resource key).
fn password_attributes<'a>(
    identity: &'a Identity,
    desktop: Option<&'a str>,
) -> HashMap<&'a str, &'a str> {
    let mut a = HashMap::from([
        ("application", APPLICATION),
        ("type", PASSWORD_KIND),
        ("tenant", identity.tenant_id.as_str()),
        ("account", identity.object_id.as_str()),
    ]);
    if let Some(d) = desktop {
        a.insert("desktop", d);
    }
    a
}

/// Remembers a desktop's Windows password, replacing an earlier one.
/// The label names the desktop, so the user recognises it in a keyring
/// manager.
pub fn save_password(
    identity: &Identity,
    desktop: &str,
    desktop_title: &str,
    password: &Secret,
) -> Result<(), SecretsError> {
    save_text(
        "Deskfeed Windows password",
        desktop_title,
        &password_attributes(identity, Some(desktop)),
        password,
    )
}

/// A desktop's remembered password, if there is one.
pub fn load_password(identity: &Identity, desktop: &str) -> Result<Option<Secret>, SecretsError> {
    load_text(&password_attributes(identity, Some(desktop)))
}

/// Attributes of a plain RDP connection's remembered password (ADR 0029;
/// `connection` is its opaque ID). Its own `type`, so neither an account's
/// searches nor these ever match the other's items.
fn connection_password_attributes(connection: &str) -> HashMap<&str, &str> {
    HashMap::from([
        ("application", APPLICATION),
        ("type", CONNECTION_PASSWORD_KIND),
        ("connection", connection),
    ])
}

/// Remembers a plain RDP connection's password, replacing an earlier one.
pub fn save_connection_password(
    connection: &str,
    title: &str,
    password: &Secret,
) -> Result<(), SecretsError> {
    save_text(
        "Deskfeed RDP password",
        title,
        &connection_password_attributes(connection),
        password,
    )
}

/// A plain RDP connection's remembered password, if there is one.
pub fn load_connection_password(connection: &str) -> Result<Option<Secret>, SecretsError> {
    load_text(&connection_password_attributes(connection))
}

/// Forgets a plain RDP connection's password (Forget, or removing it).
pub fn delete_connection_password(connection: &str) -> Result<(), SecretsError> {
    Keyring::open()?.delete_matching(&connection_password_attributes(connection))
}

/// Stores `password` under `attributes`, replacing an item with the same
/// ones, labelled "`kind` (`title`)" so the user recognises it in a
/// keyring manager.
fn save_text(
    kind: &str,
    title: &str,
    attributes: &HashMap<&str, &str>,
    password: &Secret,
) -> Result<(), SecretsError> {
    if password.expose().len() > MAX_PASSWORD {
        return Err(SecretsError::Malformed);
    }
    let title: String = title.chars().filter(|c| !c.is_control()).take(80).collect();
    Keyring::open()?.create_item(
        format!("{kind} ({title})"),
        attributes,
        password.expose().as_bytes().to_vec(),
    )
}

/// The password stored under `attributes`, if any; one that isn't a
/// password (too long, a NUL, not UTF-8) is `Malformed`.
fn load_text(attributes: &HashMap<&str, &str>) -> Result<Option<Secret>, SecretsError> {
    let keyring = Keyring::open()?;
    let Some(item) = keyring.search(attributes)?.into_iter().next() else {
        return Ok(None);
    };
    let value = keyring.read_secret(&item)?;
    if value.len() > MAX_PASSWORD || value.contains(&0) {
        return Err(SecretsError::Malformed);
    }
    let text = String::from_utf8(value).map_err(|_| SecretsError::Malformed)?;
    Ok(Some(Secret::new(text)))
}

/// Forgets a desktop's password, or with `None` all of the account's
/// (sign-out).
pub fn delete_passwords(identity: &Identity, desktop: Option<&str>) -> Result<(), SecretsError> {
    Keyring::open()?.delete_matching(&password_attributes(identity, desktop))
}

/// Deletes all of the app's saved sign-ins for this client identity.
#[cfg(test)]
pub fn delete_all(config: &ClientConfig) -> Result<(), SecretsError> {
    Keyring::open()?.delete_matching(&app_attributes(config))
}

#[cfg(test)]
mod tests {
    use super::fake::{Answer, FakeKeyring};
    use super::*;

    fn identity() -> Identity {
        Identity {
            tenant_id: "11111111-2222-3333-4444-555555555555".into(),
            object_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            username: Some("anna@example.com".into()),
            display_name: Some("Anna".into()),
        }
    }

    fn attrs_of(config: &ClientConfig, id: &Identity) -> HashMap<String, String> {
        account_attributes(config, id)
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }

    #[test]
    fn loading_skips_unreadable_items_and_keeps_one_per_account() {
        // The keyring test that covers loading is ignored without a
        // keyring; this covers the rule itself.
        let stored = |object: &str, token: &str| Stored {
            identity: Identity {
                object_id: object.into(),
                ..identity()
            },
            refresh_token: Secret::new(token.into()),
        };
        let got = one_per_account(
            [
                Err(SecretsError::Malformed),
                Ok(stored("a", "first")),
                Ok(stored("b", "other account")),
                Ok(stored("a", "duplicate")),
                Err(SecretsError::Malformed),
            ]
            .into_iter(),
        );
        let seen: Vec<(&str, &str)> = got
            .iter()
            .map(|s| (s.identity.object_id.as_str(), s.refresh_token.expose()))
            .collect();
        // The first item of an account wins; an unreadable one is skipped,
        // not an error for the others.
        assert_eq!(seen, [("a", "first"), ("b", "other account")]);
        assert!(one_per_account(std::iter::empty()).is_empty());
    }

    #[test]
    fn attributes_separate_accounts_and_clients() {
        let config = ClientConfig::public_remote_desktop();
        let id = identity();
        let a = account_attributes(&config, &id);
        assert_eq!(a["application"], APPLICATION);
        assert_eq!(a["client"], "a85cf173-4192-42f8-81fa-777a763e6e2c");
        assert_eq!(
            a["authority"],
            "https://login.microsoftonline.com/organizations"
        );
        assert_eq!(a["tenant"], "11111111-2222-3333-4444-555555555555");
        assert_eq!(a["account"], "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        // No secret and no personal name in the searchable attributes.
        assert!(!a.values().any(|v| v.contains("anna") || v.contains("rt-")));
    }

    #[test]
    fn round_trip() {
        let config = ClientConfig::public_remote_desktop();
        let value = encode(&identity(), &Secret::new("rt-secret".into()));
        let stored = decode(&attrs_of(&config, &identity()), &value).unwrap();
        assert_eq!(stored.identity, identity());
        assert_eq!(stored.refresh_token.expose(), "rt-secret");
        // Debug output never shows the token.
        assert!(!format!("{stored:?}").contains("rt-secret"));
    }

    /// On a fake keyring (`fake.rs`); the desktop's is never touched.
    #[test]
    fn keyring_round_trip() {
        let _keyring = FakeKeyring::start(Answer::Unlock);
        let mut config = ClientConfig::public_remote_desktop();
        config.client_id = "00000000-0000-0000-0000-00000000test".into();
        delete_all(&config).unwrap();
        assert!(load_all(&config).unwrap().is_empty());
        save(&config, &identity(), &Secret::new("rt-one".into())).unwrap();
        // Replacing keeps one item per account.
        save(&config, &identity(), &Secret::new("rt-two".into())).unwrap();
        let all = load_all(&config).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].identity, identity());
        assert_eq!(all[0].refresh_token.expose(), "rt-two");
        // A second account is kept next to the first.
        let mut other = identity();
        other.object_id = "ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee".into();
        save(&config, &other, &Secret::new("rt-three".into())).unwrap();
        assert_eq!(load_all(&config).unwrap().len(), 2);
        // Deleting one leaves the other.
        delete(&config, &identity()).unwrap();
        let all = load_all(&config).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].identity, other);
        delete_all(&config).unwrap();
        assert!(load_all(&config).unwrap().is_empty());
    }

    #[test]
    fn a_locked_keyring_is_unlocked_through_its_prompt() {
        let keyring = FakeKeyring::start(Answer::Unlock);
        let config = ClientConfig::public_remote_desktop();
        save(&config, &identity(), &Secret::new("rt".into())).unwrap();
        keyring.set_locked(true);
        let all = load_all(&config).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].refresh_token.expose(), "rt");
        assert_eq!(keyring.prompts(), (1, 0));
        assert!(!keyring.locked());
    }

    #[test]
    fn a_cancelled_unlock_is_dismissed_not_an_empty_keyring() {
        let keyring = FakeKeyring::start(Answer::Cancel);
        let config = ClientConfig::public_remote_desktop();
        save(&config, &identity(), &Secret::new("rt".into())).unwrap();
        keyring.set_locked(true);
        // Not "nothing saved": the caller must not treat it as signed out.
        assert_eq!(load_all(&config).unwrap_err(), SecretsError::Dismissed);
        assert_eq!(
            delete(&config, &identity()).unwrap_err(),
            SecretsError::Dismissed
        );
        assert_eq!(
            save(&config, &identity(), &Secret::new("x".into())).unwrap_err(),
            SecretsError::Dismissed
        );
        // Nothing was changed behind the user's back.
        keyring.set_locked(false);
        assert_eq!(load_all(&config).unwrap()[0].refresh_token.expose(), "rt");
    }

    #[test]
    fn an_unanswered_prompt_is_given_up_and_dismissed() {
        let keyring = FakeKeyring::start(Answer::Never);
        let config = ClientConfig::public_remote_desktop();
        save(&config, &identity(), &Secret::new("rt".into())).unwrap();
        keyring.set_locked(true);
        assert_eq!(load_all(&config).unwrap_err(), SecretsError::NoAnswer);
        // The app closed the prompt itself, so it doesn't linger.
        assert_eq!(keyring.prompts(), (1, 1));
    }

    #[test]
    fn no_default_keyring_is_unavailable() {
        let keyring = FakeKeyring::start(Answer::Unlock);
        keyring.set_no_default();
        let config = ClientConfig::public_remote_desktop();
        assert!(matches!(check(), Err(SecretsError::Unavailable(_))));
        assert!(matches!(
            save(&config, &identity(), &Secret::new("rt".into())),
            Err(SecretsError::Unavailable(_))
        ));
        assert!(keyring.items().is_empty());
    }

    #[test]
    fn items_are_labelled_for_people() {
        let keyring = FakeKeyring::start(Answer::Unlock);
        let config = ClientConfig::public_remote_desktop();
        save(&config, &identity(), &Secret::new("rt".into())).unwrap();
        save_password(
            &identity(),
            "desk-1",
            "My\ndesktop",
            &Secret::new("pw".into()),
        )
        .unwrap();
        let mut labels: Vec<String> = keyring.items().into_iter().map(|i| i.label).collect();
        labels.sort();
        assert_eq!(
            labels,
            [
                // The line break in the desktop's name is dropped.
                "Deskfeed Windows password (Mydesktop)",
                "Deskfeed sign-in (anna@example.com)"
            ]
        );
    }

    #[test]
    fn remembered_passwords_per_desktop() {
        let keyring = FakeKeyring::start(Answer::Unlock);
        let config = ClientConfig::public_remote_desktop();
        let id = identity();
        save(&config, &id, &Secret::new("rt".into())).unwrap();
        assert!(load_password(&id, "desk-1").unwrap().is_none());
        save_password(&id, "desk-1", "One", &Secret::new("pw-1".into())).unwrap();
        // Replacing keeps one item per desktop.
        save_password(&id, "desk-1", "One", &Secret::new("pw-1b".into())).unwrap();
        save_password(&id, "desk-2", "Two", &Secret::new("pw-2".into())).unwrap();
        assert_eq!(
            load_password(&id, "desk-1").unwrap().unwrap().expose(),
            "pw-1b"
        );
        assert_eq!(keyring.items().len(), 3);
        // Forgetting one desktop's leaves the other and the sign-in.
        delete_passwords(&id, Some("desk-1")).unwrap();
        assert!(load_password(&id, "desk-1").unwrap().is_none());
        assert_eq!(
            load_password(&id, "desk-2").unwrap().unwrap().expose(),
            "pw-2"
        );
        // Sign-out removes all of them, still not the sign-in itself.
        delete_passwords(&id, None).unwrap();
        assert!(load_password(&id, "desk-2").unwrap().is_none());
        assert_eq!(load_all(&config).unwrap().len(), 1);
        // A password longer than any Windows allows isn't stored.
        let long = Secret::new("x".repeat(MAX_PASSWORD + 1));
        assert_eq!(
            save_password(&id, "desk-3", "Three", &long).unwrap_err(),
            SecretsError::Malformed
        );
    }

    /// A plain RDP connection's password (ADR 0029): saved, replaced, read
    /// and forgotten, labelled for people, and apart from an account's
    /// items, so a sign-out never deletes it and removing it never deletes
    /// those.
    #[test]
    fn connection_passwords_are_their_own_items() {
        let keyring = FakeKeyring::start(Answer::Unlock);
        let config = ClientConfig::public_remote_desktop();
        let id = identity();
        save(&config, &id, &Secret::new("rt".into())).unwrap();
        save_password(&id, "desk-1", "One", &Secret::new("pw-desk".into())).unwrap();
        assert!(load_connection_password("rdp-1").unwrap().is_none());
        save_connection_password("rdp-1", "Office\nPC", &Secret::new("pw-a".into())).unwrap();
        save_connection_password("rdp-1", "Office PC", &Secret::new("pw-b".into())).unwrap();
        save_connection_password("rdp-2", "Lab", &Secret::new("pw-c".into())).unwrap();
        assert_eq!(
            load_connection_password("rdp-1").unwrap().unwrap().expose(),
            "pw-b"
        );
        assert_eq!(keyring.items().len(), 4, "replaced, not added");
        assert!(
            keyring
                .items()
                .iter()
                .any(|i| i.label == "Deskfeed RDP password (Office PC)")
        );
        // The account's sign-out leaves the connections' passwords.
        delete_passwords(&id, None).unwrap();
        assert!(load_connection_password("rdp-1").unwrap().is_some());
        // Forgetting one connection's leaves the other's and the sign-in.
        save_password(&id, "desk-1", "One", &Secret::new("pw-desk".into())).unwrap();
        delete_connection_password("rdp-1").unwrap();
        assert!(load_connection_password("rdp-1").unwrap().is_none());
        assert!(load_connection_password("rdp-2").unwrap().is_some());
        assert!(load_password(&id, "desk-1").unwrap().is_some());
        assert_eq!(load_all(&config).unwrap().len(), 1);
        // Too long for any Windows password: not stored.
        let long = Secret::new("x".repeat(MAX_PASSWORD + 1));
        assert_eq!(
            save_connection_password("rdp-3", "Long", &long).unwrap_err(),
            SecretsError::Malformed
        );
    }

    #[test]
    fn connection_password_attributes_match_no_account_search() {
        let id = identity();
        let config = ClientConfig::public_remote_desktop();
        let connection = connection_password_attributes("rdp-1");
        for search in [
            account_attributes(&config, &id),
            password_attributes(&id, None),
        ] {
            assert!(
                !search.iter().all(|(k, v)| connection.get(k) == Some(v)),
                "{search:?} would match a connection's password"
            );
        }
        // And a connection's search never matches an account's items.
        assert!(
            !connection
                .iter()
                .all(|(k, v)| password_attributes(&id, Some("rdp-1")).get(k) == Some(v))
        );
    }

    #[test]
    fn rejects_bad_items() {
        let config = ClientConfig::public_remote_desktop();
        let attrs = attrs_of(&config, &identity());
        assert_eq!(
            decode(&attrs, b"not json").unwrap_err(),
            SecretsError::Malformed
        );
        assert_eq!(
            decode(&attrs, br#"{"v":2,"refresh_token":"x"}"#).unwrap_err(),
            SecretsError::Malformed
        );
        assert_eq!(
            decode(&attrs, br#"{"v":1,"refresh_token":""}"#).unwrap_err(),
            SecretsError::Malformed
        );
        assert_eq!(
            decode(&HashMap::new(), br#"{"v":1,"refresh_token":"x"}"#).unwrap_err(),
            SecretsError::Malformed
        );
    }

    #[test]
    fn password_items_are_apart_from_sign_ins() {
        let id = identity();
        let config = ClientConfig::public_remote_desktop();
        let sign_in = account_attributes(&config, &id);
        let one = password_attributes(&id, Some("key1"));
        let all = password_attributes(&id, None);
        // A search for sign-ins never matches a password, and back.
        assert_ne!(sign_in.get("type"), one.get("type"));
        // Sign-out's search (no desktop) matches every desktop's item.
        assert!(all.iter().all(|(k, v)| one.get(k) == Some(v)));
        assert_eq!(one.get("desktop"), Some(&"key1"));
        // Nothing secret or personal in the searchable attributes.
        assert!(!one.values().any(|v| v.contains('@')));
    }

    /// Against the real keyring: `cargo test --release
    /// real_keyring_password -- --ignored`. Leaves nothing behind.
    #[test]
    #[ignore = "uses the session's real keyring"]
    fn real_keyring_password() {
        REAL_KEYRING.set(true);
        let mut id = identity();
        id.object_id = "00000000-0000-0000-0000-00000000test".into();
        save_password(
            &id,
            "desk-test",
            "Test desktop",
            &Secret::new("p@ss w0rd".into()),
        )
        .unwrap();
        let back = load_password(&id, "desk-test").unwrap().unwrap();
        assert_eq!(back.expose(), "p@ss w0rd");
        // Replaced, not added.
        save_password(
            &id,
            "desk-test",
            "Test desktop",
            &Secret::new("second".into()),
        )
        .unwrap();
        assert_eq!(
            load_password(&id, "desk-test").unwrap().unwrap().expose(),
            "second"
        );
        assert!(load_password(&id, "other").unwrap().is_none());
        delete_passwords(&id, None).unwrap();
        assert!(load_password(&id, "desk-test").unwrap().is_none());
        // The sign-in list never shows it.
        assert!(
            load_all(&ClientConfig::public_remote_desktop())
                .unwrap()
                .iter()
                .all(|s| s.identity.object_id != id.object_id)
        );
    }
}
