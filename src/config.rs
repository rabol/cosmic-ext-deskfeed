// SPDX-License-Identifier: MIT

use std::collections::{BTreeMap, BTreeSet};

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};
use serde::{Deserialize, Serialize};

use crate::sessions::launcher::LogonSetting;
use crate::settings::ConnectionSettings;

#[derive(Debug, Default, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
// The field names are the keys stored on disk; renaming them would lose
// users' settings.
#[allow(clippy::struct_field_names)]
pub struct Config {
    /// Logon setting that last worked, per resource. Keys are opaque hashes
    /// of account, tenant, workspace and resource ID (spec §7: titles are never
    /// identity). No credentials are stored here.
    pub logon_settings: BTreeMap<String, LogonSetting>,
    /// Session host certificates the user approved, per desktop (resource
    /// keys; sha256 fingerprints), ADR 0016.
    pub certificate_pins: BTreeMap<String, String>,
    /// Desktops with a Windows password remembered in the keyring (resource
    /// keys; the passwords themselves are only in the keyring).
    pub saved_passwords: BTreeSet<String>,
    /// Favorite desktops (resource keys, as above).
    pub favorites: BTreeSet<String>,
    /// Keep running with the window closed, with a tray icon (App settings
    /// → Background, ADR 0017). Off by default (spec §5).
    pub background: bool,
    /// Deskfeed's own look (App settings → Appearance): 0 or none follows
    /// the desktop, 1 dark, 2 light.
    pub app_theme: Option<u32>,
    /// Log rotation size in MiB (App settings → Diagnostics); `None` is
    /// the default (`log::MAX_SIZE_MIB`).
    pub log_max_mib: Option<u32>,
    /// Connection defaults for every account (App settings).
    pub app_settings: ConnectionSettings,
    /// Overrides of the app's settings per account (keys: `accounts::account_id`).
    pub account_settings: BTreeMap<String, ConnectionSettings>,
    /// Per-desktop overrides (keys: resource keys, as above).
    pub desktop_settings: BTreeMap<String, ConnectionSettings>,
    /// The accounts in the sidebar, signed in or signed out, until removed
    /// (keys: `accounts::account_id`). Names and IDs only: what's secret
    /// is in the keyring.
    pub accounts: BTreeMap<String, KnownAccount>,
    /// Saved plain RDP connections (ADR 0029; keys: `connections::new_id`).
    /// No passwords: those are in the keyring, opt-in.
    pub connections: BTreeMap<String, crate::connections::SavedConnection>,
    /// A local folder shared with a desktop or saved connection (keys as
    /// above; absolute paths). Off unless chosen (ADR 0029, compared with
    /// Remmina).
    pub shared_folders: BTreeMap<String, String>,
}

/// An account Deskfeed lists: who it is, for the sidebar and for signing
/// in again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownAccount {
    pub tenant_id: String,
    pub object_id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

impl From<&crate::auth::id_token::Identity> for KnownAccount {
    fn from(i: &crate::auth::id_token::Identity) -> Self {
        Self {
            tenant_id: i.tenant_id.clone(),
            object_id: i.object_id.clone(),
            username: i.username.clone(),
            display_name: i.display_name.clone(),
        }
    }
}

impl From<&KnownAccount> for crate::auth::id_token::Identity {
    fn from(k: &KnownAccount) -> Self {
        Self {
            tenant_id: k.tenant_id.clone(),
            object_id: k.object_id.clone(),
            username: k.username.clone(),
            display_name: k.display_name.clone(),
        }
    }
}
