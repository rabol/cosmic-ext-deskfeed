// SPDX-License-Identifier: MIT

//! Reading FreeRDP's output: connection steps, certificates, refusals, and
//! how a session ended.

// The launcher's parts share its imports and items (mod.rs).
#[allow(clippy::wildcard_imports)]
use super::*;

/// The session is active: FreeRDP's last connection step
/// (`libfreerdp/core/connection.c`), logged by `STATE_LOGGER`.
pub(super) const ACTIVE_MARKER: &str = "--> CONNECTION_STATE_ACTIVE";

/// Entering and leaving the sign-in steps: Entra (`AAD`) or the Windows
/// password (`NLA`), seen in real AVD logs (docs/compatibility.md).
pub(super) const AUTH_STATES: &[&str] = &["CONNECTION_STATE_AAD", "CONNECTION_STATE_NLA"];

/// FreeRDP lost a connected session (`client/common/client.c`,
/// `client_auto_reconnect_ex`): logged before it checks whether it may
/// reconnect, so also with its reconnect off.
pub(super) const LOST_MARKERS: &[&str] = &[
    "client_auto_reconnect_ex]: Network disconnect!",
    "client_auto_reconnect_ex]: Disconnected by server hitting a bug",
];

/// FreeRDP's log line when Windows refuses to start a RemoteApp.
pub(super) const APP_REFUSED_MARKER: &str = "RAIL exec error: execResult=";

/// The server's reason when another connection took over the session
/// (`libfreerdp/core/errinfo.c`, logged at INFO).
pub(super) const REPLACED_MARKER: &str = "ERRINFO_DISCONNECTED_BY_OTHER_CONNECTION";

/// FreeRDP's log line with the proxy's answer (`http_proxy_connect`).
pub(super) const PROXY_ANSWER_MARKER: &str = "[http_proxy_connect]: HTTP Proxy: ";

/// The session host didn't finish the logon within `/timeout`
/// (`connection.c`, `rdp_client_wait_for_activation`). FreeRDP then exits
/// with its generic 131 (`XF_EXIT_CONN_FAILED`). Seen on the owner's host
/// right after the previous session closed (docs/compatibility.md).
pub(super) const ACTIVATION_TIMEOUT_MARKER: &str = "ERRCONNECT_ACTIVATION_TIMEOUT";

pub(super) const MAX_LOG_LINES: usize = 60;

pub(super) const STATE_LOGGER: &str = "com.freerdp.core.rdp";

/// Why a session ended, from FreeRDP's exit code and log. The X11 and SDL
/// clients share these codes (FreeRDP 3.32.2 `client/X11/xfreerdp.h`,
/// `client/SDL/SDL3/sdl_utils.hpp`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// Normal end: disconnect, log off, or the user closed the window.
    Closed,
    /// Windows disconnected this connection (exit code 1,
    /// `ERRINFO_RPC_INITIATED_DISCONNECT`): an administrator, or, on the
    /// Windows host tested in ADR 0020, another connection taking over the
    /// session. A normal end otherwise.
    DisconnectedByServer,
    LogonFailed,
    AccountLockedOut,
    PasswordExpired,
    AccountDisabled,
    AuthenticationFailed,
    DnsFailed,
    TlsFailed,
    Cancelled,
    /// A connected session lost its connection (network, sleep, server).
    ConnectionLost,
    /// Another connection took over the Windows session (exit code 5,
    /// `ERRINFO_DISCONNECTED_BY_OTHER_CONNECTION`): the desktop was opened
    /// elsewhere, or another RemoteApp from the same host pool connected
    /// (ADR 0020). Never reconnected: both would take turns.
    Replaced,
    /// Windows wouldn't start the RemoteApp (ADR 0020).
    AppRefused(AppRefusal),
    /// The proxy wanted a user name and password (HTTP 407). FreeRDP 3.32.2
    /// can't give it one: it drops the last character of `user:password`
    /// before encoding it (`libfreerdp/core/proxy.c`, `http_proxy_connect`,
    /// `size - 1` on `winpr_asprintf`'s length, which has no NUL; still so
    /// on FreeRDP's master branch, 2026-10-19).
    ProxyAuthRefused,
    /// The session host didn't finish the logon in time
    /// (`ACTIVATION_TIMEOUT_MARKER`): the server's side, not the sign-in.
    HostTimedOut,
    /// FreeRDP couldn't verify the session host's certificate (ADR 0016).
    CertificateNotTrusted(CertificateInfo),
    Failed {
        code: i32,
    },
    Crashed,
}

/// Why Windows wouldn't start a RemoteApp: FreeRDP's
/// `RAIL exec error: execResult=RAIL_EXEC_E_…` line (`client/X11/xf_rail.c`,
/// MS-RDPERP 2.2.2.8.1). FreeRDP then cancels the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRefusal {
    /// Not published, or not allowed on the host (`NOT_IN_ALLOWLIST`).
    NotAllowed,
    /// The program isn't on the host (`FILE_NOT_FOUND`).
    NotFound,
    /// The Windows session is locked (`SESSION_LOCKED`).
    SessionLocked,
    /// Anything else (`HOOK_NOT_LOADED`, `DECODE_FAILED`, `FAIL`, unknown).
    Other,
}

impl AppRefusal {
    pub(super) fn from_log(line: &str) -> Option<Self> {
        let (_, rest) = line.split_once(APP_REFUSED_MARKER)?;
        let name = rest.split([' ', '[']).next().unwrap_or_default();
        Some(match name {
            "RAIL_EXEC_E_NOT_IN_ALLOWLIST" => Self::NotAllowed,
            "RAIL_EXEC_E_FILE_NOT_FOUND" => Self::NotFound,
            "RAIL_EXEC_E_SESSION_LOCKED" => Self::SessionLocked,
            _ => Self::Other,
        })
    }
}

/// A session host certificate as FreeRDP described it. All text comes
/// from the network: bounded, printable only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateInfo {
    /// The host the certificate was for (as FreeRDP connected).
    pub host: String,
    pub common_name: String,
    pub subject: String,
    pub issuer: String,
    pub valid_from: String,
    pub valid_to: String,
    /// sha256, lower case, `aa:bb:…` (32 pairs).
    pub fingerprint: String,
}

/// A field from FreeRDP's output: one line, printable, at most 256 chars.
pub(super) fn clean_field(s: &str) -> String {
    s.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect()
}

/// A sha256 fingerprint as FreeRDP prints and parses it.
pub fn is_sha256_fingerprint(s: &str) -> bool {
    s.len() == 95
        && s.split(':').count() == 32
        && s.split(':')
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// FreeRDP gives up on an unverified certificate (`libfreerdp/crypto/tls.c`).
pub(super) const CERT_REFUSED_MARKER: &str = "certificate not trusted, aborting";

/// FreeRDP's log line with the presented certificate's fingerprint
/// (`tls_print_new_certificate_warn`, `tls_print_certificate_error`).
pub(super) const PRESENTED_MARKER: &str =
    "The fingerprint for the host key sent by the remote host is ";

/// How a session ended. `connection_lost`: FreeRDP said a connected
/// session lost its connection (exit code 147, or 0 when it knew no error).
pub fn classify_exit(
    code: Option<i32>,
    log_says_logon_failure: bool,
    connection_lost: bool,
) -> SessionEnd {
    match code {
        None => SessionEnd::Crashed,
        // `XF_EXIT_CONN_REPLACED`; the server's reason, so it wins over a
        // lost connection.
        Some(5) => SessionEnd::Replaced,
        _ if log_says_logon_failure => SessionEnd::LogonFailed,
        _ if connection_lost => SessionEnd::ConnectionLost,
        Some(1) => SessionEnd::DisconnectedByServer,
        Some(0 | 2 | 3 | 11 | 12) => SessionEnd::Closed,
        Some(134) => SessionEnd::LogonFailed,
        Some(135) => SessionEnd::AccountLockedOut,
        Some(148 | 149 | 152) => SessionEnd::PasswordExpired,
        Some(151) => SessionEnd::AccountDisabled,
        Some(132) => SessionEnd::AuthenticationFailed,
        Some(139 | 140) => SessionEnd::DnsFailed,
        Some(143) => SessionEnd::TlsFailed,
        Some(145) => SessionEnd::Cancelled,
        Some(code) => SessionEnd::Failed { code },
    }
}

/// A FreeRDP connection step (`STATE_A --> STATE_B`); no secrets.
pub(super) fn is_state_change(line: &str) -> bool {
    line.contains(&format!("[DEBUG][{STATE_LOGGER}]")) && line.contains(" --> ")
}

// Independent facts read from the log; a bitflag type would only hide them.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
pub(super) struct Log {
    pub(super) lines: Vec<String>,
    pub(super) connected: bool,
    pub(super) logon_failure: bool,
    /// The connection was lost (`LOST_MARKERS`).
    pub(super) lost: bool,
    /// Another connection took over the session (`REPLACED_MARKER`).
    pub(super) replaced: bool,
    /// Windows refused to start the RemoteApp (`APP_REFUSED_MARKER`).
    pub(super) app_refused: Option<AppRefusal>,
    /// The proxy asked for a password (`PROXY_ANSWER_MARKER`, 407).
    pub(super) proxy_auth: bool,
    /// The host didn't finish the logon in time (`ACTIVATION_TIMEOUT_MARKER`).
    pub(super) activation_timeout: bool,
    /// The certificate FreeRDP is describing (`Certificate details for`).
    pub(super) certificate: Option<CertificateInfo>,
    /// A complete description (it has a well-formed thumbprint).
    pub(super) described: Option<CertificateInfo>,
    /// The fingerprint from FreeRDP's own log line about the new host key.
    pub(super) presented: Option<String>,
    /// FreeRDP refused the certificate.
    pub(super) refused: bool,
    pub(super) stop_sent: bool,
    /// Set once when the refusal and the description are both in: the
    /// reader ends FreeRDP, which otherwise may wait after it.
    pub(super) stop_now: bool,
}

impl Log {
    /// FreeRDP's description of an unverified certificate
    /// (`client/common/client.c`, `client_cli_verify_certificate_ex`):
    /// `Certificate details for <host> (<name>):`, then tab-indented
    /// `Common Name`, `Subject`, `Issuer`, `Valid from`, `Valid to`,
    /// `Thumbprint` lines. The refusal follows; only a complete, well-formed
    /// description counts.
    pub(super) fn read_certificate(&mut self, line: &str) {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("Certificate details for ") {
            let host = rest.split(" (").next().unwrap_or_default();
            self.certificate = Some(CertificateInfo {
                host: clean_field(host),
                common_name: String::new(),
                subject: String::new(),
                issuer: String::new(),
                valid_from: String::new(),
                valid_to: String::new(),
                fingerprint: String::new(),
            });
            return;
        }
        if let (Some(c), Some(field)) = (self.certificate.as_mut(), line.strip_prefix('\t'))
            && let Some((name, value)) = field.split_once(':')
        {
            let value = clean_field(value);
            match name.trim() {
                "Common Name" => c.common_name = value,
                "Subject" => c.subject = value,
                "Issuer" => c.issuer = value,
                "Valid from" => c.valid_from = value,
                "Valid to" => c.valid_to = value,
                "Thumbprint" => c.fingerprint = value.to_ascii_lowercase(),
                _ => {}
            }
        }
        if self
            .certificate
            .as_ref()
            .is_some_and(|c| is_sha256_fingerprint(&c.fingerprint))
        {
            self.described = self.certificate.take();
        }
        if let Some((_, fp)) = line.split_once(PRESENTED_MARKER) {
            let fp = clean_field(fp).to_ascii_lowercase();
            if is_sha256_fingerprint(&fp) {
                self.presented = Some(fp);
            }
        }
        if line.contains(CERT_REFUSED_MARKER) {
            self.refused = true;
        }
        if self.refused && self.described.is_some() && !self.stop_sent {
            self.stop_sent = true;
            self.stop_now = true;
        }
    }

    /// How the session ended, from FreeRDP's exit code (`None`: killed by
    /// a signal) and what its output said. The order is the precedence:
    /// what the output names exactly wins over the generic exit code.
    pub(super) fn end(&self, code: Option<i32>) -> SessionEnd {
        match (self.refused_certificate(), self.app_refused) {
            (Some(cert), _) => SessionEnd::CertificateNotTrusted(cert),
            (None, Some(why)) => SessionEnd::AppRefused(why),
            _ if self.proxy_auth => SessionEnd::ProxyAuthRefused,
            _ if self.replaced && code.is_some() => SessionEnd::Replaced,
            _ if self.activation_timeout && !self.connected && !self.logon_failure => {
                SessionEnd::HostTimedOut
            }
            _ => classify_exit(code, self.logon_failure, self.connected && self.lost),
        }
    }

    /// The refused certificate, from whatever arrived: stdout (the
    /// description) and stderr (the log lines) are read separately, so
    /// their order isn't known. FreeRDP's own fingerprint line wins; a
    /// description of another certificate is ignored.
    pub(super) fn refused_certificate(&self) -> Option<CertificateInfo> {
        if !self.refused {
            return None;
        }
        match (&self.described, &self.presented) {
            (Some(d), Some(p)) if d.fingerprint == *p => Some(d.clone()),
            (_, Some(p)) => Some(CertificateInfo {
                host: String::new(),
                common_name: String::new(),
                subject: String::new(),
                issuer: String::new(),
                valid_from: String::new(),
                valid_to: String::new(),
                fingerprint: p.clone(),
            }),
            (Some(d), None) => Some(d.clone()),
            (None, None) => None,
        }
    }

    /// Keeps warnings, errors and connection steps (bounded). Returns the
    /// event a connection step means: a stage while starting, `Connected`
    /// the first time the session is active.
    pub(super) fn push(&mut self, line: &str) -> Option<SessionEvent> {
        if line.contains("LOGON_FAILURE") {
            self.logon_failure = true;
        }
        if LOST_MARKERS.iter().any(|m| line.contains(m)) {
            self.lost = true;
        }
        if line.contains(REPLACED_MARKER) {
            self.replaced = true;
        }
        if let Some(why) = AppRefusal::from_log(line) {
            self.app_refused.get_or_insert(why);
        }
        if line
            .split_once(PROXY_ANSWER_MARKER)
            .is_some_and(|(_, answer)| answer.split_whitespace().nth(1) == Some("407"))
        {
            self.proxy_auth = true;
        }
        if line.contains(ACTIVATION_TIMEOUT_MARKER) {
            self.activation_timeout = true;
        }
        self.read_certificate(line);
        // `ERRINFO_…` is the server's reason for ending, logged at INFO
        // (`libfreerdp/core/errinfo.c`); kept so the app can explain it.
        if line.contains("[ERROR]")
            || line.contains("[WARN]")
            || line.contains("ERRINFO_")
            || is_state_change(line)
        {
            if self.lines.len() == MAX_LOG_LINES {
                self.lines.remove(0);
            }
            // Redacted before it is shortened, so a cut token can't slip
            // past the redaction (log.rs).
            self.lines
                .push(crate::log::redact(line).chars().take(300).collect());
        }
        if self.connected || !is_state_change(line) {
            return None;
        }
        if line.contains(ACTIVE_MARKER) {
            self.connected = true;
            return Some(SessionEvent::Connected);
        }
        let (from, to) = line.split_once(" --> ")?;
        if AUTH_STATES.iter().any(|s| to.trim_end().ends_with(s)) {
            return Some(SessionEvent::Stage(Stage::Authenticating));
        }
        if AUTH_STATES.iter().any(|s| from.ends_with(s)) {
            return Some(SessionEvent::Stage(Stage::Connecting));
        }
        None
    }
}
