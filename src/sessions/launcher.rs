// SPDX-License-Identifier: MIT

//! Starts and supervises FreeRDP for one session (spec §8).
//!
//! - All arguments, including the password, go through a private pipe
//!   (`/args-from:fd:3`), so nothing sensitive is on the command line or in
//!   the environment.
//! - The filtered connection file is written to a private runtime folder
//!   (`$XDG_RUNTIME_DIR/cosmic-ext-deskfeed/<random>`, mode 0700, file 0600) and
//!   removed when the session ends.
//! - FreeRDP gets an environment allowlist, runs in its own process group,
//!   and uses our sign-in helper for Entra token requests.
//! - "Connected" comes from FreeRDP's own log, not from process start.

use std::fmt::Write as _;
use std::fs;
use std::io::{BufReader, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::rdpfile::ConnectionFile;
use crate::auth::token::Secret;
use crate::service::http::BearerToken;

/// FreeRDP clients we know, in order of preference (ADR 0004): the X11
/// client. It sends the window's size as soon as the display channel is
/// ready; the SDL client didn't under COSMIC tiling (ADR 0004, client
/// comparison). Some distributions name FreeRDP 3's client `xfreerdp`; the
/// dependency check requires version 3.32.2 either way.
const FREERDP_BINARIES: &[&str] = &["xfreerdp3", "xfreerdp"];
/// Log line FreeRDP writes once the session is set up.
/// The session is active: FreeRDP's last connection step
/// (`libfreerdp/core/connection.c`), logged by `STATE_LOGGER`.
const ACTIVE_MARKER: &str = "--> CONNECTION_STATE_ACTIVE";
/// Entering and leaving the sign-in steps: Entra (`AAD`) or the Windows
/// password (`NLA`), seen in real AVD logs (docs/milestone-1.md).
const AUTH_STATES: &[&str] = &["CONNECTION_STATE_AAD", "CONNECTION_STATE_NLA"];
/// FreeRDP lost a connected session (`client/common/client.c`,
/// `client_auto_reconnect_ex`): logged before it checks whether it may
/// reconnect, so also with its reconnect off.
const LOST_MARKERS: &[&str] = &[
    "client_auto_reconnect_ex]: Network disconnect!",
    "client_auto_reconnect_ex]: Disconnected by server hitting a bug",
];
/// Longest user name and password handed to FreeRDP, in bytes. Windows
/// allows 256 characters for a password and 104 for a user principal name;
/// these leave room for multi-byte characters. With the 16 KiB gateway
/// token the arguments stay far below a pipe's 64 KiB (`send_args`).
const MAX_USER_NAME: usize = 1024;
const MAX_PASSWORD: usize = 1024;

/// FreeRDP's log line when Windows refuses to start a RemoteApp.
const APP_REFUSED_MARKER: &str = "RAIL exec error: execResult=";
/// The server's reason when another connection took over the session
/// (`libfreerdp/core/errinfo.c`, logged at INFO).
const REPLACED_MARKER: &str = "ERRINFO_DISCONNECTED_BY_OTHER_CONNECTION";
/// FreeRDP's log line with the proxy's answer (`http_proxy_connect`).
const PROXY_ANSWER_MARKER: &str = "[http_proxy_connect]: HTTP Proxy: ";
const MAX_LOG_LINES: usize = 60;
const STATE_LOGGER: &str = "com.freerdp.core.rdp";
/// Longest FreeRDP output line looked at; the rest of a line is skipped.
const MAX_OUTPUT_LINE: usize = 4096;

/// Environment every child process (FreeRDP, the sign-in helper) gets;
/// everything else is dropped. See [`pass_environment`].
const BASE_ENV: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "DBUS_SESSION_BUS_ADDRESS",
    "HOME",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_MESSAGES",
    "PATH",
];

/// The variable that tells the sign-in helper where its proxy file is
/// (`signin/src/proxy.rs`); it holds only the path.
const PROXY_FILE_ENV: &str = "COSMIC_EXT_DESKFEED_SIGNIN_PROXY_FILE";

/// The proxy the app's own HTTP client uses: the first set of these, in
/// ureq's order (ureq 3.4.2 `Proxy::try_from_env`).
const APP_PROXY_VARS: &[&str] = &[
    "ALL_PROXY",
    "all_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
];

/// The proxy the app itself uses (ureq 3.4.2, `Proxy::try_from_env`): the
/// first of [`APP_PROXY_VARS`] whose value ureq accepts, and `NO_PROXY`
/// before `no_proxy`. `get` reads an environment variable.
fn app_proxy(get: impl Fn(&str) -> Option<String>) -> FreeRdpProxy {
    FreeRdpProxy {
        https_proxy: APP_PROXY_VARS
            .iter()
            .filter_map(|name| get(name))
            .find(|value| ureq::Proxy::new(value).is_ok()),
        no_proxy: ["NO_PROXY", "no_proxy"].iter().find_map(|name| get(name)),
    }
    .or_none()
}

/// The proxy FreeRDP gets: exactly the app's (spec §7). FreeRDP 3.32.2
/// accepts `http://`, `socks5://` or no scheme (`proxy_parse_uri`).
///
/// Its address may hold a user name and password, so it never goes into an
/// environment (review F03): FreeRDP gets it as `/proxy:` through the
/// private argument pipe, and only `no_proxy` as a variable, which FreeRDP
/// still reads with `/proxy:` set (`libfreerdp/core/proxy.c`,
/// `proxy_prepare`). The sign-in helper reads it from a private file.
fn freerdp_proxy(get: impl Fn(&str) -> Option<String>) -> Result<FreeRdpProxy, LaunchError> {
    let proxy = app_proxy(get);
    let Some(uri) = &proxy.https_proxy else {
        return Ok(proxy);
    };
    let scheme = uri.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        None | Some("http" | "socks5") => Ok(proxy),
        // Connecting directly instead would bypass the proxy the app uses.
        Some(other) => Err(LaunchError::UnsupportedProxy(other.to_owned())),
    }
}

/// The app's proxy; `None`: not set.
#[derive(Debug, Default, PartialEq, Eq)]
struct FreeRdpProxy {
    https_proxy: Option<String>,
    no_proxy: Option<String>,
}

impl FreeRdpProxy {
    /// `no_proxy` alone means nothing.
    fn or_none(self) -> Self {
        if self.https_proxy.is_none() {
            return Self::default();
        }
        self
    }

    /// FreeRDP's `/proxy:` option, when there is a proxy; `None` for a
    /// value that would break the argument pipe.
    fn arg(&self) -> Option<String> {
        self.https_proxy
            .as_ref()
            .filter(|p| !p.contains(['\n', '\r', '\0']))
            .map(|p| format!("/proxy:{p}"))
    }

    /// Writes the sign-in helper's proxy file (`signin/src/proxy.rs`) into
    /// the private folder `dir`, readable only by the user; `None` without
    /// a proxy.
    fn write_file(&self, dir: &Path) -> Result<Option<PathBuf>, LaunchError> {
        let Some(uri) = &self.https_proxy else {
            return Ok(None);
        };
        let text = serde_json::json!({ "uri": uri, "no_proxy": self.no_proxy }).to_string();
        let path = dir.join("proxy.json");
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| f.write_all(text.as_bytes()))
            .map_err(|e| io_err(&e))?;
        Ok(Some(path))
    }
}

/// A private folder holding the sign-in helper's proxy file, for a sign-in
/// the app starts itself; removed when dropped.
pub struct HelperProxy {
    _dir: Option<RuntimeDir>,
    file: Option<PathBuf>,
}

impl HelperProxy {
    /// The app's proxy for its own sign-in window. Any scheme: the web view
    /// isn't limited like FreeRDP.
    pub fn new() -> Result<Self, LaunchError> {
        let proxy = app_proxy(|name| std::env::var(name).ok());
        if proxy.https_proxy.is_none() {
            return Ok(Self {
                _dir: None,
                file: None,
            });
        }
        let dir = RuntimeDir::create(&runtime_base()?)?;
        let file = proxy.write_file(&dir.0)?;
        Ok(Self {
            _dir: Some(dir),
            file,
        })
    }

    /// Tells the helper where the file is (`PROXY_FILE_ENV`), if any.
    pub fn apply(&self, cmd: &mut Command) {
        if let Some(file) = &self.file {
            cmd.env(PROXY_FILE_ENV, file);
        }
    }
}

/// Copies the allowed variables ([`BASE_ENV`] plus `extra`) from the app's
/// environment into `cmd`, which should start with `env_clear()`.
pub fn pass_environment(cmd: &mut Command, extra: &[&str]) {
    for key in BASE_ENV.iter().chain(extra) {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
}

/// How to log on to a host pool without Entra single sign-on
/// (ADR 0005). Deployments differ, so this is a per-resource choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogonSetting {
    /// No domain given: FreeRDP uses `AzureAD`.
    #[default]
    Automatic,
    /// An empty domain (needed by some setups, FreeRDP issue #11887).
    EmptyDomain,
    /// A domain the user typed.
    CustomDomain(String),
    /// Entra authentication to the session host, no password.
    EntraSignIn,
}

impl LogonSetting {
    pub fn needs_password(&self) -> bool {
        !matches!(self, Self::EntraSignIn)
    }
}

pub struct LaunchSpec {
    pub connection: ConnectionFile,
    /// Shown to FreeRDP as `/u:`; the AVD service may replace it.
    pub username: Option<String>,
    pub password: Option<Secret>,
    pub logon: LogonSetting,
    pub helper: PathBuf,
    /// The account's sign-in web view profile, for silent token requests.
    pub profile: PathBuf,
    /// The app's own access token for the AVD service. FreeRDP asks for
    /// exactly this token (same client ID and scope, FreeRDP 3.32.2
    /// `libfreerdp/core/settings.c`) and skips its own request when one is
    /// given (`gateway/arm.c`), which saves the user a sign-in prompt.
    pub gateway_token: Option<BearerToken>,
    /// A session host certificate the user approved for this desktop
    /// (sha256, `aa:bb:…`); FreeRDP accepts exactly it (ADR 0016).
    pub pinned_certificate: Option<String>,
    /// Effective connection settings (account, then desktop overrides).
    pub settings: crate::settings::Effective,
    /// Window title and Wayland app ID for FreeRDP's window, so the dock
    /// groups it with the app and "open on a new workspace" can find it.
    pub window_title: String,
    pub app_id: String,
}

/// A window title FreeRDP can take as one argument line: no control
/// characters, bounded length.
pub fn safe_title(title: &str) -> String {
    title
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    FreeRdpMissing,
    NoRuntimeDir,
    /// Password contains a line break or NUL, which FreeRDP can't take.
    BadPassword,
    /// The app's proxy uses a scheme FreeRDP doesn't support.
    UnsupportedProxy(String),
    Io(String),
}

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
    fn from_log(line: &str) -> Option<Self> {
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
fn clean_field(s: &str) -> String {
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
const CERT_REFUSED_MARKER: &str = "certificate not trusted, aborting";
/// FreeRDP's log line with the presented certificate's fingerprint
/// (`tls_print_new_certificate_warn`, `tls_print_certificate_error`).
const PRESENTED_MARKER: &str = "The fingerprint for the host key sent by the remote host is ";

/// Where a starting session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Setting up the connection (gateway, session host).
    Connecting,
    /// Signing in to Windows: Entra or the password.
    Authenticating,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// A starting session moved on (from FreeRDP's connection steps).
    Stage(Stage),
    /// The session is active.
    Connected,
    Ended {
        end: SessionEnd,
        log: Vec<String>,
    },
}

/// Finds a FreeRDP client in `PATH`.
pub fn find_freerdp() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    FREERDP_BINARIES.iter().find_map(|name| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|p| is_executable(p))
    })
}

/// A regular file someone may execute; a non-executable file of the same
/// name doesn't hide a usable one later in `PATH`.
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The argument lines for `/args-from`. `rdp_path` must end in `.rdp`.
fn build_args(spec: &LaunchSpec, rdp_path: &Path) -> Result<Vec<String>, LaunchError> {
    let gateway = match spec.gateway_token.as_ref().map(BearerToken::expose) {
        // A JWT: base64url parts and dots. Anything else could break the
        // comma-separated option, so FreeRDP then fetches its own token.
        Some(t)
            if !t.is_empty()
                && t.len() <= 16 * 1024
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) =>
        {
            format!("/gateway:type:arm,bearer:{t}")
        }
        _ => "/gateway:type:arm".to_owned(),
    };
    let mut args = vec![rdp_path.to_string_lossy().into_owned(), gateway];
    // FreeRDP runs `~/.config/freerdp/action.sh` for key combinations it
    // lists, and that can take keys from the session. The app sets the
    // session's keys itself (spec §5), so FreeRDP gets a script path that
    // doesn't exist, in the session's private folder (`client/X11/xf_utils.c`
    // `run_action_script`: no file, no script).
    args.push(format!(
        "/action-script:{}",
        rdp_path.with_file_name("no-action-script").display()
    ));
    // The chosen logon decides, not the file's `enablerdsaadauth`: FreeRDP
    // applies the file first and then the arguments, and a lone `/sec:nla`
    // switches Entra (AAD) security off (3.32.2 `client/common/cmdline.c`).
    let entra = spec.logon == LogonSetting::EntraSignIn;
    args.push(if entra { "/sec:aad" } else { "/sec:nla" }.to_owned());
    args.push(format!("/azure:auth-helper:{}", spec.helper.display()));
    if let Some(user) = &spec.username {
        if user.contains(['\n', '\r', '\0']) || user.len() > MAX_USER_NAME {
            return Err(LaunchError::Io("bad user name".into()));
        }
        args.push(format!("/u:{user}"));
    }
    if !entra {
        match &spec.logon {
            LogonSetting::Automatic | LogonSetting::EntraSignIn => {}
            LogonSetting::EmptyDomain => args.push("/d:".to_owned()),
            LogonSetting::CustomDomain(d) => args.push(format!("/d:{d}")),
        }
        if let Some(pw) = &spec.password {
            let pw = pw.expose();
            if pw.is_empty() || pw.contains(['\n', '\r', '\0']) || pw.len() > MAX_PASSWORD {
                return Err(LaunchError::BadPassword);
            }
            args.push(format!("/p:{pw}"));
        }
    }
    // Only well-formed values; otherwise FreeRDP asks (and refuses), which
    // ends in the approval dialog again.
    if let Some(fp) = spec
        .pinned_certificate
        .as_deref()
        .filter(|f| is_sha256_fingerprint(f))
    {
        args.push(format!("/cert:fingerprint:sha256:{fp}"));
    }
    args.extend([
        "/dynamic-resolution".to_owned(),
        "/log-level:info".to_owned(),
        // The app reconnects, with a fresh connection file and gateway
        // token (ADR 0014). FreeRDP's own loop reuses the first token and
        // shows its own window; this also overrides the feed's
        // `autoreconnection enabled`.
        "-auto-reconnect".to_owned(),
    ]);
    // Display, devices and `/timeout`. FreeRDP waits `TcpAckTimeout`
    // (default 9 s) for the session host to finish activation, including
    // the Entra exchange (`connection.c` `rdp_client_wait_for_activation`);
    // measured on the owner's host it took up to 49.8 s (settings.rs).
    args.extend(if spec.connection.remote_app() {
        spec.settings.freerdp_app_args()
    } else {
        spec.settings.freerdp_args()
    });
    let title = safe_title(&spec.window_title);
    if !title.is_empty() {
        args.push(format!("/title:{title}"));
    }
    if !spec.app_id.is_empty()
        && spec
            .app_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        args.push(format!("/wm-class:{}", spec.app_id));
    }
    if args.iter().any(|a| a.contains(['\n', '\r', '\0'])) {
        return Err(LaunchError::Io("bad argument".into()));
    }
    Ok(args)
}

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

/// A private folder that is removed when dropped.
struct RuntimeDir(PathBuf);

fn runtime_base() -> Result<PathBuf, LaunchError> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or(LaunchError::NoRuntimeDir)
}

impl RuntimeDir {
    fn create(base: &Path) -> Result<Self, LaunchError> {
        let root = base.join("cosmic-ext-deskfeed");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .map_err(|e| LaunchError::Io(e.kind().to_string()))?;
        let mut bytes = [0u8; 12];
        getrandom::fill(&mut bytes).map_err(|_| LaunchError::Io("random".into()))?;
        let name: String = bytes.iter().fold(String::new(), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        });
        let dir = root.join(name);
        // `create`, not `create_all`: fails if it exists, so no one can
        // prepare it for us.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| LaunchError::Io(e.kind().to_string()))?;
        Ok(Self(dir))
    }
}

impl Drop for RuntimeDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Removes session folders left behind by a crash.
pub fn clean_stale_runtime_dirs() {
    if let Some(base) = std::env::var_os("XDG_RUNTIME_DIR") {
        let root = PathBuf::from(base).join("cosmic-ext-deskfeed");
        if let Ok(entries) = fs::read_dir(&root) {
            for e in entries.flatten() {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

/// A running FreeRDP session. Clones refer to the same process.
#[derive(Clone)]
pub struct Session {
    child: Arc<Mutex<Child>>,
    pgid: i32,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("pgid", &self.pgid)
            .finish_non_exhaustive()
    }
}

/// How long FreeRDP gets to end by itself before it is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

impl Session {
    /// Asks FreeRDP to end, then forces it after a grace period, in the
    /// background. Only this session's process group is signalled.
    pub fn disconnect(&self) {
        let this = self.clone();
        std::thread::spawn(move || stop_all(&[this], STOP_GRACE));
    }

    fn exited(&self) -> bool {
        self.child
            .lock()
            .expect("not poisoned")
            .try_wait()
            .ok()
            .flatten()
            .is_some()
    }
}

/// Ends sessions and waits for them: SIGTERM to each process group, up to
/// `grace` for FreeRDP to exit, then SIGKILL to each group, which also
/// reaches anything FreeRDP started that is still running (the sign-in
/// helper). Blocks; not for the UI thread.
pub fn stop_all(sessions: &[Session], grace: Duration) {
    for s in sessions {
        // SAFETY: plain syscall; negative pid = our own child's group.
        unsafe { libc::kill(-s.pgid, libc::SIGTERM) };
    }
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline && !sessions.iter().all(Session::exited) {
        std::thread::sleep(Duration::from_millis(50));
    }
    for s in sessions {
        // The group ID stays reserved while any process is still in the
        // group, so this can't reach an unrelated process group; an empty
        // group gives ESRCH, which is fine.
        // SAFETY: as above.
        unsafe { libc::kill(-s.pgid, libc::SIGKILL) };
        let _ = s.child.lock().expect("not poisoned").wait();
    }
}

/// Starts FreeRDP. `events` receives `Connected` (when seen) and exactly one
/// `Ended`.
pub fn launch(
    spec: &LaunchSpec,
    events: impl Fn(SessionEvent) + Send + Sync + 'static,
) -> Result<Session, LaunchError> {
    let freerdp = find_freerdp().ok_or(LaunchError::FreeRdpMissing)?;
    let proxy = freerdp_proxy(|name| std::env::var(name).ok())?;
    launch_with(&freerdp, &runtime_base()?, spec, &proxy, events)
}

fn launch_with(
    freerdp: &Path,
    runtime: &Path,
    spec: &LaunchSpec,
    proxy: &FreeRdpProxy,
    events: impl Fn(SessionEvent) + Send + Sync + 'static,
) -> Result<Session, LaunchError> {
    let dir = RuntimeDir::create(runtime)?;
    let rdp_path = write_connection_file(&dir, &spec.connection)?;
    let mut args = build_args(spec, &rdp_path)?;
    args.extend(proxy.arg());
    let proxy_file = proxy.write_file(&dir.0)?;
    let (read_end, write_end) = pipe()?;
    let (terminal, output) = output_terminal().map_err(|e| io_err(&e))?;
    let mut child = spawn_freerdp(
        freerdp,
        &spec.profile,
        &read_end,
        output,
        proxy,
        proxy_file.as_deref(),
    )?;
    drop(read_end);
    if send_args(write_end, &args).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(LaunchError::Io("argument pipe".into()));
    }
    supervise(child, terminal, dir, Arc::new(events))
}

fn io_err(e: &std::io::Error) -> LaunchError {
    LaunchError::Io(e.kind().to_string())
}

fn write_connection_file(
    dir: &RuntimeDir,
    connection: &ConnectionFile,
) -> Result<PathBuf, LaunchError> {
    let path = dir.0.join("connection.rdp");
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| io_err(&e))?;
    f.write_all(connection.to_freerdp_text().as_bytes())
        .map_err(|e| io_err(&e))?;
    Ok(path)
}

/// A pseudo-terminal for FreeRDP's standard output: (our end, its end).
///
/// FreeRDP writes DEBUG and INFO log lines to stdout and the rest to
/// stderr (FreeRDP 3.32.2 `winpr/libwinpr/utils/wlog/ConsoleAppender.c`,
/// no setting changes this). The C library holds stdout back in blocks
/// when it is a pipe, but writes each line when it is a terminal (C11
/// 7.21.3: stdout is fully buffered only when not interactive; glibc
/// line-buffers a terminal). Through a pipe the connection steps and the
/// connected line came in blocks, some only when FreeRDP ended. FreeRDP
/// checks for a terminal only on its input (password and certificate
/// prompts, `libfreerdp/utils/passphrase.c`, `client/common/client.c`),
/// which stays `/dev/null`.
///
/// Raw mode: no line-ending translation. Not a controlling terminal
/// (`O_NOCTTY`), so it can't send signals to anyone.
fn output_terminal() -> std::io::Result<(fs::File, OwnedFd)> {
    use std::os::fd::FromRawFd;
    let last = std::io::Error::last_os_error;
    // SAFETY: plain libc calls on descriptors we own; the name buffer is
    // NUL-terminated by `ptsname_r` on success.
    unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(last());
        }
        let ours = OwnedFd::from_raw_fd(fd);
        if libc::grantpt(fd) != 0 || libc::unlockpt(fd) != 0 {
            return Err(last());
        }
        let mut name = [0 as libc::c_char; 128];
        let err = libc::ptsname_r(fd, name.as_mut_ptr(), name.len());
        if err != 0 {
            return Err(std::io::Error::from_raw_os_error(err));
        }
        let theirs = libc::open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        );
        if theirs < 0 {
            return Err(last());
        }
        let theirs = OwnedFd::from_raw_fd(theirs);
        let mut mode: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(theirs.as_raw_fd(), &raw mut mode) != 0 {
            return Err(last());
        }
        libc::cfmakeraw(&raw mut mode);
        if libc::tcsetattr(theirs.as_raw_fd(), libc::TCSANOW, &raw const mode) != 0 {
            return Err(last());
        }
        Ok((fs::File::from(ours), theirs))
    }
}

fn spawn_freerdp(
    freerdp: &Path,
    profile: &Path,
    args_pipe: &OwnedFd,
    output: OwnedFd,
    proxy: &FreeRdpProxy,
    proxy_file: Option<&Path>,
) -> Result<Child, LaunchError> {
    let read_fd = args_pipe.as_raw_fd();
    let mut cmd = Command::new(freerdp);
    cmd.arg("/args-from:fd:3")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::piped())
        .process_group(0);
    pass_environment(&mut cmd, &["PULSE_SERVER"]);
    // The proxy itself goes through the argument pipe (`FreeRdpProxy`).
    if let Some(n) = &proxy.no_proxy {
        cmd.env("no_proxy", n);
    }
    if let Some(file) = proxy_file {
        cmd.env(PROXY_FILE_ENV, file);
    }
    cmd.env("COSMIC_EXT_DESKFEED_SIGNIN_PROFILE", profile);
    // The sign-in window's title in the user's language; the helper has no
    // translations of its own (signin/src/freerdp.rs, `TITLE_ENV`).
    let title: String = crate::fl!("sign-in-window-title")
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    cmd.env("COSMIC_EXT_DESKFEED_SIGNIN_TITLE", title);
    // Connection steps ("STATE_A --> STATE_B"), so a failure shows where it
    // stopped. This logger prints no names, passwords or tokens (FreeRDP
    // 3.32.2 `libfreerdp/core/rdp.c`). An environment variable, because
    // `/log-filters:` comes too late for loggers that already exist
    // (`winpr/libwinpr/utils/wlog/wlog.c` caches the level on creation).
    cmd.env("WLOG_FILTER", format!("{STATE_LOGGER}:DEBUG"));
    // SAFETY: only async-signal-safe calls (dup2, fcntl) between fork and
    // exec. The pipe's read end becomes fd 3 without close-on-exec.
    unsafe {
        cmd.pre_exec(move || {
            if read_fd == 3 {
                let flags = libc::fcntl(3, libc::F_GETFD);
                if flags < 0 || libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(read_fd, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    spawn_tied(cmd).map_err(|e| io_err(&e))
}

/// A program start for the spawner thread, and where its result goes.
type SpawnRequest = (Command, std::sync::mpsc::Sender<std::io::Result<Child>>);

/// Starts `cmd` so that it ends when Deskfeed does, even after a crash or
/// a kill (spec §6, §8: no orphaned sessions). The kernel sends it SIGKILL
/// when its parent goes (`PR_SET_PDEATHSIG`): uncatchable, because after a
/// crash nothing is left to escalate a SIGTERM that FreeRDP ignored
/// (review F12; ADR 0022 addendum). Normal quitting stays graceful
/// (`stop_all`). "Parent" is the *thread* that
/// started it, and the async runtime's worker threads come and go, so every
/// start happens on one thread that lasts as long as the app. A child in
/// its own process group (FreeRDP) passes the signal on to its group by
/// ending; its helper then loses its pipes.
pub fn spawn_tied(mut cmd: Command) -> std::io::Result<Child> {
    static SPAWNER: std::sync::OnceLock<Mutex<std::sync::mpsc::Sender<SpawnRequest>>> =
        std::sync::OnceLock::new();
    let parent = i32::try_from(std::process::id()).unwrap_or(0);
    // SAFETY: only async-signal-safe calls (prctl, getppid) between fork
    // and exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The app ended before the line above took effect.
            if libc::getppid() != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
    let spawner = SPAWNER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<SpawnRequest>();
        std::thread::Builder::new()
            .name("spawner".into())
            .spawn(move || {
                // Lives as long as the app: the sender is in a static.
                for (mut cmd, reply) in rx {
                    let _ = reply.send(spawn_retrying(&mut cmd));
                }
            })
            .expect("start the spawner thread");
        Mutex::new(tx)
    });
    let (reply, result) = std::sync::mpsc::channel();
    spawner
        .lock()
        .expect("not poisoned")
        .send((cmd, reply))
        .map_err(|_| std::io::Error::other("spawner gone"))?;
    result
        .recv()
        .map_err(|_| std::io::Error::other("spawner gone"))?
}

/// `spawn`, retried briefly on "text file busy": the executable may still
/// be open for writing in a process another thread just forked (seen with
/// freshly written test scripts).
fn spawn_retrying(cmd: &mut Command) -> std::io::Result<Child> {
    let mut attempts = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 20 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            other => return other,
        }
    }
}

/// Writes the argument lines and closes the pipe (an empty line ends them).
/// Hands the arguments over. Never blocks (review F07): the whole payload
/// must fit in the pipe's buffer, and the write end is non-blocking, so a
/// FreeRDP that never reads them can't hold up launching or quitting. On
/// an error the caller stops FreeRDP.
fn send_args(write_end: OwnedFd, args: &[String]) -> std::io::Result<()> {
    let fd = write_end.as_raw_fd();
    // SAFETY: plain fcntl calls on a descriptor we own.
    let capacity = unsafe { libc::fcntl(fd, libc::F_GETPIPE_SZ) };
    // SAFETY: as above.
    let nonblocking = unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
    };
    let mut w = fs::File::from(write_end);
    let mut payload = args.join("\n");
    payload.push_str("\n\n");
    let fits = usize::try_from(capacity).is_ok_and(|c| payload.len() <= c);
    let result = if fits && nonblocking {
        w.write_all(payload.as_bytes())
    } else {
        Err(std::io::Error::other("arguments don't fit the pipe"))
    };
    // Wipe our copy of the password from this buffer.
    // SAFETY: overwriting bytes with zeros keeps the String valid UTF-8.
    unsafe { payload.as_bytes_mut().fill(0) };
    result
}

/// Reads FreeRDP's log, waits for it to exit, removes the runtime folder
/// and reports the end.
fn supervise(
    mut child: Child,
    stdout: fs::File,
    dir: RuntimeDir,
    events: Arc<dyn Fn(SessionEvent) + Send + Sync>,
) -> Result<Session, LaunchError> {
    let pgid = i32::try_from(child.id()).map_err(|_| LaunchError::Io("pid".into()))?;
    let stderr = child.stderr.take().expect("piped");
    let child = Arc::new(Mutex::new(child));
    let log = Arc::new(Mutex::new(Log::default()));
    // Set before `Ended`: readers that are still going report nothing more.
    let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let readers: Vec<_> = [Box::new(stdout) as Box<dyn Read + Send>, Box::new(stderr)]
        .into_iter()
        .map(|stream| {
            let log = log.clone();
            let events = events.clone();
            let ended = ended.clone();
            std::thread::spawn(move || {
                let report = |e| {
                    if !ended.load(std::sync::atomic::Ordering::SeqCst) {
                        events(e);
                    }
                };
                // Bounded: FreeRDP's output is not trusted to be sane.
                for line in
                    bounded_lines::lines(BufReader::new(stream), MAX_OUTPUT_LINE).map(|l| l.text)
                {
                    // Our sign-in helper's status lines (FreeRDP passes its
                    // stderr through); they hold no codes or tokens.
                    if line.starts_with("cosmic-ext-deskfeed-signin: ") || is_state_change(&line) {
                        crate::app_log!("{line}");
                    }
                    let (event, stop) = {
                        let mut log = log.lock().expect("not poisoned");
                        let event = log.push(&line);
                        (event, std::mem::take(&mut log.stop_now))
                    };
                    if let Some(event) = event {
                        report(event);
                    }
                    if stop {
                        // SAFETY: plain syscall on FreeRDP's own process group.
                        unsafe { libc::kill(-pgid, libc::SIGTERM) };
                    }
                }
            })
        })
        .collect();

    let watched = child.clone();
    std::thread::spawn(move || {
        let status = loop {
            if let Ok(Some(s)) = watched.lock().expect("not poisoned").try_wait() {
                break s;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        // Give the readers a moment to collect the last lines. Children of
        // FreeRDP (the sign-in helper) may hold the pipes open longer, so
        // don't wait for them to close.
        for _ in 0..20 {
            if readers.iter().all(std::thread::JoinHandle::is_finished) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(dir); // removes the connection file
        let log = std::mem::take(&mut *log.lock().expect("not poisoned"));
        let end = match (log.refused_certificate(), log.app_refused) {
            (Some(cert), _) => SessionEnd::CertificateNotTrusted(cert),
            (None, Some(why)) => SessionEnd::AppRefused(why),
            _ if log.proxy_auth => SessionEnd::ProxyAuthRefused,
            _ if log.replaced && status.code().is_some() => SessionEnd::Replaced,
            _ => classify_exit(status.code(), log.logon_failure, log.connected && log.lost),
        };
        ended.store(true, std::sync::atomic::Ordering::SeqCst);
        events(SessionEvent::Ended {
            end,
            log: log.lines,
        });
    });

    Ok(Session { child, pgid })
}

/// A FreeRDP connection step (`STATE_A --> STATE_B`); no secrets.
fn is_state_change(line: &str) -> bool {
    line.contains(&format!("[DEBUG][{STATE_LOGGER}]")) && line.contains(" --> ")
}

// Independent facts read from the log; a bitflag type would only hide them.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
struct Log {
    lines: Vec<String>,
    connected: bool,
    logon_failure: bool,
    /// The connection was lost (`LOST_MARKERS`).
    lost: bool,
    /// Another connection took over the session (`REPLACED_MARKER`).
    replaced: bool,
    /// Windows refused to start the RemoteApp (`APP_REFUSED_MARKER`).
    app_refused: Option<AppRefusal>,
    /// The proxy asked for a password (`PROXY_ANSWER_MARKER`, 407).
    proxy_auth: bool,
    /// The certificate FreeRDP is describing (`Certificate details for`).
    certificate: Option<CertificateInfo>,
    /// A complete description (it has a well-formed thumbprint).
    described: Option<CertificateInfo>,
    /// The fingerprint from FreeRDP's own log line about the new host key.
    presented: Option<String>,
    /// FreeRDP refused the certificate.
    refused: bool,
    stop_sent: bool,
    /// Set once when the refusal and the description are both in: the
    /// reader ends FreeRDP, which otherwise may wait after it.
    stop_now: bool,
}

impl Log {
    /// FreeRDP's description of an unverified certificate
    /// (`client/common/client.c`, `client_cli_verify_certificate_ex`):
    /// `Certificate details for <host> (<name>):`, then tab-indented
    /// `Common Name`, `Subject`, `Issuer`, `Valid from`, `Valid to`,
    /// `Thumbprint` lines. The refusal follows; only a complete, well-formed
    /// description counts.
    fn read_certificate(&mut self, line: &str) {
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

    /// The refused certificate, from whatever arrived: stdout (the
    /// description) and stderr (the log lines) are read separately, so
    /// their order isn't known. FreeRDP's own fingerprint line wins; a
    /// description of another certificate is ignored.
    fn refused_certificate(&self) -> Option<CertificateInfo> {
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
    fn push(&mut self, line: &str) -> Option<SessionEvent> {
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
        self.read_certificate(line);
        if line.contains("[ERROR]") || line.contains("[WARN]") || is_state_change(line) {
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

fn pipe() -> Result<(OwnedFd, OwnedFd), LaunchError> {
    let mut fds = [0; 2];
    // SAFETY: plain syscall with a valid two-element array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(LaunchError::Io("pipe".into()));
    }
    // SAFETY: pipe2 just returned these two new descriptors to us.
    unsafe {
        use std::os::fd::FromRawFd;
        Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(logon: LogonSetting, password: Option<&str>, entra_file: bool) -> LaunchSpec {
        let text = if entra_file {
            "full address:s:h\r\ngatewayhostname:s:rdgateway.wvd.microsoft.com\r\nenablerdsaadauth:i:1\r\n"
        } else {
            "full address:s:h\r\ngatewayhostname:s:rdgateway.wvd.microsoft.com\r\n"
        };
        LaunchSpec {
            connection: ConnectionFile::parse(text.as_bytes()).unwrap(),
            username: Some("anna@example.com".into()),
            password: password.map(|p| Secret::new(p.into())),
            logon,
            helper: PathBuf::from("/usr/libexec/cosmic-ext-deskfeed/cosmic-ext-deskfeed-signin"),
            profile: PathBuf::from("/tmp/p"),
            gateway_token: None,
            pinned_certificate: None,
            settings: crate::settings::DEFAULTS,
            window_title: String::new(),
            app_id: String::new(),
        }
    }

    fn args(s: &LaunchSpec) -> Vec<String> {
        build_args(
            s,
            Path::new("/run/user/1000/cosmic-ext-deskfeed/x/connection.rdp"),
        )
        .unwrap()
    }

    #[test]
    fn password_host_automatic() {
        let a = args(&spec(LogonSetting::Automatic, Some("p@ss w0rd,;"), false));
        assert_eq!(a[0], "/run/user/1000/cosmic-ext-deskfeed/x/connection.rdp");
        assert!(a.contains(&"/gateway:type:arm".into()));
        assert!(a.contains(&"/sec:nla".into()));
        assert!(a.contains(&"/p:p@ss w0rd,;".into()));
        assert!(a.contains(&"/u:anna@example.com".into()));
        assert!(
            a.iter()
                .any(|x| x.starts_with("/azure:auth-helper:/usr/libexec/"))
        );
        // Automatic: no domain at all, so FreeRDP applies `AzureAD`.
        assert!(!a.iter().any(|x| x.starts_with("/d:")));
    }

    #[test]
    fn empty_and_custom_domain() {
        let a = args(&spec(LogonSetting::EmptyDomain, Some("pw"), false));
        assert!(a.contains(&"/d:".into()));
        let a = args(&spec(
            LogonSetting::CustomDomain("CORP".into()),
            Some("pw"),
            false,
        ));
        assert!(a.contains(&"/d:CORP".into()));
    }

    #[test]
    fn entra_sign_in_sends_no_password() {
        for entra_file in [false, true] {
            let a = args(&spec(LogonSetting::EntraSignIn, Some("pw"), entra_file));
            assert!(a.contains(&"/sec:aad".into()));
            assert!(
                !a.iter()
                    .any(|x| x.starts_with("/p:") || x.starts_with("/d:"))
            );
        }
    }

    #[test]
    fn a_chosen_password_logon_wins_over_the_files_entra_hint() {
        let a = args(&spec(LogonSetting::EmptyDomain, Some("pw"), true));
        assert!(a.contains(&"/sec:nla".into()));
        assert!(!a.contains(&"/sec:aad".into()));
        assert!(a.contains(&"/p:pw".into()));
        assert!(a.contains(&"/d:".into()));
        let a = args(&spec(LogonSetting::Automatic, Some("pw"), true));
        assert!(a.contains(&"/sec:nla".into()));
        assert!(a.contains(&"/p:pw".into()));
    }

    #[test]
    fn window_title_and_app_id() {
        let mut s = spec(LogonSetting::EntraSignIn, None, false);
        s.window_title = "Standard Desktop – anna@example.com\n/p:x".into();
        s.app_id = "io.github.rabol.cosmic-ext-deskfeed".into();
        let a = args(&s);
        // The line break is dropped, so nothing becomes a separate argument.
        assert!(a.contains(&"/title:Standard Desktop – anna@example.com/p:x".into()));
        assert!(a.contains(&"/wm-class:io.github.rabol.cosmic-ext-deskfeed".into()));
        assert!(!a.iter().any(|x| x == "/p:x"));
        s.app_id = "bad id;x".into();
        assert!(!args(&s).iter().any(|x| x.starts_with("/wm-class:")));
        s.window_title = "\n".into();
        assert!(!args(&s).iter().any(|x| x.starts_with("/title:")));
    }

    #[test]
    fn a_remote_app_file_gets_the_app_options() {
        let mut s = spec(LogonSetting::EntraSignIn, None, false);
        s.settings.fullscreen = true;
        s.connection = ConnectionFile::parse(
            b"full address:s:h\r\ngatewayhostname:s:rdgateway.wvd.microsoft.com\r\n\
              remoteapplicationmode:i:1\r\nremoteapplicationprogram:s:||notepad\r\n",
        )
        .unwrap();
        let a = args(&s);
        assert!(!a.contains(&"+f".into()));
        assert!(a.contains(&"/multimon".into()));
        assert!(a.contains(&"-grab-keyboard".into()));
        // The same settings for a desktop.
        s.connection = spec(LogonSetting::EntraSignIn, None, false).connection;
        assert!(args(&s).contains(&"+f".into()));
    }

    #[test]
    fn only_executable_files_count() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("avd-exec-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("xfreerdp3");
        fs::write(&file, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable(&file));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable(&file));
        assert!(!is_executable(&dir));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn freerdp_gets_the_proxy_the_app_uses() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let set = |p: &str, n: Option<&str>| {
            Ok(FreeRdpProxy {
                https_proxy: Some(p.into()),
                no_proxy: n.map(Into::into),
            })
        };
        assert_eq!(freerdp_proxy(env(&[])), Ok(FreeRdpProxy::default()));
        assert_eq!(
            freerdp_proxy(env(&[("http_proxy", "http://p:3128")])),
            set("http://p:3128", None)
        );
        // The app's order wins: ALL_PROXY before HTTPS_PROXY.
        assert_eq!(
            freerdp_proxy(env(&[
                ("HTTPS_PROXY", "http://h:3128"),
                ("ALL_PROXY", "socks5://s:1080"),
            ])),
            set("socks5://s:1080", None)
        );
        assert_eq!(
            freerdp_proxy(env(&[("HTTP_PROXY", "p:3128")])),
            set("p:3128", None)
        );
        // NO_PROXY before no_proxy, as the app reads them.
        assert_eq!(
            freerdp_proxy(env(&[
                ("https_proxy", "http://p:3128"),
                ("no_proxy", "lower.example"),
                ("NO_PROXY", "upper.example"),
            ])),
            set("http://p:3128", Some("upper.example"))
        );
        // Values the app ignores are ignored here too.
        assert_eq!(
            freerdp_proxy(env(&[("http_proxy", "")])),
            Ok(FreeRdpProxy::default())
        );
        // A scheme FreeRDP can't use: an error, never a direct connection.
        assert_eq!(
            freerdp_proxy(env(&[("http_proxy", "https://p:443")])),
            Err(LaunchError::UnsupportedProxy("https".into()))
        );
    }

    #[test]
    fn gateway_token_only_when_well_formed() {
        let mut s = spec(LogonSetting::EntraSignIn, None, false);
        s.gateway_token = Some(BearerToken::new("eyJ0.eyJh-b_c.sig".into()));
        assert_eq!(args(&s)[1], "/gateway:type:arm,bearer:eyJ0.eyJh-b_c.sig");
        for bad in ["a,type:rpc", "a b", "", "a:b"] {
            s.gateway_token = Some(BearerToken::new(bad.into()));
            assert_eq!(args(&s)[1], "/gateway:type:arm", "{bad:?}");
        }
    }

    #[test]
    fn rejects_passwords_that_would_break_the_pipe() {
        for pw in ["a\nb", "a\rb", "a\0b", ""] {
            let s = spec(LogonSetting::Automatic, Some(pw), false);
            assert_eq!(
                build_args(&s, Path::new("/x.rdp")),
                Err(LaunchError::BadPassword),
                "{pw:?}"
            );
        }
    }

    /// Run by `children_end_when_the_app_crashes` in a process of its own:
    /// starts a long `sleep`, prints its pid, then crashes.
    #[test]
    #[ignore = "run by children_end_when_the_app_crashes"]
    fn crashing_app() {
        if std::env::var("DESKFEED_TEST_CRASH").is_err() {
            return;
        }
        // A child that ignores SIGTERM, as FreeRDP once did (ADR 0020); an
        // ignored signal stays ignored across `exec`.
        let mut cmd = Command::new("sh");
        // No inherited output: the outer test reads until it closes.
        cmd.args(["-c", "trap '' TERM; exec sleep 60"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let child = spawn_tied(cmd).unwrap();
        println!("CHILD {}", child.id());
        std::io::Write::flush(&mut std::io::stdout()).unwrap();
        std::process::abort();
    }

    #[test]
    fn children_end_when_the_app_crashes() {
        let out = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "sessions::launcher::tests::crashing_app",
            ])
            .args(["--nocapture", "--test-threads=1"])
            .env("DESKFEED_TEST_CRASH", "1")
            .output()
            .unwrap();
        assert!(!out.status.success(), "it should have crashed");
        let text = String::from_utf8_lossy(&out.stdout);
        let pid: u32 = text
            .lines()
            .find_map(|l| l.split_once("CHILD ").map(|(_, pid)| pid))
            .unwrap_or_else(|| panic!("no pid in {text}"))
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .parse()
            .unwrap();
        // Gone, or a zombie waiting for its new parent to reap it.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let state = fs::read_to_string(format!("/proc/{pid}/stat")).ok();
            let alive = state
                .as_deref()
                .and_then(|s| s.rsplit(')').next())
                .is_some_and(|rest| !rest.trim_start().starts_with('Z'));
            if !alive {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "sleep {pid} outlived the app"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn handing_over_arguments_never_blocks() {
        // Nobody reads the pipe, and the arguments are bigger than it.
        let (read_end, write_end) = pipe().unwrap();
        let args = vec!["x".repeat(1024 * 1024)];
        let started = std::time::Instant::now();
        assert!(send_args(write_end, &args).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(read_end);
        // Normal arguments fit.
        let (read_end, write_end) = pipe().unwrap();
        send_args(write_end, &["/u:anna".into(), "/p:pw".into()]).unwrap();
        let mut got = String::new();
        fs::File::from(read_end).read_to_string(&mut got).unwrap();
        assert_eq!(got, "/u:anna\n/p:pw\n\n");
        // Too long a password or user name never reaches the pipe.
        let mut s = spec(LogonSetting::Automatic, Some(&"p".repeat(2000)), false);
        assert_eq!(
            build_args(&s, Path::new("/x.rdp")).unwrap_err(),
            LaunchError::BadPassword
        );
        s.password = None;
        s.username = Some("u".repeat(2000));
        assert!(build_args(&s, Path::new("/x.rdp")).is_err());
    }

    #[test]
    fn exit_codes() {
        assert_eq!(classify_exit(Some(0), false, false), SessionEnd::Closed);
        // A lost connection, whatever FreeRDP's exit code says.
        assert_eq!(
            classify_exit(Some(147), false, true),
            SessionEnd::ConnectionLost
        );
        assert_eq!(
            classify_exit(Some(0), false, true),
            SessionEnd::ConnectionLost
        );
        assert_eq!(classify_exit(None, false, true), SessionEnd::Crashed);
        assert_eq!(classify_exit(Some(11), false, false), SessionEnd::Closed);
        assert_eq!(
            classify_exit(Some(134), false, false),
            SessionEnd::LogonFailed
        );
        assert_eq!(
            classify_exit(Some(131), true, false),
            SessionEnd::LogonFailed
        );
        assert_eq!(
            classify_exit(Some(135), false, false),
            SessionEnd::AccountLockedOut
        );
        assert_eq!(
            classify_exit(Some(149), false, false),
            SessionEnd::PasswordExpired
        );
        assert_eq!(
            classify_exit(Some(140), false, false),
            SessionEnd::DnsFailed
        );
        assert_eq!(
            classify_exit(Some(131), false, false),
            SessionEnd::Failed { code: 131 }
        );
        assert_eq!(classify_exit(None, false, false), SessionEnd::Crashed);
        // Another connection took over: never a lost connection.
        assert_eq!(classify_exit(Some(5), false, true), SessionEnd::Replaced);
        assert_eq!(
            classify_exit(Some(1), false, false),
            SessionEnd::DisconnectedByServer
        );
        assert_eq!(classify_exit(Some(12), false, false), SessionEnd::Closed);
    }

    #[test]
    fn reads_a_refused_remote_app_and_a_replaced_session() {
        // As `xf_rail.c` and `errinfo.c` in 3.32.2 print them.
        let mut log = Log::default();
        assert_eq!(log.app_refused, None);
        log.push(
            "[10:01:02:003] [1:2] [ERROR][com.freerdp.client.x11.rail] - [xf_rail_server_execute_result]: \
             RAIL exec error: execResult=RAIL_EXEC_E_NOT_IN_ALLOWLIST [0x00000003] NtError=0x0",
        );
        assert_eq!(log.app_refused, Some(AppRefusal::NotAllowed));
        for (name, why) in [
            ("RAIL_EXEC_E_FILE_NOT_FOUND", AppRefusal::NotFound),
            ("RAIL_EXEC_E_SESSION_LOCKED", AppRefusal::SessionLocked),
            ("RAIL_EXEC_E_FAIL", AppRefusal::Other),
            ("RAIL_EXEC_E_UNKNOWN", AppRefusal::Other),
        ] {
            assert_eq!(
                AppRefusal::from_log(&format!("RAIL exec error: execResult={name} [0x1]")),
                Some(why),
                "{name}"
            );
        }
        assert!(!log.replaced);
        log.push(
            "[10:01:02:003] [1:2] [INFO][com.freerdp.core] - [rdp_print_errinfo]: \
             ERRINFO_DISCONNECTED_BY_OTHER_CONNECTION (0x00000005):Another user connected to the server, \
             forcing the disconnection of the current connection.",
        );
        assert!(log.replaced);
        assert!(!log.proxy_auth);
        // As 3.32.2 printed it against an authenticating test proxy.
        log.push(
            "[12:16:11:369] [1:2] [INFO][com.freerdp.core.proxy] - [http_proxy_connect]: \
             HTTP Proxy: HTTP/1.1 407 Proxy Authentication Required",
        );
        assert!(log.proxy_auth);
        let mut other = Log::default();
        other.push(
            "[INFO][x] - [http_proxy_connect]: HTTP Proxy: HTTP/1.1 200 Connection established",
        );
        assert!(!other.proxy_auth);
    }

    /// As xfreerdp3 3.32.2 printed it for the local test server's
    /// throwaway certificate (research/rdp-test-server).
    const REFUSAL: &str = "Certificate details for /tmp/tfreerdp-server.33900:3389 (RDP-Server):
\tCommon Name: (null)
\tSubject:     (null)
\tIssuer:      (null)
\tValid from:  Oct 18 08:01:10 2026 GMT
\tValid to:    Oct 18 08:01:10 2027 GMT
\tThumbprint:  c8:3c:d4:90:e0:4e:ca:86:9d:9e:71:aa:89:09:90:0c:94:da:3a:bf:cf:b8:dc:9a:32:59:09:c7:2b:d9:e4:af
The above X.509 certificate could not be verified, possibly because you do not have
Do you trust the above certificate? (Y/T/N) [15:13:10:299] [675264:000a4dc2] [ERROR][com.freerdp.crypto] - [freerdp_tls_handshake]: certificate not trusted, aborting.";

    #[test]
    fn reads_a_refused_certificate() {
        let mut log = Log::default();
        for line in REFUSAL.lines() {
            log.push(line);
        }
        // FreeRDP's own log line, read on the other stream: it may come
        // before or after the description.
        log.push("[ERROR][com.freerdp.crypto] - [tls_print_new_certificate_warn]: The fingerprint for the host key sent by the remote host is c8:3c:d4:90:e0:4e:ca:86:9d:9e:71:aa:89:09:90:0c:94:da:3a:bf:cf:b8:dc:9a:32:59:09:c7:2b:d9:e4:af");
        let c = log.refused_certificate().expect("refused");
        assert_eq!(c.host, "/tmp/tfreerdp-server.33900:3389");
        assert_eq!(c.valid_to, "Oct 18 08:01:10 2027 GMT");
        assert!(c.fingerprint.starts_with("c8:3c:d4") && is_sha256_fingerprint(&c.fingerprint));
        assert!(log.stop_now);
        // The refusal first, the description later: still found.
        let mut log = Log::default();
        let mut lines: Vec<&str> = REFUSAL.lines().collect();
        lines.rotate_right(1);
        for line in lines {
            log.push(line);
        }
        assert_eq!(
            log.refused_certificate().unwrap().valid_to,
            "Oct 18 08:01:10 2027 GMT"
        );
        // A malformed thumbprint isn't something to approve.
        let mut log = Log::default();
        for line in REFUSAL.replace("c8:3c", "zz:3c").lines() {
            log.push(line);
        }
        assert!(log.refused_certificate().is_none() && !log.stop_now);
        // Only FreeRDP's log line: the fingerprint, without details.
        let mut log = Log::default();
        log.push("[ERROR][com.freerdp.crypto] - [tls_print_new_certificate_warn]: The fingerprint for the host key sent by the remote host is C8:3C:D4:90:E0:4E:CA:86:9D:9E:71:AA:89:09:90:0C:94:DA:3A:BF:CF:B8:DC:9A:32:59:09:C7:2B:D9:E4:AF");
        log.push("[ERROR][com.freerdp.crypto] - [freerdp_tls_handshake]: certificate not trusted, aborting.");
        let c = log.refused_certificate().unwrap();
        assert!(c.fingerprint.starts_with("c8:3c") && c.issuer.is_empty());
        // Control characters from the network are dropped.
        assert_eq!(clean_field(" a\u{1b}[31mb\t "), "a[31mb");
    }

    #[test]
    fn pinned_certificate_goes_to_freerdp() {
        let fp = "c8:3c:d4:90:e0:4e:ca:86:9d:9e:71:aa:89:09:90:0c:94:da:3a:bf:cf:b8:dc:9a:32:59:09:c7:2b:d9:e4:af";
        let mut s = spec(LogonSetting::EntraSignIn, None, true);
        assert!(!args(&s).iter().any(|a| a.starts_with("/cert")));
        s.pinned_certificate = Some(fp.into());
        assert!(args(&s).contains(&format!("/cert:fingerprint:sha256:{fp}")));
        // Never anything else through this field.
        s.pinned_certificate = Some(format!("{fp},ignore"));
        assert!(!args(&s).iter().any(|a| a.starts_with("/cert")));
    }

    #[test]
    fn log_keeps_errors_and_detects_connection() {
        let mut log = Log::default();
        let step = |from: &str, to: &str| {
            format!(
                "[DEBUG][com.freerdp.core.rdp] - [rdp_client_transition_to_state][0x1]: CONNECTION_STATE_{from} --> CONNECTION_STATE_{to}"
            )
        };
        assert_eq!(log.push("[INFO][com.freerdp.core] - something"), None);
        // Channels load before the session is active: not connected yet.
        assert_eq!(
            log.push("[INFO][com.freerdp.channels.drdynvc.client] - Loading Dynamic Virtual Channel rdpgfx"),
            None
        );
        assert_eq!(
            log.push(&step("NEGO", "AAD")),
            Some(SessionEvent::Stage(Stage::Authenticating))
        );
        assert_eq!(
            log.push(&step("AAD", "MCS_CREATE_REQUEST")),
            Some(SessionEvent::Stage(Stage::Connecting))
        );
        assert_eq!(
            log.push(&step("LICENSING", "MULTITRANSPORT_BOOTSTRAPPING_REQUEST")),
            None
        );
        assert_eq!(
            log.push(&step("FINALIZATION_CLIENT_FONT_MAP", "ACTIVE")),
            Some(SessionEvent::Connected)
        );
        // Only once, and nothing more after it.
        assert_eq!(log.push(&step("NEGO", "NLA")), None);
        assert_eq!(
            log.push(&step("FINALIZATION_CLIENT_FONT_MAP", "ACTIVE")),
            None
        );
        log.lines.clear();
        assert!(!log.lost);
        // As xfreerdp3 3.32.2 printed it when the local test server went away.
        log.push("[14:13:52:196] [664309:000a22f0] [INFO][com.freerdp.client.common] - [client_auto_reconnect_ex]: Network disconnect!");
        assert!(log.lost);
        log.push("[ERROR][com.freerdp.core.rdstls] - RDSTLS_RESULT_LOGON_FAILURE [0x0000052E]");
        assert!(log.logon_failure);
        assert_eq!(log.lines.len(), 1);
        log.push(&step("NEGO", "AAD"));
        log.push("[DEBUG][com.freerdp.core.rdp] - [rdp_recv_pdu]: recv Data PDU, length: 12");
        assert_eq!(log.lines.len(), 2);
        for i in 0..100 {
            log.push(&format!("[WARN] line {i}"));
        }
        assert_eq!(log.lines.len(), MAX_LOG_LINES);
    }

    /// Runs a fake FreeRDP that records what it received, to check the
    /// pipe hand-off, the private connection file and cleanup.
    #[test]
    fn launch_hands_arguments_over_privately() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("avd-launch-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.txt");
        let fake = dir.join("xfreerdp3");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh\n\
                 out='{out}'\n\
                 echo \"ARGV:$*\" > \"$out\"\n\
                 first=''\n\
                 while IFS= read -r line <&3; do [ -z \"$line\" ] && break; [ -z \"$first\" ] && first=\"$line\"; echo \"ARG:$line\" >> \"$out\"; done\n\
                 echo \"MODE:$(stat -c %a \"$first\")\" >> \"$out\"\n\
                 echo \"DIRMODE:$(stat -c %a \"$(dirname \"$first\")\")\" >> \"$out\"\n\
                 grep -q drivestoredirect \"$first\" && echo DRIVES >> \"$out\"\n\
                 env | grep -q AVD_TEST_SECRET && echo LEAKED_ENV >> \"$out\"\n\
                 env | grep -qi 'pr0xy-pw' && echo PROXY_IN_ENV >> \"$out\"\n\
                 env | grep -q '^no_proxy=.corp.example$' && echo NO_PROXY_OK >> \"$out\"\n\
                 pf=\"$COSMIC_EXT_DESKFEED_SIGNIN_PROXY_FILE\"\n\
                 [ -n \"$pf\" ] && echo \"PROXYFILE:$(stat -c %a \"$pf\"):$(cat \"$pf\")\" >> \"$out\"\n\
                 env | grep -q '^COSMIC_EXT_DESKFEED_SIGNIN_PROFILE=/tmp/p$' && echo PROFILE_OK >> \"$out\"\n\
                 env | grep -q '^WLOG_FILTER=com.freerdp.core.rdp:DEBUG$' && echo FILTER_OK >> \"$out\"\n\
                 echo '[DEBUG][com.freerdp.core.rdp] - [rdp_client_transition_to_state][0x1]: CONNECTION_STATE_FINALIZATION_CLIENT_FONT_MAP --> CONNECTION_STATE_ACTIVE'\n\
                 exit 0\n",
                out = out.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = dir.join("run");
        fs::create_dir_all(&runtime).unwrap();
        // SAFETY: no other test reads this variable.
        unsafe { std::env::set_var("AVD_TEST_SECRET", "x") };

        let (tx, rx) = std::sync::mpsc::channel();
        let s = spec(LogonSetting::Automatic, Some("hunter2"), false);
        // A proxy with a (synthetic) password: only through the pipe and
        // the private file, never the environment (review F03).
        let proxy = FreeRdpProxy {
            https_proxy: Some("http://anna:pr0xy-pw@proxy.example:3128".into()),
            no_proxy: Some(".corp.example".into()),
        };
        let _session = launch_with(&fake, &runtime, &s, &proxy, move |e| {
            tx.send(e).unwrap();
        })
        .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            SessionEvent::Connected
        );
        let SessionEvent::Ended { end, .. } = rx.recv_timeout(Duration::from_secs(10)).unwrap()
        else {
            panic!("expected end")
        };
        assert_eq!(end, SessionEnd::Closed);

        let got = fs::read_to_string(&out).unwrap();
        assert!(got.contains("ARGV:/args-from:fd:3\n"), "{got}");
        assert!(
            !got.lines().next().unwrap().contains("hunter2"),
            "password on command line"
        );
        assert!(got.contains("ARG:/p:hunter2"), "{got}");
        assert!(got.contains("ARG:/gateway:type:arm"));
        assert!(got.contains("MODE:600"), "{got}");
        assert!(got.contains("DIRMODE:700"), "{got}");
        assert!(got.contains("PROFILE_OK"), "{got}");
        assert!(got.contains("FILTER_OK"), "{got}");
        assert!(!got.contains("DRIVES"));
        assert!(!got.contains("LEAKED_ENV"));
        assert!(
            got.contains("ARG:/proxy:http://anna:pr0xy-pw@proxy.example:3128"),
            "{got}"
        );
        assert!(!got.contains("PROXY_IN_ENV"), "{got}");
        assert!(got.contains("NO_PROXY_OK"), "{got}");
        assert!(
            got.contains(
                r#"PROXYFILE:600:{"uri":"http://anna:pr0xy-pw@proxy.example:3128","no_proxy":".corp.example"}"#
            ),
            "{got}"
        );
        // The connection file and its folder are gone.
        assert_eq!(
            fs::read_dir(runtime.join("cosmic-ext-deskfeed"))
                .unwrap()
                .count(),
            0
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// FreeRDP's log lines arrive while it runs. `sed` writes through the C
    /// library like FreeRDP: through a pipe the line would wait in its
    /// buffer until `sleep` ends.
    #[test]
    fn log_lines_arrive_while_freerdp_runs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("avd-launch-live-{}", std::process::id()));
        fs::create_dir_all(dir.join("run")).unwrap();
        let fake = dir.join("fake");
        fs::write(
            &fake,
            "#!/bin/sh\ncat <&3 >/dev/null\n\
             { echo '[DEBUG][com.freerdp.core.rdp] - [rdp_client_transition_to_state][0x1]: CONNECTION_STATE_FINALIZATION_CLIENT_FONT_MAP --> CONNECTION_STATE_ACTIVE'; sleep 5; } | sed -n p\n",
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let s = spec(LogonSetting::Automatic, Some("pw"), false);
        let session = launch_with(
            &fake,
            &dir.join("run"),
            &s,
            &FreeRdpProxy::default(),
            move |e| tx.send(e).unwrap(),
        )
        .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            SessionEvent::Connected
        );
        stop_all(&[session], STOP_GRACE);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reports_crash_and_failure_codes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("avd-launch-fail-{}", std::process::id()));
        fs::create_dir_all(dir.join("run")).unwrap();
        for (script, want) in [
            (
                "echo '[ERROR][x] - ERRCONNECT_LOGON_FAILURE [0x00020014]'; exit 131",
                SessionEnd::LogonFailed,
            ),
            ("kill -9 $$", SessionEnd::Crashed),
            ("exit 140", SessionEnd::DnsFailed),
        ] {
            let fake = dir.join("fake");
            fs::write(&fake, format!("#!/bin/sh\ncat <&3 >/dev/null\n{script}\n")).unwrap();
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let s = spec(LogonSetting::Automatic, Some("pw"), false);
            let _session = launch_with(
                &fake,
                &dir.join("run"),
                &s,
                &FreeRdpProxy::default(),
                move |e| tx.send(e).unwrap(),
            )
            .unwrap();
            match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                SessionEvent::Ended { end, log } => {
                    assert_eq!(end, want, "{script}");
                    if want == SessionEnd::LogonFailed {
                        assert_eq!(log.len(), 1);
                    }
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    /// A FreeRDP that ignores SIGTERM and leaves a child behind (like the
    /// sign-in helper) is still ended completely, child included.
    #[test]
    fn stopping_ends_the_whole_process_group() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("avd-launch-stop-{}", std::process::id()));
        fs::create_dir_all(dir.join("run")).unwrap();
        let out = dir.join("child.txt");
        let fake = dir.join("fake");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh
cat <&3 >/dev/null
trap '' TERM
sleep 300 &
echo $! > '{}'
sleep 300
",
                out.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let s = spec(LogonSetting::Automatic, Some("pw"), false);
        let session = launch_with(
            &fake,
            &dir.join("run"),
            &s,
            &FreeRdpProxy::default(),
            move |e| tx.send(e).unwrap(),
        )
        .unwrap();
        let child: i32 = (0..100)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(50));
                fs::read_to_string(&out).ok()?.trim().parse().ok()
            })
            .expect("fake started its child");

        stop_all(&[session], Duration::from_millis(300));

        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            SessionEvent::Ended { .. }
        ));
        // The child is gone (or a zombie waiting for init to reap it).
        let gone = (0..40).any(|_| {
            let state = fs::read_to_string(format!("/proc/{child}/stat")).ok();
            if state.is_none_or(|s| s.rsplit(") ").next().is_some_and(|r| r.starts_with('Z'))) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
            false
        });
        assert!(gone, "child {child} still running");
        fs::remove_dir_all(dir).unwrap();
    }

    /// `launch_with`'s steps for a test host without the Azure gateway:
    /// Deskfeed's own arguments minus `/gateway:`, changed by `adjust`.
    fn start_without_gateway(
        s: &LaunchSpec,
        dir: &Path,
        adjust: impl FnOnce(Vec<String>) -> Vec<String>,
    ) -> (Session, std::sync::mpsc::Receiver<SessionEvent>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let freerdp = find_freerdp().expect("xfreerdp3 installed");
        let rundir = RuntimeDir::create(&dir.join("run")).unwrap();
        let rdp_path = write_connection_file(&rundir, &s.connection).unwrap();
        let args = build_args(s, &rdp_path)
            .unwrap()
            .into_iter()
            .filter(|a| !a.starts_with("/gateway:"))
            .collect();
        let args = adjust(args);
        let (read_end, write_end) = pipe().unwrap();
        let (terminal, output) = output_terminal().unwrap();
        let child = spawn_freerdp(
            &freerdp,
            &s.profile,
            &read_end,
            output,
            &FreeRdpProxy::default(),
            None,
        )
        .unwrap();
        drop(read_end);
        send_args(write_end, &args).unwrap();
        let session = supervise(
            child,
            terminal,
            rundir,
            Arc::new(move |e| {
                let _ = tx.send(e);
            }),
        )
        .unwrap();
        (session, rx)
    }

    /// A RemoteApp from a Windows host with RemoteApp allowed (ADR 0020):
    /// `DESKFEED_TEST_RAIL_HOST`, `_USER`, `_PASSWORD`, `_PROGRAM` (a path
    /// or `||alias`), optional `_SECS` (default 15). `_EXPECT=missing`
    /// expects Windows to refuse a program that isn't there;
    /// `_EXPECT=taken` expects another connection (a second run, started
    /// within `_SECS`) to take the session over. Checks the launcher's
    /// events; the window shows on the running session.
    #[test]
    #[ignore = "needs a Windows host with RemoteApp allowed"]
    fn windows_host_remote_app() {
        let var = |name: &str| std::env::var(format!("DESKFEED_TEST_RAIL_{name}"));
        let host = var("HOST").expect("DESKFEED_TEST_RAIL_HOST");
        let secs: u64 = var("SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(15);
        let dir = std::env::temp_dir().join(format!("avd-rail-{}", std::process::id()));
        fs::create_dir_all(dir.join("run")).unwrap();
        let s = LaunchSpec {
            connection: ConnectionFile::local_test_app(
                &host,
                &var("PROGRAM").expect("DESKFEED_TEST_RAIL_PROGRAM"),
            ),
            username: Some(var("USER").expect("DESKFEED_TEST_RAIL_USER")),
            password: Some(Secret::new(
                var("PASSWORD").expect("DESKFEED_TEST_RAIL_PASSWORD"),
            )),
            logon: LogonSetting::Automatic,
            helper: PathBuf::from("/nonexistent/helper"),
            profile: dir.join("profile"),
            gateway_token: None,
            pinned_certificate: None,
            settings: crate::settings::Effective {
                // Set for a desktop; an app must not get them.
                fullscreen: true,
                system_shortcuts: true,
                scale: crate::display::resolve_percent(0),
                ..crate::settings::DEFAULTS
            },
            window_title: "Deskfeed RemoteApp test".into(),
            app_id: "io.github.rabol.DeskfeedTest".into(),
        };
        // A test host's self-signed certificate.
        let (session, rx) = start_without_gateway(&s, &dir, |mut args| {
            assert!(args.contains(&"/multimon".to_owned()), "{args:?}");
            assert!(!args.contains(&"+f".to_owned()), "{args:?}");
            args.push("/cert:ignore".to_owned());
            args
        });
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        let ended = loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match rx.recv_timeout(left) {
                Ok(SessionEvent::Ended { end, log }) => break Some((end, log)),
                Ok(e) => {
                    println!("event: {e:?}");
                    let connected = e == SessionEvent::Connected;
                    events.push(e);
                    if connected && var("EXPECT").is_err() {
                        break None;
                    }
                    if connected && var("EXPECT").is_ok_and(|v| v == "taken") {
                        let left = Duration::from_secs(secs);
                        match rx.recv_timeout(left) {
                            Ok(SessionEvent::Ended { end, log }) => break Some((end, log)),
                            other => panic!("not taken over: {other:?}"),
                        }
                    }
                }
                Err(e) => panic!("nothing happened in time ({e}): {events:?}"),
            }
        };
        if var("EXPECT").is_ok_and(|v| v == "missing") {
            let (end, log) = ended.expect("an end");
            println!("ended: {end:?}");
            for line in &log {
                println!("  {line}");
            }
            assert_eq!(end, SessionEnd::AppRefused(AppRefusal::NotFound));
        } else if var("EXPECT").is_ok_and(|v| v == "taken") {
            let (end, log) = ended.expect("an end");
            println!("ended: {end:?}");
            for line in log.iter().filter(|l| l.contains("ERRINFO")) {
                println!("  {line}");
            }
            assert!(
                matches!(end, SessionEnd::DisconnectedByServer | SessionEnd::Replaced),
                "{end:?}"
            );
        } else {
            assert!(ended.is_none(), "{ended:?}");
            std::thread::sleep(Duration::from_secs(secs));
            stop_all(&[session], STOP_GRACE);
            let rest: Vec<_> = rx.try_iter().collect();
            println!("after stopping: {rest:?}");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    /// Real `xfreerdp3` through Deskfeed's own launch path, against the
    /// local test server (`research/rdp-test-server/README.md`):
    /// `DESKFEED_TEST_SERVER=<its socket> cargo test --release
    /// local_test_server_session -- --ignored`. Runs `DESKFEED_TEST_SECS`
    /// (default 20) seconds, with the camera when `DESKFEED_TEST_CAMERA=1`.
    /// With `DESKFEED_TEST_EXPECT_LOST=1` it instead expects the session to
    /// end as a lost connection within that time (stop the server);
    /// the window's tiling and the server's resize log
    /// are checked from outside.
    #[test]
    #[ignore = "needs xfreerdp3 and the local test server"]
    fn local_test_server_session() {
        let socket = std::env::var("DESKFEED_TEST_SERVER").expect("DESKFEED_TEST_SERVER");
        let secs: u64 = std::env::var("DESKFEED_TEST_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);
        let dir = std::env::temp_dir().join(format!("avd-local-{}", std::process::id()));
        fs::create_dir_all(dir.join("run")).unwrap();
        let s = LaunchSpec {
            connection: ConnectionFile::local_test(&socket),
            username: Some("test".into()),
            password: Some(Secret::new("test".into())),
            logon: LogonSetting::EmptyDomain,
            helper: PathBuf::from("/nonexistent/helper"),
            profile: dir.join("profile"),
            gateway_token: None,
            // `DESKFEED_TEST_PIN=<sha256>`: connect with that approved
            // certificate instead of `/cert:ignore`.
            pinned_certificate: std::env::var("DESKFEED_TEST_PIN").ok(),
            settings: crate::settings::Effective {
                // `DESKFEED_TEST_CAMERA=1`: with the camera channel
                // (research/rdp-test-server logs what the client offers).
                camera: std::env::var("DESKFEED_TEST_CAMERA").is_ok_and(|v| v == "1"),
                // `DESKFEED_TEST_FULLSCREEN=1`: full screen.
                fullscreen: std::env::var("DESKFEED_TEST_FULLSCREEN").is_ok_and(|v| v == "1"),
                // "Automatic", resolved as `app/connect.rs` does.
                scale: crate::display::resolve_percent(0),
                ..crate::settings::DEFAULTS
            },
            // `DESKFEED_TEST_TITLE`: another title, for two sessions at once.
            window_title: std::env::var("DESKFEED_TEST_TITLE")
                .unwrap_or_else(|_| "Deskfeed local test \u{2013} t\u{e9}st".into()),
            app_id: "io.github.rabol.DeskfeedTest".into(),
        };
        // The two arguments the test server can't take: TLS instead of
        // NLA; and without a pin its self-signed certificate is ignored,
        // unless `DESKFEED_TEST_CHECK_CERT=1`.
        let ignore_cert =
            s.pinned_certificate.is_none() && std::env::var("DESKFEED_TEST_CHECK_CERT").is_err();
        let (session, rx) = start_without_gateway(&s, &dir, |args| {
            args.into_iter()
                .map(|a| {
                    if a == "/sec:nla" {
                        "/sec:tls".to_owned()
                    } else {
                        a
                    }
                })
                .chain(ignore_cert.then(|| "/cert:ignore".to_owned()))
                .collect()
        });
        if std::env::var("DESKFEED_TEST_CHECK_CERT").is_ok() {
            // Expect the refusal, ended by the launcher at once.
            let SessionEvent::Ended { end, log } =
                rx.recv_timeout(Duration::from_secs(15)).unwrap()
            else {
                panic!("expected an end");
            };
            println!("ended: {end:?}");
            for line in &log {
                println!("  {line}");
            }
            assert!(matches!(end, SessionEnd::CertificateNotTrusted(_)));
            stop_all(&[session], STOP_GRACE);
            fs::remove_dir_all(dir).unwrap();
            return;
        }
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(15)).unwrap(),
            SessionEvent::Connected
        );
        if std::env::var("DESKFEED_TEST_EXPECT_LOST").is_ok_and(|v| v == "1") {
            // The server is stopped from outside meanwhile.
            let ended = rx.recv_timeout(Duration::from_secs(secs)).unwrap();
            assert!(
                matches!(
                    ended,
                    SessionEvent::Ended {
                        end: SessionEnd::ConnectionLost,
                        ..
                    }
                ),
                "{ended:?}"
            );
        } else {
            std::thread::sleep(Duration::from_secs(secs));
        }
        stop_all(&[session], STOP_GRACE);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Real `xfreerdp3`: every property we keep, plus our arguments, must
    /// pass FreeRDP's argument parsing. Without `loadbalanceinfo` FreeRDP stops
    /// right after parsing, before any network access.
    #[test]
    #[ignore = "needs xfreerdp3"]
    fn real_freerdp_accepts_all_kept_properties() {
        let ints = [
            "gatewayusagemethod:i:1",
            "gatewayprofileusagemethod:i:1",
            "gatewaycredentialssource:i:0",
            "gatewaybrokeringtype:i:1",
            "remoteapplicationmode:i:0",
            "enablerdsaadauth:i:0",
            "enablecredsspsupport:i:1",
            "promptcredentialonce:i:1",
            "targetisaadjoined:i:0",
            "use redirection server name:i:0",
            "audiomode:i:0",
            "videoplaybackmode:i:1",
            "use multimon:i:0",
            "screen mode id:i:2",
            "smart sizing:i:1",
            "dynamic resolution:i:1",
            "autoreconnection enabled:i:1",
        ];
        let strs = [
            "full address:s:127.0.0.1",
            "alternate full address:s:127.0.0.1",
            // Never contacted: FreeRDP stops before the network (see above).
            "gatewayhostname:s:rdgateway.wvd.microsoft.com:443",
            "armpath:s:x",
            "aadtenantid:s:x",
            "geo:s:x",
            "hubdiscoverygeourl:s:https://127.0.0.1/",
            "wvd endpoint pool:s:x",
            "workspace id:s:x",
            "resourceprovider:s:x",
            "remotedesktopname:s:x",
            "kdcproxyname:s:x",
            "remoteapplicationprogram:s:||x",
            "remoteapplicationname:s:x",
            "remoteapplicationcmdline:s:/x \"y z\"",
        ];
        let mut text = ints
            .iter()
            .chain(&strs)
            .copied()
            .collect::<Vec<_>>()
            .join("\r\n");
        text.push_str("\r\n");
        let connection = ConnectionFile::parse(text.as_bytes()).unwrap();
        let dir = std::env::temp_dir().join(format!("avd-real-freerdp-{}", std::process::id()));
        fs::create_dir_all(dir.join("run")).unwrap();
        let s = LaunchSpec {
            connection,
            username: Some("anna@example.com".into()),
            password: Some(Secret::new("pw".into())),
            logon: LogonSetting::EmptyDomain,
            helper: PathBuf::from("/nonexistent/helper"),
            profile: dir.join("profile"),
            gateway_token: None,
            pinned_certificate: None,
            settings: crate::settings::DEFAULTS,
            window_title: String::new(),
            app_id: String::new(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let freerdp = find_freerdp().expect("xfreerdp3 installed");
        let _session = launch_with(
            &freerdp,
            &dir.join("run"),
            &s,
            &FreeRdpProxy::default(),
            move |e| tx.send(e).unwrap(),
        )
        .unwrap();
        let SessionEvent::Ended { log, .. } = rx.recv_timeout(Duration::from_secs(30)).unwrap()
        else {
            panic!("expected end")
        };
        let parse_errors: Vec<_> = log.iter().filter(|l| l.contains("cmdline")).collect();
        assert!(parse_errors.is_empty(), "{parse_errors:#?}");
        assert!(
            log.iter().any(|l| l.contains("loadBalanceInfo")),
            "{log:#?}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn random_freerdp_output_never_breaks_the_reader() {
        let fp = "aa:".repeat(31) + "aa";
        let output = format!(
            "[1:2] [DEBUG][{STATE_LOGGER}] - [x]: CONNECTION_STATE_NEGO --> CONNECTION_STATE_AAD\n\
             Certificate details for host:3389 (RDP-Server):\n\
             \tCommon Name: host\n\tSubject: CN = host\n\tIssuer: CN = host\n\
             \tValid from: Oct 1\n\tValid to: Oct 2\n\tThumbprint: {fp}\n\
             [WARN][x] - The fingerprint for the host key sent by the remote host is {fp}\n\
             [ERROR][x] - certificate not trusted, aborting.\n\
             [ERROR][x] - RAIL exec error: execResult=RAIL_EXEC_E_NOT_IN_ALLOWLIST [0x3]\n\
             [INFO][x] - ERRINFO_DISCONNECTED_BY_OTHER_CONNECTION (0x5):Another user\n\
             [DEBUG][{STATE_LOGGER}] - [x]: X --> {ACTIVE_MARKER}\n"
        );
        crate::test_util::random_inputs("freerdp output", &[output.as_bytes()], |input| {
            let mut log = Log::default();
            for line in String::from_utf8_lossy(input).lines() {
                let _ = log.push(line);
            }
            // Bounded, and what reaches the user is one printable line.
            assert!(log.lines.len() <= MAX_LOG_LINES);
            assert!(log.lines.iter().all(|l| l.chars().count() <= 300));
            if let Some(c) = log.refused_certificate() {
                for field in [
                    &c.host,
                    &c.common_name,
                    &c.subject,
                    &c.issuer,
                    &c.valid_from,
                    &c.valid_to,
                    &c.fingerprint,
                ] {
                    assert!(field.chars().count() <= 256, "{field:?}");
                    assert!(!field.chars().any(char::is_control), "{field:?}");
                }
            }
        });
    }
}
