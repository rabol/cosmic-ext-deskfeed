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

use common::helper::{self, LOGIN_HINT_ENV, PROXY_FILE_ENV};

use serde::{Deserialize, Serialize};

use super::rdpfile::ConnectionFile;
use crate::auth::token::Secret;
use crate::service::http::BearerToken;

mod args;
mod output;
mod process;
#[cfg(test)]
mod tests;

pub use args::*;
pub use output::*;
pub use process::*;

/// FreeRDP clients we know, in order of preference (ADR 0004): the X11
/// client. It sends the window's size as soon as the display channel is
/// ready; the SDL client didn't under COSMIC tiling (ADR 0004, client
/// comparison). Some distributions name FreeRDP 3's client `xfreerdp`; the
/// dependency check requires at least version 3.32.2 either way.
const FREERDP_BINARIES: &[&str] = &["xfreerdp3", "xfreerdp"];

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

/// A domain the user typed for `CustomDomain`: letters, digits, `.`, `-`
/// and `_`, at most 255.
pub fn valid_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 255
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

impl LogonSetting {
    pub fn needs_password(&self) -> bool {
        !matches!(self, Self::EntraSignIn)
    }
}

pub struct LaunchSpec {
    pub connection: ConnectionFile,
    pub target: Target,
    /// Shown to FreeRDP as `/u:`; the AVD service may replace it.
    pub username: Option<String>,
    pub password: Option<Secret>,
    pub logon: LogonSetting,
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

/// Where FreeRDP connects.
pub enum Target {
    /// An Azure Virtual Desktop or Windows 365 desktop or app: through
    /// Azure's gateway, with FreeRDP's Azure sign-in going through
    /// Deskfeed's sign-in helper.
    Azure {
        helper: PathBuf,
        /// The account's sign-in web view profile, for silent token
        /// requests.
        profile: PathBuf,
        /// The app's own access token for the AVD service. FreeRDP asks for
        /// exactly this token (same client ID and scope, FreeRDP 3.32.2
        /// `libfreerdp/core/settings.c`) and skips its own request when one
        /// is given (`gateway/arm.c`), which saves the user a sign-in
        /// prompt.
        gateway_token: Option<BearerToken>,
        /// The account's user name, so FreeRDP's own sign-in for the
        /// desktop starts with it selected (ADR 0027).
        login_hint: Option<String>,
    },
    /// A Windows machine by host name (ADR 0029): the connection file
    /// (`ConnectionFile::direct`) names it; no gateway and no Azure
    /// sign-in; the password over NLA only.
    // Started from the screens in step 4 of ADR 0029; until then by the
    // tests only.
    #[cfg_attr(not(test), allow(dead_code))]
    Direct,
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
    /// Entra sign-in to the desktop needs Azure (`Target::Azure`).
    EntraNeedsAzure,
    Io(String),
}

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
    launch_with(
        &freerdp,
        &runtime_base()?,
        spec,
        &proxy,
        events,
        std::convert::identity,
    )
}

/// `edit_args` changes FreeRDP's arguments last: the app leaves them as
/// they are; tests against a host without the Azure gateway take out
/// `/gateway:`.
fn launch_with(
    freerdp: &Path,
    runtime: &Path,
    spec: &LaunchSpec,
    proxy: &FreeRdpProxy,
    events: impl Fn(SessionEvent) + Send + Sync + 'static,
    edit_args: impl FnOnce(Vec<String>) -> Vec<String>,
) -> Result<Session, LaunchError> {
    let dir = RuntimeDir::create(runtime)?;
    // The clipboard is switched on in the file (`to_freerdp_text_with`).
    let clipboard = spec.settings.clipboard != crate::settings::clipboard::OFF;
    let rdp_path = write_connection_file(&dir, &spec.connection, clipboard)?;
    let mut args = build_args(spec, &rdp_path)?;
    args.extend(proxy.arg());
    let args = edit_args(args);
    let proxy_file = proxy.write_file(&dir.0)?;
    let (read_end, write_end) = pipe()?;
    let (terminal, output) = output_terminal().map_err(|e| io_err(&e))?;
    let mut child = spawn_freerdp(
        freerdp,
        &spec.target,
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
    clipboard: bool,
) -> Result<PathBuf, LaunchError> {
    let path = dir.0.join("connection.rdp");
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| io_err(&e))?;
    f.write_all(connection.to_freerdp_text_with(clipboard).as_bytes())
        .map_err(|e| io_err(&e))?;
    Ok(path)
}
