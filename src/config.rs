// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

use crate::sessions::launcher::LogonSetting;

#[derive(Debug, Default, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    /// Logon setting that last worked, per resource. Keys are opaque hashes
    /// of account, tenant and resource ID (spec §7: titles are never
    /// identity). No credentials are stored here.
    pub logon_settings: BTreeMap<String, LogonSetting>,
}
