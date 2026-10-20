// SPDX-License-Identifier: MIT

//! Saved plain RDP connections (ADR 0029): a Windows machine by host name,
//! as the user entered it in "Add connection", checked. The password is
//! never here; it is asked for, or remembered in the keyring.
//!
//! Every field ends up in FreeRDP's arguments or connection file, one per
//! line, so the checks refuse anything that could change their meaning:
//! control characters, spaces, commas, user info in the host.

use std::net::{Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

use crate::sessions::launcher::{LogonSetting, valid_domain};

/// RDP's port.
pub const DEFAULT_PORT: u16 = 3389;
const MAX_HOST: usize = 253;
const MAX_NAME: usize = 80;
/// As the launcher's limit for `/u:`.
const MAX_USER_NAME: usize = 1024;

/// A saved connection (config `connections`, keyed by [`new_id`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedConnection {
    /// What the list shows; empty shows the host.
    #[serde(default)]
    pub name: String,
    /// A DNS name, or an IPv4 or IPv6 address (without brackets).
    pub host: String,
    pub port: u16,
    /// Empty: the password dialog asks for it.
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub logon: LogonSetting,
}

/// What's wrong with a connection's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionError {
    NoHost,
    /// Not a host name or an IP address.
    BadHost,
    /// Not a number from 1 to 65535.
    BadPort,
    BadUserName,
    BadName,
    /// The typed domain isn't one (letters, digits, `.`, `-`, `_`).
    BadDomain,
    /// Entra sign-in is for Azure Virtual Desktop only (ADR 0029).
    EntraNotAvailable,
}

impl SavedConnection {
    /// The connection from the form's fields, trimmed: an empty port is
    /// RDP's; a host typed as `[address]` loses its brackets.
    pub fn from_form(
        name: &str,
        host: &str,
        port: &str,
        username: &str,
        logon: LogonSetting,
    ) -> Result<Self, ConnectionError> {
        let host = host.trim();
        let host = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        let port = match port.trim() {
            "" => DEFAULT_PORT,
            p => p
                .parse::<u16>()
                .ok()
                .filter(|&p| p != 0)
                .ok_or(ConnectionError::BadPort)?,
        };
        let logon = match logon {
            LogonSetting::CustomDomain(d) => LogonSetting::CustomDomain(d.trim().to_owned()),
            other => other,
        };
        let connection = Self {
            name: name.trim().to_owned(),
            host: host.to_owned(),
            port,
            username: username.trim().to_owned(),
            logon,
        };
        connection.check()?;
        Ok(connection)
    }

    /// Checks every field; run again before each launch, since the
    /// settings file can be edited by hand.
    pub fn check(&self) -> Result<(), ConnectionError> {
        if self.host.is_empty() {
            return Err(ConnectionError::NoHost);
        }
        if !valid_host(&self.host) {
            return Err(ConnectionError::BadHost);
        }
        if self.port == 0 {
            return Err(ConnectionError::BadPort);
        }
        if self.username.len() > MAX_USER_NAME || self.username.chars().any(char::is_control) {
            return Err(ConnectionError::BadUserName);
        }
        if self.name.chars().count() > MAX_NAME || self.name.chars().any(char::is_control) {
            return Err(ConnectionError::BadName);
        }
        match &self.logon {
            LogonSetting::CustomDomain(d) if !valid_domain(d) => Err(ConnectionError::BadDomain),
            LogonSetting::EntraSignIn => Err(ConnectionError::EntraNotAvailable),
            _ => Ok(()),
        }
    }

    /// `host:port`, an IPv6 address in brackets, as FreeRDP takes it.
    pub fn address(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// The name, or the host without one.
    pub fn title(&self) -> &str {
        if self.name.is_empty() {
            &self.host
        } else {
            &self.name
        }
    }
}

/// A DNS name (letters, digits and `-` in labels of at most 63, not
/// starting or ending with `-`, at most 253 in all; a single label such as
/// a Windows computer name too), an IPv4 address, or an IPv6 address.
/// Names with other characters (international ones included) aren't
/// accepted; their `xn--` form is.
fn valid_host(host: &str) -> bool {
    if host.len() > MAX_HOST {
        return false;
    }
    if host.parse::<Ipv4Addr>().is_ok() || host.parse::<Ipv6Addr>().is_ok() {
        return true;
    }
    // All digits and dots, but not an IPv4 address: a typo, not a name.
    if host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// An opaque, random ID for a new connection. It names the connection in
/// the settings and the keyring, never its host, and can't be taken for an
/// Azure desktop's key (those are base64 hashes, never with this prefix).
pub fn new_id() -> Option<String> {
    use std::fmt::Write as _;
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().fold("rdp-".to_owned(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(host: &str, port: &str) -> Result<SavedConnection, ConnectionError> {
        SavedConnection::from_form("", host, port, "", LogonSetting::Automatic)
    }

    #[test]
    fn hosts_names_and_addresses() {
        for host in [
            "win11.corp.example",
            "WIN11-PC",
            "pc01",
            "10.0.0.5",
            "fe80::1",
            "xn--bcher-kva.example",
        ] {
            assert!(form(host, "").is_ok(), "{host}");
        }
        for host in [
            "win 11",
            "pc,01",
            "anna@pc01",
            "pc01:3390",
            "-pc01",
            "pc01-.example",
            "pc..example",
            "bücher.example",
            "10.0.0.256",
            "10.0.0",
            "pc01\n/p:x",
            &"a".repeat(64),
            &format!("{}.example", "a.".repeat(130)),
        ] {
            assert_eq!(form(host, ""), Err(ConnectionError::BadHost), "{host:?}");
        }
        assert_eq!(form("  ", ""), Err(ConnectionError::NoHost));
    }

    #[test]
    fn ipv6_in_brackets_is_stored_without_them_and_given_back_with_them() {
        let c = form(" [fe80::1] ", "3390").unwrap();
        assert_eq!(c.host, "fe80::1");
        assert_eq!(c.address(), "[fe80::1]:3390");
        assert_eq!(form("pc01", "").unwrap().address(), "pc01:3389");
    }

    #[test]
    fn ports() {
        assert_eq!(form("pc01", "").unwrap().port, DEFAULT_PORT);
        assert_eq!(form("pc01", " 3390 ").unwrap().port, 3390);
        for port in ["0", "65536", "-1", "33 89", "x"] {
            assert_eq!(form("pc01", port), Err(ConnectionError::BadPort), "{port}");
        }
    }

    #[test]
    fn user_name_name_and_logon() {
        let ok = |name: &str, user: &str, logon| {
            SavedConnection::from_form(name, "pc01", "", user, logon)
        };
        // DOMAIN\user and user@domain are user names Windows takes.
        assert!(ok("Office PC", r"CORP\anna", LogonSetting::Automatic).is_ok());
        assert!(ok("", "anna@corp.example", LogonSetting::EmptyDomain).is_ok());
        assert_eq!(
            ok("", "anna\nx", LogonSetting::Automatic),
            Err(ConnectionError::BadUserName)
        );
        assert_eq!(
            ok(&"n".repeat(81), "", LogonSetting::Automatic),
            Err(ConnectionError::BadName)
        );
        assert_eq!(
            ok("", "", LogonSetting::CustomDomain(" CORP ".into()))
                .unwrap()
                .logon,
            LogonSetting::CustomDomain("CORP".into())
        );
        assert_eq!(
            ok("", "", LogonSetting::CustomDomain("co rp".into())),
            Err(ConnectionError::BadDomain)
        );
        assert_eq!(
            ok("", "", LogonSetting::EntraSignIn),
            Err(ConnectionError::EntraNotAvailable)
        );
    }

    #[test]
    fn the_title_is_the_name_or_the_host() {
        assert_eq!(form("pc01", "").unwrap().title(), "pc01");
        let named = SavedConnection::from_form("Office", "pc01", "", "", LogonSetting::Automatic);
        assert_eq!(named.unwrap().title(), "Office");
    }

    #[test]
    fn ids_are_random_and_never_a_desktop_key() {
        let (a, b) = (new_id().unwrap(), new_id().unwrap());
        assert_ne!(a, b);
        assert!(a.starts_with("rdp-") && a.len() == 4 + 24, "{a}");
    }

    /// Saved by an earlier run or edited by hand: read back and checked.
    #[test]
    fn a_saved_connection_round_trips_and_is_checked_again() {
        let c = SavedConnection::from_form(
            "Office",
            "pc01",
            "3390",
            "anna",
            LogonSetting::CustomDomain("CORP".into()),
        )
        .unwrap();
        let text = ron::to_string(&c).unwrap();
        assert_eq!(ron::from_str::<SavedConnection>(&text).unwrap(), c);
        let edited: SavedConnection = ron::from_str(r#"(host: "pc01 /p:x", port: 3389)"#).unwrap();
        assert_eq!(edited.check(), Err(ConnectionError::BadHost));
    }

    /// Whatever is typed: an accepted connection has a host FreeRDP can
    /// take as one value, and checks out the same when read back.
    #[test]
    fn random_input_never_gets_an_unsafe_host_through() {
        let samples: [&[u8]; 4] = [b"win11.corp.example", b"10.0.0.5", b"[fe80::1]", b"PC-01"];
        crate::test_util::random_inputs("connection host", &samples, |input| {
            let host = String::from_utf8_lossy(input);
            if let Ok(c) = form(&host, "") {
                assert!(
                    c.host.len() <= MAX_HOST
                        && !c.host.contains([' ', ',', '@', '/', '\\', '[', ']'])
                        && !c.host.chars().any(char::is_control),
                    "{:?}",
                    c.host
                );
                assert_eq!(c.check(), Ok(()));
                assert!(!c.address().contains(['\n', '\r', ' ']));
            }
        });
        let user_samples: [&[u8]; 2] = [br"CORP\anna", b"anna@corp.example"];
        crate::test_util::random_inputs("connection user name", &user_samples, |input| {
            let user = String::from_utf8_lossy(input);
            if let Ok(c) =
                SavedConnection::from_form("", "pc01", "", &user, LogonSetting::Automatic)
            {
                assert!(
                    !c.username.chars().any(char::is_control),
                    "{:?}",
                    c.username
                );
                assert!(c.username.len() <= MAX_USER_NAME);
            }
        });
    }
}
