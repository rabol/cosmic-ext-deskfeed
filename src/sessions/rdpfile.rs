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
//! `camerastoredirect`, `redirectclipboard`, `audiocapturemode` and
//! `audiomode` aren't passed on either, but they are read: they are the
//! administrator's device policy, which the user's settings can't override
//! (`cameras`, `devices`; spec §5 "respect server policy"). A missing one
//! means Microsoft's default (RDP properties reference): no camera, no
//! clipboard, no microphone, sound played on this computer.
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
/// Most refused property names kept for the log.
const MAX_REFUSED: usize = 16;

/// How a kept property's value is checked: by type, with destinations
/// checked separately.
#[derive(Clone, Copy)]
enum Rule {
    /// `i`, within this range (Microsoft's RDP properties reference).
    Int(i64, i64),
    /// `s`: printable text.
    Text,
    /// `s`: a host name or IP address, optionally with `:port`; no scheme,
    /// user or path.
    HostPort,
    /// `s`: a host name only.
    Host,
}

/// Properties copied from the feed's file (lower-case) and their rules.
/// Display behaviour (`smart sizing`, which conflicts with
/// `/dynamic-resolution`; full screen and monitors) is left out: the app's
/// settings decide it (settings.rs). FreeRDP can't switch full screen off
/// from the command line, so a `screen mode id:i:2` here would override
/// "windowed".
///
/// Not kept although AVD sends them: `armpath`, `geo`,
/// `hubdiscoverygeourl` and `wvd endpoint pool`, which FreeRDP 3.32.2
/// reads but never uses; `resourceprovider`, whose only effect
/// (`resourceprovider:s:arm` → `GatewayArmTransport`, `file.c`) the
/// launcher's `/gateway:type:arm` already has; and `workspace id` and
/// `remotedesktopname`, which it doesn't read. With the AVD gateway the
/// session host comes from the broker's answer (`gateway/arm.c`), so
/// `full address` is only checked for its form.
fn rule(key: &str) -> Option<Rule> {
    use Rule::{Host, HostPort, Int, Text};
    Some(match key {
        "full address" | "alternate full address" | "gatewayhostname" => HostPort,
        // FreeRDP's Kerberos KDC address, as given (`KerberosKdcUrl`).
        "kdcproxyname" => Host,
        // Text: `loadbalanceinfo`, `aadtenantid`, and for RemoteApps
        // (ADR 0020) which program, with the arguments the administrator
        // published. Not `remoteapplicationfile` or
        // `remoteapplicationicon`, which name files on this computer, and
        // not `remoteapplicationexpandcmdline`/`…expandworkingdir`: their `0`
        // asks for this computer's environment values, and FreeRDP 3.32.2
        // ignores them anyway (read in `client/common/file.c`, used nowhere).
        "loadbalanceinfo"
        | "aadtenantid"
        | "remoteapplicationprogram"
        | "remoteapplicationname"
        | "remoteapplicationcmdline" => Text,
        "gatewayusagemethod" => Int(0, 4),
        "gatewaycredentialssource" => Int(0, 5),
        "audiomode" => Int(0, 2),
        // Not documented for AVD; small values only.
        "gatewaybrokeringtype" => Int(0, 9),
        "gatewayprofileusagemethod"
        | "remoteapplicationmode"
        | "enablerdsaadauth"
        | "enablecredsspsupport"
        | "promptcredentialonce"
        | "targetisaadjoined"
        | "use redirection server name"
        | "videoplaybackmode"
        | "dynamic resolution"
        | "autoreconnection enabled" => Int(0, 1),
        _ => return None,
    })
}

/// The value as the rule allows it, or `None`.
fn checked(rule: Rule, kind: &str, value: &str) -> Option<Value> {
    let printable = |v: &str| !v.is_empty() && !v.chars().any(char::is_control);
    match (rule, kind) {
        (Rule::Int(min, max), "i") => {
            let v: i64 = value.trim().parse().ok()?;
            (min..=max).contains(&v).then_some(Value::Int(v))
        }
        (Rule::Text, "s") if printable(value) => Some(Value::Str(value.to_owned())),
        (Rule::HostPort, "s") if is_host_port(value) => Some(Value::Str(value.to_owned())),
        (Rule::Host, "s") if is_host(value) => Some(Value::Str(value.to_owned())),
        _ => None,
    }
}

/// A DNS name or IPv4 address: labels of letters, digits, `-` and `_`.
fn is_host(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        })
}

/// `host`, `host:port` or `[IPv6]:port`, port 1–65535.
fn is_host_port(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix('[') {
        let Some((ip, port)) = rest.split_once(']') else {
            return false;
        };
        return ip.parse::<std::net::Ipv6Addr>().is_ok()
            && (port.is_empty() || port.strip_prefix(':').is_some_and(is_port));
    }
    match s.rsplit_once(':') {
        Some((host, port)) => is_host(host) && is_port(port),
        None => is_host(s),
    }
}

fn is_port(p: &str) -> bool {
    p.parse::<u16>().is_ok_and(|n| n > 0) && p.chars().all(|c| c.is_ascii_digit())
}

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

/// What the feed's `camerastoredirect` allows. Its value is `*` for every
/// camera, or a `;`-separated list of Windows device paths, where a leading
/// `-` excludes one (Microsoft's RDP properties reference).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraPolicy {
    /// Not set or empty: no cameras.
    NotAllowed,
    /// `*` alone: every camera.
    All,
    /// A list of specific cameras, or `*` with exclusions. FreeRDP 3.32.2
    /// can't follow it: it shares every camera and ignores device names
    /// (`channels/rdpecam/client`), so no camera is shared.
    Specific,
}

impl CameraPolicy {
    fn parse(value: &str) -> Self {
        let entries: Vec<&str> = value
            .split([';', ','])
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .collect();
        match entries.as_slice() {
            [] => Self::NotAllowed,
            ["*"] => Self::All,
            _ => Self::Specific,
        }
    }
}

/// What the administrator allows for this computer's devices, besides
/// cameras.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevicePolicy {
    /// `redirectclipboard:i:1`.
    pub clipboard: bool,
    /// `audiocapturemode:i:1`.
    pub microphone: bool,
    /// `audiomode:i:0` (or missing): the desktop's sound plays here. `1`
    /// leaves it on the host, `2` plays none.
    pub sound: bool,
}

impl DevicePolicy {
    /// Microsoft's defaults, for properties the file doesn't set.
    const DEFAULT: Self = Self {
        clipboard: false,
        microphone: false,
        sound: true,
    };

    fn read(&mut self, key: &str, kind: &str, value: &str) {
        if kind != "i" {
            return;
        }
        let Ok(v) = value.trim().parse::<i64>() else {
            return;
        };
        match key {
            "redirectclipboard" => self.clipboard = v == 1,
            "audiocapturemode" => self.microphone = v == 1,
            "audiomode" => self.sound = v == 0,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionFile {
    props: BTreeMap<String, Value>,
    /// Properties that were in the feed's file but not passed on.
    pub dropped: usize,
    /// The administrator's camera policy (`camerastoredirect`).
    pub cameras: CameraPolicy,
    /// The administrator's clipboard, microphone and sound policy.
    pub devices: DevicePolicy,
    /// Names of properties Deskfeed keeps that were dropped for their form
    /// (bounded; for the log).
    pub refused: Vec<String>,
}

impl ConnectionFile {
    pub fn parse(bytes: &[u8]) -> Result<Self, RdpError> {
        let text = decode(bytes)?;
        let mut props = BTreeMap::new();
        let mut dropped = 0;
        let mut cameras = CameraPolicy::NotAllowed;
        let mut devices = DevicePolicy::DEFAULT;
        let mut refused = Vec::new();
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
            if key == "camerastoredirect" && kind == "s" && value.len() <= MAX_VALUE_LEN {
                cameras = CameraPolicy::parse(value);
            }
            devices.read(&key, kind, value);
            let Some(rule) = rule(&key) else {
                dropped += 1;
                continue;
            };
            if let Some(value) = (value.len() <= MAX_VALUE_LEN)
                .then(|| checked(rule, kind, value))
                .flatten()
            {
                props.insert(key, value);
            } else {
                // A property Deskfeed keeps, in a form it doesn't accept:
                // its name goes to the log (`refused`), never its value.
                dropped += 1;
                if refused.len() < MAX_REFUSED && !refused.contains(&key) {
                    refused.push(key);
                }
            }
        }
        if !matches!(props.get("full address"), Some(Value::Str(s)) if !s.is_empty()) {
            return Err(RdpError::MissingAddress);
        }
        if !matches!(props.get("gatewayhostname"), Some(Value::Str(g)) if is_avd_gateway(g)) {
            return Err(RdpError::GatewayNotAllowed);
        }
        Ok(Self {
            props,
            dropped,
            cameras,
            devices,
            refused,
        })
    }

    /// A file Deskfeed makes for a Windows machine by host name (ADR 0029):
    /// only its address, checked like a feed file's `full address`, so no
    /// other property can get in. With no administrator's file, the user's
    /// own settings decide the devices.
    // Used from the screens in step 4 of ADR 0029; until then by the tests
    // only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn direct(address: &str) -> Option<Self> {
        is_host_port(address).then(|| Self::with_address(address))
    }

    /// Tests only: a file for the local test server
    /// (`research/rdp-test-server`), whose address may be a socket path,
    /// which `direct` rightly refuses.
    #[cfg(test)]
    pub fn local_test(address: &str) -> Self {
        Self::with_address(address)
    }

    fn with_address(address: &str) -> Self {
        let mut props = BTreeMap::new();
        props.insert("full address".to_owned(), Value::Str(address.to_owned()));
        Self {
            props,
            dropped: 0,
            cameras: CameraPolicy::All,
            devices: DevicePolicy {
                clipboard: true,
                microphone: true,
                sound: true,
            },
            refused: Vec::new(),
        }
    }

    /// The file opens a RemoteApp, not a desktop (`remoteapplicationmode:i:1`).
    /// FreeRDP then shows the app's own windows (ADR 0020).
    pub fn remote_app(&self) -> bool {
        matches!(self.props.get("remoteapplicationmode"), Some(Value::Int(v)) if *v != 0)
    }

    /// Tests only: `local_test` opening a RemoteApp (`||alias` or a path).
    #[cfg(test)]
    pub fn local_test_app(address: &str, program: &str) -> Self {
        let mut file = Self::local_test(address);
        file.props
            .insert("remoteapplicationmode".to_owned(), Value::Int(1));
        file.props.insert(
            "remoteapplicationprogram".to_owned(),
            Value::Str(program.to_owned()),
        );
        file
    }

    /// Serialized for FreeRDP with every device off (tests).
    #[cfg(test)]
    pub fn to_freerdp_text(&self) -> String {
        self.to_freerdp_text_with(false)
    }

    /// Serialized for FreeRDP: kept properties, then device defaults, with
    /// the clipboard on when the user's effective setting has it (and the
    /// policy allowed it, `Effective::limit_to`).
    ///
    /// The clipboard has to be switched on here. FreeRDP 3.32.2 reads this
    /// file before the arguments, and `/clipboard:direction-to:…` only sets
    /// directions: it doesn't turn redirection back on after
    /// `redirectclipboard:i:0` (`client/common/cmdline.c`,
    /// `parse_clipboard_options`; only the plain `+clipboard` and
    /// `use-selection:` do). Without this, the clipboard stays off whatever
    /// the setting says.
    pub fn to_freerdp_text_with(&self, clipboard: bool) -> String {
        let mut out = String::new();
        for (key, value) in &self.props {
            // Writing to a String can't fail.
            let _ = match value {
                Value::Int(v) => write!(out, "{key}:i:{v}\r\n"),
                Value::Str(s) => write!(out, "{key}:s:{s}\r\n"),
            };
        }
        for line in DEVICE_DEFAULTS {
            let line = if clipboard && *line == "redirectclipboard:i:0" {
                "redirectclipboard:i:1"
            } else {
                line
            };
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
    fn the_clipboard_is_switched_on_in_the_file_when_wanted() {
        let f = ConnectionFile::parse(SAMPLE.as_bytes()).unwrap();
        let on = f.to_freerdp_text_with(true);
        assert_eq!(on.matches("redirectclipboard:").count(), 1, "{on}");
        assert!(on.contains("redirectclipboard:i:1\r\n"));
        // Only the clipboard: every other device stays off.
        assert!(on.contains("redirectprinters:i:0") && on.contains("audiocapturemode:i:0"));
        let off = f.to_freerdp_text_with(false);
        assert!(off.contains("redirectclipboard:i:0") && !off.contains("redirectclipboard:i:1"));
        assert_eq!(off, f.to_freerdp_text());
    }

    #[test]
    fn keeps_connection_properties_and_drops_devices() {
        let f = ConnectionFile::parse(SAMPLE.as_bytes()).unwrap();
        let text = f.to_freerdp_text();
        for kept in [
            "full address:s:rdgateway.example",
            "gatewayhostname:s:rdgateway.wvd.microsoft.com:443",
            "audiomode:i:0",
        ] {
            assert!(text.contains(kept), "{kept}");
        }
        // Read by FreeRDP but never used: not passed on.
        assert!(!text.contains("armpath"));
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
        assert_eq!(f.dropped, 10);
        assert_eq!(f.cameras, CameraPolicy::All);
    }

    #[test]
    fn reads_camera_policy() {
        let policy = |line: &str| {
            ConnectionFile::parse(format!("full address:s:h\r\n{GW}{line}").as_bytes())
                .unwrap()
                .cameras
        };
        assert_eq!(policy(""), CameraPolicy::NotAllowed);
        assert_eq!(policy("camerastoredirect:s:\r\n"), CameraPolicy::NotAllowed);
        assert_eq!(policy("CameraStoreDirect:s: * \r\n"), CameraPolicy::All);
        assert_eq!(
            policy("camerastoredirect:s:\\\\?\\usb#vid_0bda&pid_58b0\r\n"),
            CameraPolicy::Specific
        );
        assert_eq!(
            policy("camerastoredirect:s:*;-\\\\?\\usb#vid_0bda\r\n"),
            CameraPolicy::Specific
        );
        // Wrong type: not a camera policy.
        assert_eq!(
            policy("camerastoredirect:i:1\r\n"),
            CameraPolicy::NotAllowed
        );
    }

    #[test]
    fn each_property_has_its_form() {
        let kept = |line: &str| {
            let f = ConnectionFile::parse(format!("full address:s:h\r\n{GW}{line}\r\n").as_bytes())
                .unwrap();
            (f.to_freerdp_text().contains(line), f.refused)
        };
        for good in [
            "alternate full address:s:host-1.example.com:3389",
            "alternate full address:s:10.0.0.4",
            "alternate full address:s:[fe80::1]:3389",
            "kdcproxyname:s:kdc.example.com",
            "audiomode:i:2",
            "gatewayusagemethod:i:4",
            "enablerdsaadauth:i:1",
            "loadbalanceinfo:s:tsv://MS Terminal Services Plugin.1.Pool",
            "remoteapplicationcmdline:s:/x \"y z\"",
        ] {
            assert_eq!(kept(good), (true, vec![]), "{good}");
        }
        for (bad, key) in [
            (
                "alternate full address:s:https://evil.example/x",
                "alternate full address",
            ),
            (
                "alternate full address:s:user@host",
                "alternate full address",
            ),
            ("alternate full address:s:host:0", "alternate full address"),
            (
                "alternate full address:s:host:99999",
                "alternate full address",
            ),
            (
                "alternate full address:s:host/path",
                "alternate full address",
            ),
            (
                "kdcproxyname:s:https://kdc.example.com/KdcProxy",
                "kdcproxyname",
            ),
            ("kdcproxyname:s:kdc.example.com:443", "kdcproxyname"),
            ("audiomode:i:7", "audiomode"),
            ("enablerdsaadauth:i:-1", "enablerdsaadauth"),
            ("enablerdsaadauth:s:1", "enablerdsaadauth"),
            ("loadbalanceinfo:i:1", "loadbalanceinfo"),
        ] {
            assert_eq!(kept(bad), (false, vec![key.to_owned()]), "{bad}");
        }
        // A full address with a scheme makes the file unusable.
        assert_eq!(
            ConnectionFile::parse(format!("full address:s:https://h/\r\n{GW}").as_bytes()),
            Err(RdpError::MissingAddress)
        );
    }

    #[test]
    fn reads_device_policy_with_microsofts_defaults() {
        let policy = |lines: &str| {
            ConnectionFile::parse(format!("full address:s:h\r\n{GW}{lines}").as_bytes())
                .unwrap()
                .devices
        };
        assert_eq!(policy(""), DevicePolicy::DEFAULT);
        assert_eq!(
            policy("redirectclipboard:i:1\r\naudiocapturemode:i:1\r\naudiomode:i:0\r\n"),
            DevicePolicy {
                clipboard: true,
                microphone: true,
                sound: true
            }
        );
        let off = policy("RedirectClipboard:i:0\r\naudiocapturemode:i:0\r\naudiomode:i:2\r\n");
        assert!(!off.clipboard && !off.microphone && !off.sound);
        assert!(!policy("audiomode:i:1\r\n").sound);
        // Wrong type or value: the default stays.
        assert!(!policy("redirectclipboard:s:1\r\n").clipboard);
        assert!(!policy("redirectclipboard:i:x\r\n").clipboard);
        // Never passed on as such: the device defaults follow.
        let text = ConnectionFile::parse(
            format!("full address:s:h\r\n{GW}redirectclipboard:i:1\r\n").as_bytes(),
        )
        .unwrap()
        .to_freerdp_text();
        assert!(text.contains("redirectclipboard:i:0") && !text.contains("redirectclipboard:i:1"));
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

    #[test]
    fn keeps_a_remote_apps_program_but_not_local_files() {
        let f = ConnectionFile::parse(
            format!(
                "full address:s:h\r\n{GW}remoteapplicationmode:i:1\r\n\
                 remoteapplicationprogram:s:||notepad\r\n\
                 remoteapplicationname:s:Notepad\r\n\
                 remoteapplicationcmdline:s:/A \"%USERPROFILE%\"\r\n\
                 remoteapplicationexpandcmdline:i:1\r\n\
                 remoteapplicationfile:s:/home/anna/secret.txt\r\n\
                 remoteapplicationicon:s:/home/anna/icon.ico\r\n"
            )
            .as_bytes(),
        )
        .unwrap();
        assert!(f.remote_app());
        let text = f.to_freerdp_text();
        for kept in [
            "remoteapplicationmode:i:1",
            "remoteapplicationprogram:s:||notepad",
            "remoteapplicationname:s:Notepad",
            "remoteapplicationcmdline:s:/A \"%USERPROFILE%\"",
        ] {
            assert!(text.contains(kept), "{kept}");
        }
        assert!(!text.contains("remoteapplicationfile"));
        assert!(!text.contains("remoteapplicationicon"));
        assert!(!text.contains("remoteapplicationexpandcmdline"));
        assert_eq!(f.dropped, 3);
    }

    #[test]
    fn a_desktop_is_not_a_remote_app() {
        let file = |extra: &str| {
            ConnectionFile::parse(format!("full address:s:h\r\n{GW}{extra}").as_bytes()).unwrap()
        };
        assert!(!file("").remote_app());
        assert!(!file("remoteapplicationmode:i:0\r\n").remote_app());
        assert!(!file("remoteapplicationmode:s:1\r\n").remote_app());
    }

    /// The owner's saved connection file, if present: prints only counts.
    #[test]
    #[ignore = "needs AVD_RDP_FILE"]
    fn real_connection_file() {
        let bytes = std::fs::read(std::env::var("AVD_RDP_FILE").unwrap()).unwrap();
        let f = ConnectionFile::parse(&bytes).unwrap();
        println!("kept {} properties, dropped {}", f.props.len(), f.dropped);
    }

    #[test]
    fn random_connection_files_never_break_the_parser() {
        let app = format!(
            "full address:s:h\r\n{GW}remoteapplicationmode:i:1\r\n\
             remoteapplicationprogram:s:||notepad\r\nremoteapplicationcmdline:s:/x\r\n\
             redirectclipboard:i:1\r\naudiocapturemode:i:1\r\n"
        );
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(SAMPLE.encode_utf16().flat_map(u16::to_le_bytes));
        let defaults: Vec<&str> = DEVICE_DEFAULTS
            .iter()
            .filter_map(|l| l.split(':').next())
            .collect();
        crate::test_util::random_inputs(
            "rdpfile",
            &[SAMPLE.as_bytes(), app.as_bytes(), &utf16],
            |input| {
                let Ok(f) = ConnectionFile::parse(input) else {
                    return;
                };
                // What reaches FreeRDP: only reviewed properties, one per
                // line, no control characters, and an AVD gateway.
                let text = f.to_freerdp_text();
                for line in text.split_terminator("\r\n") {
                    assert!(!line.chars().any(char::is_control), "{line:?}");
                    let key = line.split(':').next().unwrap_or_default();
                    assert!(rule(key).is_some() || defaults.contains(&key), "{line:?}");
                }
                assert!(matches!(
                    f.props.get("gatewayhostname"),
                    Some(Value::Str(g)) if is_avd_gateway(g)
                ));
            },
        );
    }

    /// A file Deskfeed makes for a host by name (ADR 0029): only a checked
    /// address goes in, so nothing can add a property line.
    #[test]
    fn a_direct_file_holds_only_a_checked_address() {
        for ok in [
            "pc01:3389",
            "win11.corp.example:3390",
            "10.0.0.5:3389",
            "[fe80::1]:3389",
        ] {
            let file = ConnectionFile::direct(ok).unwrap();
            let text = file.to_freerdp_text();
            assert!(
                text.starts_with(&format!("full address:s:{ok}\r\n")),
                "{text}"
            );
            assert_eq!(
                text.lines()
                    .filter(|l| l.starts_with("full address"))
                    .count(),
                1
            );
            assert!(!file.remote_app());
        }
        for bad in [
            "pc01:3389\r\nalternate shell:s:cmd.exe",
            "pc 01:3389",
            "anna@pc01:3389",
            "pc01:0",
            "",
        ] {
            assert!(ConnectionFile::direct(bad).is_none(), "{bad:?}");
        }
    }
}
