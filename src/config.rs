// SPDX-License-Identifier: MIT

use std::collections::{BTreeMap, BTreeSet};

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

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
    /// Favorite desktops (resource keys, as above).
    pub favorites: BTreeSet<String>,
    /// Log rotation size in MiB (App settings → Diagnostics); `None` is
    /// the default (`log::MAX_SIZE_MIB`).
    pub log_max_mib: Option<u32>,
    /// Connection defaults for every account (App settings).
    pub app_settings: ConnectionSettings,
    /// Overrides of the app's settings per account (keys: `accounts::account_id`).
    pub account_settings: BTreeMap<String, ConnectionSettings>,
    /// Per-desktop overrides (keys: resource keys, as above).
    pub desktop_settings: BTreeMap<String, ConnectionSettings>,
}
