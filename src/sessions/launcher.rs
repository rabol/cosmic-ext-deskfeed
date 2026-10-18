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

/// FreeRDP clients we know, in order of preference (ADR 0004).
const FREERDP_BINARIES: &[&str] = &["sdl-freerdp3", "sdl-freerdp"];
/// Log line FreeRDP writes once the session is set up.
const CONNECTED_MARKER: &str = "Loading Dynamic Virtual Channel";
const MAX_LOG_LINES: usize = 60;
const STATE_LOGGER: &str = "com.freerdp.core.rdp";
/// Longest FreeRDP output line looked at; the rest of a line is skipped.
const MAX_OUTPUT_LINE: usize = 4096;
/// FreeRDP's SDL3 display channel logger; at DEBUG it reports when the
/// channel is ready (`client/SDL/SDL3/sdl_disp.cpp`, `DisplayControlCaps`).
/// Its lines hold only monitor counts and area factors.
const DISPLAY_LOGGER: &str = "com.freerdp.client.sdl.disp";
/// Logged by that channel, in a windowed session only, once it is ready.
const DISPLAY_READY_MARKER: &str = "DisplayControlCapsPdu: setting the window as resizable";
/// FreeRDP's SDL3 client logger. SDL's own messages go through it at its
/// level (`client/SDL/SDL3/sdl_freerdp.cpp`, `SDL_SetLogOutputFunction`);
/// at DEBUG it reports each window event with the window's position and
/// size (`sdl_context.cpp`, `handleEvent(const SDL_WindowEvent&)`).
const SDL_LOGGER: &str = "com.freerdp.client.SDL";
/// The desktop window took a new size. SDL sends this only when the size
/// really changed, so a size the window ignored doesn't produce it.
const WINDOW_RESIZED_MARKER: &str = "SDL_EVENT_WINDOW_RESIZED: [";

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
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
];

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

/// The proxy FreeRDP gets: exactly the app's (spec §7), under the names
/// FreeRDP reads.
///
/// The app (ureq 3.4.2, `Proxy::try_from_env`) takes the first of
/// [`APP_PROXY_VARS`] whose value it accepts, and `NO_PROXY` before
/// `no_proxy`. FreeRDP 3.32.2 reads `https_proxy`, then `HTTPS_PROXY`, and
/// `no_proxy` before `NO_PROXY` (`libfreerdp/core/proxy.c`,
/// `proxy_prepare`), and accepts `http://`, `socks5://` or no scheme
/// (`proxy_parse_uri`). So FreeRDP gets only `https_proxy` and `no_proxy`,
/// set to the app's choice. `get` reads an environment variable.
fn freerdp_proxy(get: impl Fn(&str) -> Option<String>) -> Result<FreeRdpProxy, LaunchError> {
    let Some(proxy) = APP_PROXY_VARS
        .iter()
        .filter_map(|name| get(name))
        .find(|value| ureq::Proxy::new(value).is_ok())
    else {
        return Ok(FreeRdpProxy::default());
    };
    let scheme = proxy.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        None | Some("http" | "socks5") => Ok(FreeRdpProxy {
            https_proxy: Some(proxy),
            no_proxy: ["NO_PROXY", "no_proxy"].iter().find_map(|name| get(name)),
        }),
        // Connecting directly instead would bypass the proxy the app uses.
        Some(other) => Err(LaunchError::UnsupportedProxy(other.to_owned())),
    }
}

/// Proxy variables for FreeRDP; `None`: not set.
#[derive(Debug, Default, PartialEq, Eq)]
struct FreeRdpProxy {
    https_proxy: Option<String>,
    no_proxy: Option<String>,
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

/// Why a session ended, from FreeRDP's exit code (SDL client) and log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// Normal end: disconnect, log off, or the user closed the window.
    Closed,
    LogonFailed,
    AccountLockedOut,
    PasswordExpired,
    AccountDisabled,
    AuthenticationFailed,
    DnsFailed,
    TlsFailed,
    Cancelled,
    Failed {
        code: i32,
    },
    Crashed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Connected,
    /// The display channel is ready in a windowed session: from now on a
    /// window event makes FreeRDP send the window's size (workspace.rs).
    DisplayReady,
    /// The desktop window's size changed.
    WindowResized,
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
    // The chosen logon decides, not the file's `enablerdsaadauth`: FreeRDP
    // applies the file first and then the arguments, and a lone `/sec:nla`
    // switches Entra (AAD) security off (3.32.2 `client/common/cmdline.c`).
    let entra = spec.logon == LogonSetting::EntraSignIn;
    args.push(if entra { "/sec:aad" } else { "/sec:nla" }.to_owned());
    args.push(format!("/azure:auth-helper:{}", spec.helper.display()));
    if let Some(user) = &spec.username {
        if user.contains(['\n', '\r', '\0']) {
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
            if pw.is_empty() || pw.contains(['\n', '\r', '\0']) {
                return Err(LaunchError::BadPassword);
            }
            args.push(format!("/p:{pw}"));
        }
    }
    args.extend([
        "/dynamic-resolution".to_owned(),
        "/log-level:info".to_owned(),
    ]);
    // Display, devices and `/timeout`. FreeRDP waits `TcpAckTimeout`
    // (default 9 s) for the session host to finish activation, including
    // the Entra exchange (`connection.c` `rdp_client_wait_for_activation`);
    // measured on the owner's host it took up to 49.8 s (settings.rs).
    args.extend(spec.settings.freerdp_args());
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

pub fn classify_exit(code: Option<i32>, log_says_logon_failure: bool) -> SessionEnd {
    match code {
        None => SessionEnd::Crashed,
        _ if log_says_logon_failure => SessionEnd::LogonFailed,
        Some(0..=3 | 11 | 12) => SessionEnd::Closed,
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
    launch_with(&freerdp, &runtime_base()?, spec, events)
}

fn launch_with(
    freerdp: &Path,
    runtime: &Path,
    spec: &LaunchSpec,
    events: impl Fn(SessionEvent) + Send + Sync + 'static,
) -> Result<Session, LaunchError> {
    let dir = RuntimeDir::create(runtime)?;
    let rdp_path = write_connection_file(&dir, &spec.connection)?;
    let args = build_args(spec, &rdp_path)?;
    let (read_end, write_end) = pipe()?;
    let (terminal, output) = output_terminal().map_err(|e| io_err(&e))?;
    let mut child = spawn_freerdp(freerdp, &spec.profile, &read_end, output)?;
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
/// line-buffers a terminal). Through a pipe the
/// display-ready line, and with it the window refit, came only when FreeRDP
/// ended. FreeRDP checks for a terminal only on its input (password prompt,
/// `libfreerdp/utils/passphrase.c`), which stays `/dev/null`.
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
    let proxy = freerdp_proxy(|name| std::env::var(name).ok())?;
    for name in APP_PROXY_VARS.iter().chain(&["NO_PROXY", "no_proxy"]) {
        cmd.env_remove(name);
    }
    if let Some(p) = proxy.https_proxy {
        cmd.env("https_proxy", p);
    }
    if let Some(n) = proxy.no_proxy {
        cmd.env("no_proxy", n);
    }
    cmd.env("COSMIC_EXT_DESKFEED_SIGNIN_PROFILE", profile);
    // Native Wayland, not XWayland. SDL 3 picks XWayland when the
    // compositor lacks `wp_fifo_manager_v1` (SDL 3.4.18
    // `src/video/wayland/SDL_waylandvideo.c`), which COSMIC does: the
    // window then had a Latin-1-mangled title, no Wayland app ID (so the
    // refit and "open on a new workspace" couldn't find it) and X11's
    // handling of tiling. A list: X11 still works without Wayland
    // (`SDL_HINT_VIDEO_DRIVER`, SDL_hints.h). The user's own choice wins.
    cmd.env(
        "SDL_VIDEO_DRIVER",
        std::env::var_os("SDL_VIDEO_DRIVER").unwrap_or_else(|| "wayland,x11".into()),
    );
    // Connection steps ("STATE_A --> STATE_B"), so a failure shows where it
    // stopped. This logger prints no names, passwords or tokens (FreeRDP
    // 3.32.2 `libfreerdp/core/rdp.c`). An environment variable, because
    // `/log-filters:` comes too late for loggers that already exist
    // (`winpr/libwinpr/utils/wlog/wlog.c` caches the level on creation).
    // The display channel and SDL client loggers too (`WLOG_FILTER` is a
    // comma-separated list, `winpr/libwinpr/utils/wlog/wlog.c`).
    cmd.env(
        "WLOG_FILTER",
        format!("{STATE_LOGGER}:DEBUG,{DISPLAY_LOGGER}:DEBUG,{SDL_LOGGER}:DEBUG"),
    );
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
    spawn_retrying(&mut cmd).map_err(|e| io_err(&e))
}

/// `spawn`, retried briefly on "text file busy": the executable may still
/// be open for writing in a process another thread just forked (seen with
/// freshly written test scripts).
pub fn spawn_retrying(cmd: &mut Command) -> std::io::Result<Child> {
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
fn send_args(write_end: OwnedFd, args: &[String]) -> std::io::Result<()> {
    let mut w = fs::File::from(write_end);
    let mut payload = args.join("\n");
    payload.push_str("\n\n");
    let result = w.write_all(payload.as_bytes());
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
                for line in crate::bounded_lines::lines(BufReader::new(stream), MAX_OUTPUT_LINE)
                    .map(|l| l.text)
                {
                    // Our sign-in helper's status lines (FreeRDP passes its
                    // stderr through); they hold no codes or tokens.
                    if line.starts_with("cosmic-ext-deskfeed-signin: ") || is_state_change(&line) {
                        crate::app_log!("{line}");
                    }
                    if log.lock().expect("not poisoned").push(&line) {
                        report(SessionEvent::Connected);
                    }
                    if line.contains(DISPLAY_READY_MARKER) {
                        report(SessionEvent::DisplayReady);
                    }
                    if line.contains(WINDOW_RESIZED_MARKER) {
                        report(SessionEvent::WindowResized);
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
        let end = classify_exit(status.code(), log.logon_failure);
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

#[derive(Default)]
struct Log {
    lines: Vec<String>,
    connected: bool,
    logon_failure: bool,
}

impl Log {
    /// Keeps warnings, errors and connection steps (bounded). Returns `true` the first time
    /// the connected marker appears.
    fn push(&mut self, line: &str) -> bool {
        if line.contains("LOGON_FAILURE") {
            self.logon_failure = true;
        }
        if line.contains("[ERROR]") || line.contains("[WARN]") || is_state_change(line) {
            if self.lines.len() == MAX_LOG_LINES {
                self.lines.remove(0);
            }
            // Redacted before it is shortened, so a cut token can't slip
            // past the redaction (log.rs).
            self.lines
                .push(crate::log::redact(line).chars().take(300).collect());
        }
        if !self.connected && line.contains(CONNECTED_MARKER) {
            self.connected = true;
            return true;
        }
        false
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
    fn only_executable_files_count() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("avd-exec-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sdl-freerdp3");
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

    #[test]
    fn exit_codes() {
        assert_eq!(classify_exit(Some(0), false), SessionEnd::Closed);
        assert_eq!(classify_exit(Some(11), false), SessionEnd::Closed);
        assert_eq!(classify_exit(Some(134), false), SessionEnd::LogonFailed);
        assert_eq!(classify_exit(Some(131), true), SessionEnd::LogonFailed);
        assert_eq!(
            classify_exit(Some(135), false),
            SessionEnd::AccountLockedOut
        );
        assert_eq!(classify_exit(Some(149), false), SessionEnd::PasswordExpired);
        assert_eq!(classify_exit(Some(140), false), SessionEnd::DnsFailed);
        assert_eq!(
            classify_exit(Some(131), false),
            SessionEnd::Failed { code: 131 }
        );
        assert_eq!(classify_exit(None, false), SessionEnd::Crashed);
    }

    #[test]
    fn log_keeps_errors_and_detects_connection() {
        let mut log = Log::default();
        assert!(!log.push("[INFO][com.freerdp.core] - something"));
        assert!(log.push(
            "[INFO][com.freerdp.channels.drdynvc.client] - Loading Dynamic Virtual Channel rdpgfx"
        ));
        assert!(!log.push(
            "[INFO][com.freerdp.channels.drdynvc.client] - Loading Dynamic Virtual Channel rdpsnd"
        ));
        log.push("[ERROR][com.freerdp.core.rdstls] - RDSTLS_RESULT_LOGON_FAILURE [0x0000052E]");
        assert!(log.logon_failure);
        assert_eq!(log.lines.len(), 1);
        log.push("[DEBUG][com.freerdp.core.rdp] - [rdp_client_transition_to_state][0x1]: CONNECTION_STATE_NEGO --> CONNECTION_STATE_AAD");
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
        let fake = dir.join("sdl-freerdp3");
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
                 env | grep -q '^COSMIC_EXT_DESKFEED_SIGNIN_PROFILE=/tmp/p$' && echo PROFILE_OK >> \"$out\"\n\
                 env | grep -q '^WLOG_FILTER=com.freerdp.core.rdp:DEBUG,com.freerdp.client.sdl.disp:DEBUG,com.freerdp.client.SDL:DEBUG$' && echo FILTER_OK >> \"$out\"\n\
                 env | grep -q '^SDL_VIDEO_DRIVER=wayland,x11$' && echo WAYLAND_OK >> \"$out\"\n\
                 echo '[INFO][x] - Loading Dynamic Virtual Channel rdpgfx'\n\
                 echo '[DEBUG][com.freerdp.client.sdl.disp] - [DisplayControlCaps]: DisplayControlCapsPdu: setting the window as resizable'\n\
                 echo '[DEBUG][com.freerdp.client.SDL] - [winpr_LogOutputFunction]: [APP] SDL_EVENT_WINDOW_RESIZED: [3] 0x0-1524x1680'\n\
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
        let _session = launch_with(&fake, &runtime, &s, move |e| tx.send(e).unwrap()).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            SessionEvent::Connected
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            SessionEvent::DisplayReady
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            SessionEvent::WindowResized
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
        assert!(got.contains("WAYLAND_OK"), "{got}");
        assert!(!got.contains("DRIVES"));
        assert!(!got.contains("LEAKED_ENV"));
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
             { echo '[DEBUG][com.freerdp.client.sdl.disp] - DisplayControlCapsPdu: setting the window as resizable'; sleep 5; } | sed -n p\n",
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let s = spec(LogonSetting::Automatic, Some("pw"), false);
        let session =
            launch_with(&fake, &dir.join("run"), &s, move |e| tx.send(e).unwrap()).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            SessionEvent::DisplayReady
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
            let _session =
                launch_with(&fake, &dir.join("run"), &s, move |e| tx.send(e).unwrap()).unwrap();
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
        let session =
            launch_with(&fake, &dir.join("run"), &s, move |e| tx.send(e).unwrap()).unwrap();
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

    /// Real `sdl-freerdp3`: every property we keep, plus our arguments, must
    /// pass FreeRDP's argument parsing. Without `loadbalanceinfo` FreeRDP stops
    /// right after parsing, before any network access.
    #[test]
    #[ignore = "needs sdl-freerdp3"]
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
            settings: crate::settings::DEFAULTS,
            window_title: String::new(),
            app_id: String::new(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let freerdp = find_freerdp().expect("sdl-freerdp3 installed");
        let _session =
            launch_with(&freerdp, &dir.join("run"), &s, move |e| tx.send(e).unwrap()).unwrap();
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
}
