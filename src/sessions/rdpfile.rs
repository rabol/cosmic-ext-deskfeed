// SPDX-License-Identifier: MIT

//! Connection (`.rdp`) files from the feed, parsed into a reviewed property
//! set (spec §7).
//!
//! Only properties needed to reach the resource are kept. Everything that
//! would share local devices (drives, USB, cameras, printers, smart cards,
//! serial ports, microphone, clipboard) is dropped and explicitly turned
//! off; the user's own settings decide those later. Commands such as
//! `alternate shell` are never passed on.
//!
//! The file decides where the access token goes (`gatewayhostname`), so a
//! file naming any other gateway than an AVD service host is refused. Its
//! certificate policy (`authentication level`) is not kept: in FreeRDP
//! 3.32.2 level 0 skips the session host's certificate check (`tls.c`), and
//! without the property FreeRDP uses its default, 2, which checks.

use crate::service::endpoint::is_avd_gateway;

use std::collections::BTreeMap;
use std::fmt::Write as _;

const MAX_LINES: usize = 1_000;
const MAX_VALUE_LEN: usize = 4_096;

/// Properties copied from the feed's file, lower-case. Display behaviour
/// (`smart sizing`, which conflicts with `/dynamic-resolution`; full screen
/// and monitors) is left out: the app's settings decide it (settings.rs).
/// FreeRDP can't switch full screen off from the command line, so a
/// `screen mode id:i:2` here would override "windowed".
const ALLOWED: &[&str] = &[
    "full address",
    "alternate full address",
    "gatewayhostname",
    "gatewayusagemethod",
    "gatewayprofileusagemethod",
    "gatewaycredentialssource",
    "gatewaybrokeringtype",
    "armpath",
    "loadbalanceinfo",
    "aadtenantid",
    "geo",
    "hubdiscoverygeourl",
    "wvd endpoint pool",
    "workspace id",
    "resourceprovider",
    "remoteapplicationmode",
    "remoteapplicationprogram",
    "remoteapplicationname",
    "remotedesktopname",
    "enablerdsaadauth",
    "enablecredsspsupport",
    "promptcredentialonce",
    "targetisaadjoined",
    "kdcproxyname",
    "use redirection server name",
    "audiomode",
    "videoplaybackmode",
    "dynamic resolution",
    "autoreconnection enabled",
];

/// Written after the kept properties so local devices stay private.
const DEVICE_DEFAULTS: &[&str] = &[
    "redirectclipboard:i:0",
    "redirectprinters:i:0",
    "redirectsmartcards:i:0",
    "redirectcomports:i:0",
    "redirectlocation:i:0",
    "redirectwebauthn:i:0",
    "audiocapturemode:i:0",
    "encode redirected video capture:i:0",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Str(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdpError {
    NotText,
    TooLarge,
    /// No `full address`, so there is nothing to connect to.
    MissingAddress,
    /// No gateway, or one that isn't an AVD service host.
    GatewayNotAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionFile {
    props: BTreeMap<String, Value>,
    /// Properties that were in the feed's file but not passed on.
    pub dropped: usize,
}

impl ConnectionFile {
    pub fn parse(bytes: &[u8]) -> Result<Self, RdpError> {
        let text = decode(bytes)?;
        let mut props = BTreeMap::new();
        let mut dropped = 0;
        for (n, line) in text.lines().enumerate() {
            if n >= MAX_LINES {
                return Err(RdpError::TooLarge);
            }
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let Some((key, kind, value)) = split_line(line) else {
                dropped += 1;
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            if !ALLOWED.contains(&key.as_str()) || value.len() > MAX_VALUE_LEN {
                dropped += 1;
                continue;
            }
            let value = match kind {
                "i" => {
                    let Ok(v) = value.trim().parse() else {
                        dropped += 1;
                        continue;
                    };
                    Value::Int(v)
                }
                "s" if value.chars().all(|c| !c.is_control()) => Value::Str(value.to_owned()),
                _ => {
                    dropped += 1;
                    continue;
                }
            };
            props.insert(key, value);
        }
        if !matches!(props.get("full address"), Some(Value::Str(s)) if !s.is_empty()) {
            return Err(RdpError::MissingAddress);
        }
        if !matches!(props.get("gatewayhostname"), Some(Value::Str(g)) if is_avd_gateway(g)) {
            return Err(RdpError::GatewayNotAllowed);
        }
        Ok(Self { props, dropped })
    }

    /// Serialized for FreeRDP: kept properties, then device defaults.
    pub fn to_freerdp_text(&self) -> String {
        let mut out = String::new();
        for (key, value) in &self.props {
            // Writing to a String can't fail.
            let _ = match value {
                Value::Int(v) => write!(out, "{key}:i:{v}\r\n"),
                Value::Str(s) => write!(out, "{key}:s:{s}\r\n"),
            };
        }
        for line in DEVICE_DEFAULTS {
            out.push_str(line);
            out.push_str("\r\n");
        }
        out
    }
}

/// `.rdp` files are UTF-16LE with a byte-order mark, or UTF-8.
fn decode(bytes: &[u8]) -> Result<String, RdpError> {
    if bytes.len() > 256 * 1024 {
        return Err(RdpError::TooLarge);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        if rest.len() % 2 != 0 {
            return Err(RdpError::NotText);
        }
        let (pairs, _) = rest.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
        return String::from_utf16(&units).map_err(|_| RdpError::NotText);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(bytes.to_vec()).map_err(|_| RdpError::NotText)
}

/// `key:type:value`, where the value may itself contain colons.
fn split_line(line: &str) -> Option<(&str, &str, &str)> {
    let (key, rest) = line.split_once(':')?;
    let (kind, value) = rest.split_once(':')?;
    Some((key, kind, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GW: &str = "gatewayhostname:s:rdgateway.wvd.microsoft.com:443\r\n";

    const SAMPLE: &str = "full address:s:rdgateway.example\r\n\
        alternate full address:s:rdgateway.example\r\n\
        gatewayhostname:s:rdgateway.wvd.microsoft.com:443\r\n\
        armpath:s:abc/def\r\n\
        loadbalanceinfo:s:lb\r\n\
        enablecredsspsupport:i:1\r\n\
        promptcredentialonce:i:1\r\n\
        drivestoredirect:s:*\r\n\
        devicestoredirect:s:*\r\n\
        usbdevicestoredirect:s:*\r\n\
        camerastoredirect:s:*\r\n\
        redirectclipboard:i:1\r\n\
        redirectprinters:i:1\r\n\
        alternate shell:s:cmd.exe /c evil\r\n\
        signature:s:AAAA\r\n\
        diagnosticserviceurl:s:https://diag.example\r\n\
        audiomode:i:0\r\n";

    #[test]
    fn keeps_connection_properties_and_drops_devices() {
        let f = ConnectionFile::parse(SAMPLE.as_bytes()).unwrap();
        let text = f.to_freerdp_text();
        for kept in [
            "full address:s:rdgateway.example",
            "gatewayhostname:s:rdgateway.wvd.microsoft.com:443",
            "armpath:s:abc/def",
            "audiomode:i:0",
        ] {
            assert!(text.contains(kept), "{kept}");
        }
        for gone in [
            "drivestoredirect",
            "devicestoredirect",
            "usbdevicestoredirect",
            "camerastoredirect",
            "alternate shell",
            "signature",
            "diagnosticserviceurl",
        ] {
            assert!(!text.contains(gone), "{gone}");
        }
        assert!(text.contains("redirectclipboard:i:0"));
        assert!(text.contains("redirectprinters:i:0"));
        assert!(!text.contains("redirectclipboard:i:1"));
        assert_eq!(f.dropped, 9);
    }

    #[test]
    fn reads_utf16_with_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in format!("full address:s:h\r\n{GW}enablerdsaadauth:i:1\r\n").encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let f = ConnectionFile::parse(&bytes).unwrap();
        assert!(f.to_freerdp_text().contains("enablerdsaadauth:i:1"));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(
            ConnectionFile::parse(b"\xff\xfe\x00").unwrap_err(),
            RdpError::NotText
        );
        assert_eq!(
            ConnectionFile::parse(&[0xC3, 0x28]).unwrap_err(),
            RdpError::NotText
        );
        assert_eq!(
            ConnectionFile::parse(b"audiomode:i:0\r\n").unwrap_err(),
            RdpError::MissingAddress
        );
        let many = "x:i:1\n".repeat(MAX_LINES + 1);
        assert_eq!(
            ConnectionFile::parse(many.as_bytes()).unwrap_err(),
            RdpError::TooLarge
        );
    }

    #[test]
    fn line_injection_is_impossible() {
        // A value can't smuggle a second property: lines are split first,
        // and control characters inside values are refused.
        let f = ConnectionFile::parse(
            format!("full address:s:h\r\n{GW}remotedesktopname:s:a\x0bdrivestoredirect:s:*\r\n")
                .as_bytes(),
        )
        .unwrap();
        let text = f.to_freerdp_text();
        assert!(!text.contains("drivestoredirect"));
        assert!(!text.contains("remotedesktopname"));
    }

    #[test]
    fn bad_integer_is_dropped() {
        let f = ConnectionFile::parse(
            format!("full address:s:h\r\n{GW}enablerdsaadauth:i:yes\r\n").as_bytes(),
        )
        .unwrap();
        assert!(!f.to_freerdp_text().contains("enablerdsaadauth"));
        assert_eq!(f.dropped, 1);
    }

    #[test]
    fn token_only_goes_to_an_avd_gateway() {
        for gateway in [
            "",
            "gatewayhostname:s:attacker.example:443\r\n",
            "gatewayhostname:s:127.0.0.1:1\r\n",
            "gatewayhostname:i:1\r\n",
        ] {
            let text = format!("full address:s:h\r\n{gateway}");
            assert_eq!(
                ConnectionFile::parse(text.as_bytes()).unwrap_err(),
                RdpError::GatewayNotAllowed,
                "{gateway}"
            );
        }
    }

    #[test]
    fn certificate_policy_is_not_taken_from_the_feed() {
        let f = ConnectionFile::parse(
            format!("full address:s:h\r\n{GW}authentication level:i:0\r\n").as_bytes(),
        )
        .unwrap();
        assert!(!f.to_freerdp_text().contains("authentication level"));
        assert_eq!(f.dropped, 1);
    }

    /// The owner's saved connection file, if present: prints only counts.
    #[test]
    #[ignore = "needs AVD_RDP_FILE"]
    fn real_connection_file() {
        let bytes = std::fs::read(std::env::var("AVD_RDP_FILE").unwrap()).unwrap();
        let f = ConnectionFile::parse(&bytes).unwrap();
        println!("kept {} properties, dropped {}", f.props.len(), f.dropped);
    }
}
