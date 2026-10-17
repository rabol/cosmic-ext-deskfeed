// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;

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
    /// of account, tenant and resource ID (spec §7: titles are never
    /// identity). No credentials are stored here.
    pub logon_settings: BTreeMap<String, LogonSetting>,
    /// Connection defaults per account (keys: `accounts::account_id`).
    pub account_settings: BTreeMap<String, ConnectionSettings>,
    /// Per-desktop overrides (keys: resource keys, as above).
    pub desktop_settings: BTreeMap<String, ConnectionSettings>,
}
