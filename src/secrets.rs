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
const APPLICATION: &str = "io.github.rabol.cosmic-ext-avd";
const KIND: &str = "refresh-token";
const PAYLOAD_VERSION: u32 = 1;
const MAX_PAYLOAD: usize = 64 * 1024;

/// `(session, parameters, value, content type)` from the specification.
type SecretStruct = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsError {
    /// No Secret Service, no default keyring, or a D-Bus error.
    Unavailable(String),
    /// The user dismissed the keyring's unlock prompt.
    Dismissed,
    /// The stored item isn't one we can read.
    Malformed,
}

impl fmt::Display for SecretsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(e) => write!(f, "keyring unavailable: {e}"),
            Self::Dismissed => write!(f, "keyring unlock dismissed"),
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
}

impl Keyring {
    fn open() -> Result<Self, SecretsError> {
        let conn = Connection::session()?;
        let service = Proxy::new(&conn, BUS_NAME, SERVICE_PATH, SERVICE)?;
        let (_, session): (OwnedValue, OwnedObjectPath) =
            service.call("OpenSession", &("plain", Value::from("")))?;
        Ok(Self { conn, session })
    }

    fn proxy<'a>(&'a self, path: &'a str, interface: &'a str) -> Result<Proxy<'a>, SecretsError> {
        Ok(Proxy::new(&self.conn, BUS_NAME, path, interface)?)
    }

    fn service(&self) -> Result<Proxy<'_>, SecretsError> {
        self.proxy(SERVICE_PATH, SERVICE)
    }

    /// Runs a prompt (for example "unlock the keyring") and waits for it.
    fn prompt(&self, path: &OwnedObjectPath) -> Result<OwnedValue, SecretsError> {
        let prompt = self.proxy(path.as_str(), PROMPT)?;
        let mut completed = prompt.receive_signal("Completed")?;
        prompt.call_method("Prompt", &("",))?;
        let msg = completed
            .next()
            .ok_or_else(|| SecretsError::Unavailable("prompt ended".into()))?;
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

    fn default_collection(&self) -> Result<OwnedObjectPath, SecretsError> {
        let path: OwnedObjectPath = self.service()?.call("ReadAlias", &("default",))?;
        if path.as_str() == "/" {
            return Err(SecretsError::Unavailable("no default keyring".into()));
        }
        let locked: bool = self
            .proxy(path.as_str(), COLLECTION)?
            .get_property("Locked")?;
        if locked {
            self.unlock(std::slice::from_ref(&path))?;
        }
        Ok(path)
    }

    fn read(&self, item: &OwnedObjectPath) -> Result<Stored, SecretsError> {
        let proxy = self.proxy(item.as_str(), ITEM)?;
        let attributes: HashMap<String, String> = proxy.get_property("Attributes")?;
        let (_, _, value, _): SecretStruct = proxy.call("GetSecret", &(&self.session,))?;
        decode(&attributes, &value)
    }

    fn delete(&self, item: &OwnedObjectPath) -> Result<(), SecretsError> {
        let prompt: OwnedObjectPath = self.proxy(item.as_str(), ITEM)?.call("Delete", &())?;
        if prompt.as_str() != "/" {
            self.prompt(&prompt)?;
        }
        Ok(())
    }
}

/// Whether a Secret Service answers and has a default keyring.
pub fn check() -> Result<(), SecretsError> {
    let keyring = Keyring::open()?;
    let path: OwnedObjectPath = keyring.service()?.call("ReadAlias", &("default",))?;
    if path.as_str() == "/" {
        return Err(SecretsError::Unavailable("no default keyring".into()));
    }
    Ok(())
}

/// Loads every saved sign-in for this client identity, one per account.
/// Items that can't be read are skipped (and reported on stderr without
/// contents); the user signs in to that account again.
pub fn load_all(config: &ClientConfig) -> Result<Vec<Stored>, SecretsError> {
    let keyring = Keyring::open()?;
    let mut accounts: Vec<Stored> = Vec::new();
    for item in keyring.search(&app_attributes(config))? {
        match keyring.read(&item) {
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
    Ok(accounts)
}

/// Saves (or replaces) one account's refresh token. Other accounts' items
/// are left alone.
pub fn save(
    config: &ClientConfig,
    identity: &Identity,
    refresh_token: &Secret,
) -> Result<(), SecretsError> {
    let keyring = Keyring::open()?;
    let collection = keyring.default_collection()?;
    let attributes = account_attributes(config, identity);
    let label = match identity.username.as_deref() {
        Some(user) => format!("AVD Client sign-in ({user})"),
        None => "AVD Client sign-in".to_owned(),
    };
    let properties: HashMap<&str, Value<'_>> = HashMap::from([
        ("org.freedesktop.Secret.Item.Label", Value::from(label)),
        (
            "org.freedesktop.Secret.Item.Attributes",
            Value::from(attributes.clone()),
        ),
    ]);
    let secret = (
        keyring.session.as_ref(),
        Vec::<u8>::new(),
        encode(identity, refresh_token),
        // `text/plain`: oo7 0.6 rejects other types such as
        // `application/json` (newer oo7 and gnome-keyring accept any).
        "text/plain",
    );
    let (item, prompt): (OwnedObjectPath, OwnedObjectPath) = keyring
        .proxy(collection.as_str(), COLLECTION)?
        .call("CreateItem", &(properties, secret, true))?;
    if item.as_str() == "/" && prompt.as_str() != "/" {
        keyring.prompt(&prompt)?;
    }
    Ok(())
}

/// Deletes one account's saved sign-in (sign-out of that account).
pub fn delete(config: &ClientConfig, identity: &Identity) -> Result<(), SecretsError> {
    let keyring = Keyring::open()?;
    for item in keyring.search(&account_attributes(config, identity))? {
        keyring.delete(&item)?;
    }
    Ok(())
}

/// Deletes all of the app's saved sign-ins for this client identity.
#[cfg(test)]
pub fn delete_all(config: &ClientConfig) -> Result<(), SecretsError> {
    let keyring = Keyring::open()?;
    for item in keyring.search(&app_attributes(config))? {
        keyring.delete(&item)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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

    /// Against the real Secret Service: `cargo test -- --ignored keyring`.
    /// Uses a test client ID, so the app's real items are never touched.
    #[test]
    #[ignore = "uses the desktop keyring"]
    fn keyring_round_trip() {
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
}
