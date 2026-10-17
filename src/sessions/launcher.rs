// SPDX-License-Identifier: MIT

//! Starts and supervises FreeRDP for one session (spec §8).
//!
//! - All arguments, including the password, go through a private pipe
//!   (`/args-from:fd:3`), so nothing sensitive is on the command line or in
//!   the environment.
//! - The filtered connection file is written to a private runtime folder
//!   (`$XDG_RUNTIME_DIR/cosmic-ext-avd/<random>`, mode 0700, file 0600) and
//!   removed when the session ends.
//! - FreeRDP gets an environment allowlist, runs in its own process group,
//!   and uses our sign-in helper for Entra token requests.
//! - "Connected" comes from FreeRDP's own log, not from process start.

use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
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
const ACTIVATION_TIMEOUT_MS: u32 = 120_000;

const ENV_ALLOWLIST: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "DBUS_SESSION_BUS_ADDRESS",
    "PULSE_SERVER",
    "HOME",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_MESSAGES",
    "PATH",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
];

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    FreeRdpMissing,
    NoRuntimeDir,
    /// Password contains a line break or NUL, which FreeRDP can't take.
    BadPassword,
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
    Ended { end: SessionEnd, log: Vec<String> },
}

/// Finds a FreeRDP client in `PATH`.
pub fn find_freerdp() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    FREERDP_BINARIES.iter().find_map(|name| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    })
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
    let entra = spec.connection.entra_sso() || spec.logon == LogonSetting::EntraSignIn;
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
        // FreeRDP waits `TcpAckTimeout` (default 9000 ms) for the session
        // host to finish activation, including the Entra exchange
        // (`connection.c` `rdp_client_wait_for_activation`). Measured on the
        // owner's host (2026-10-17), the Entra exchange alone took 0.45 s,
        // 24.3 s and 49.8 s, so 9 s fails and 60 s is too close. `/timeout`
        // is FreeRDP's documented setting for this ("use if you encounter
        // timeout failures"); its maximum is 600 s.
        format!("/timeout:{ACTIVATION_TIMEOUT_MS}"),
    ]);
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
        let root = base.join("cosmic-ext-avd");
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
        let root = PathBuf::from(base).join("cosmic-ext-avd");
        if let Ok(entries) = fs::read_dir(&root) {
            for e in entries.flatten() {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

/// A running FreeRDP session.
pub struct Session {
    child: Arc<Mutex<Child>>,
    pgid: i32,
}

impl Session {
    /// Asks FreeRDP to end, then forces it after a grace period. Only this
    /// session's process group is signalled.
    pub fn disconnect(&self) {
        let pgid = self.pgid;
        let child = self.child.clone();
        std::thread::spawn(move || {
            // SAFETY: plain syscall; negative pid = our own child's group.
            unsafe { libc::kill(-pgid, libc::SIGTERM) };
            for _ in 0..50 {
                std::thread::sleep(Duration::from_millis(100));
                if child
                    .lock()
                    .expect("not poisoned")
                    .try_wait()
                    .ok()
                    .flatten()
                    .is_some()
                {
                    return;
                }
            }
            // SAFETY: as above.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        });
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
    let mut child = spawn_freerdp(freerdp, &spec.profile, &read_end)?;
    drop(read_end);
    if send_args(write_end, &args).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(LaunchError::Io("argument pipe".into()));
    }
    supervise(child, dir, Arc::new(events))
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

fn spawn_freerdp(
    freerdp: &Path,
    profile: &Path,
    args_pipe: &OwnedFd,
) -> Result<Child, LaunchError> {
    let read_fd = args_pipe.as_raw_fd();
    let mut cmd = Command::new(freerdp);
    cmd.arg("/args-from:fd:3")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    for key in ENV_ALLOWLIST {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd.env("COSMIC_EXT_AVD_SIGNIN_PROFILE", profile);
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
    dir: RuntimeDir,
    events: Arc<dyn Fn(SessionEvent) + Send + Sync>,
) -> Result<Session, LaunchError> {
    let pgid = i32::try_from(child.id()).map_err(|_| LaunchError::Io("pid".into()))?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let child = Arc::new(Mutex::new(child));
    let log = Arc::new(Mutex::new(Log::default()));

    let readers: Vec<_> = [Box::new(stdout) as Box<dyn Read + Send>, Box::new(stderr)]
        .into_iter()
        .map(|stream| {
            let log = log.clone();
            let events = events.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    // Our sign-in helper's status lines (FreeRDP passes its
                    // stderr through); they hold no codes or tokens.
                    if line.starts_with("cosmic-ext-avd-signin: ") || is_state_change(&line) {
                        eprintln!("{line}");
                    }
                    if log.lock().expect("not poisoned").push(&line) {
                        events(SessionEvent::Connected);
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
        events(SessionEvent::Ended {
            end,
            log: log.lines,
        });
    });

    Ok(Session { child, pgid })
}

/// A FreeRDP connection step ("STATE_A --> STATE_B"); no secrets.
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
            self.lines.push(line.chars().take(300).collect());
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
            "full address:s:h\r\nenablerdsaadauth:i:1\r\n"
        } else {
            "full address:s:h\r\n"
        };
        LaunchSpec {
            connection: ConnectionFile::parse(text.as_bytes()).unwrap(),
            username: Some("anna@example.com".into()),
            password: password.map(|p| Secret::new(p.into())),
            logon,
            helper: PathBuf::from("/usr/libexec/cosmic-ext-avd/cosmic-ext-avd-signin"),
            profile: PathBuf::from("/tmp/p"),
            gateway_token: None,
        }
    }

    fn args(s: &LaunchSpec) -> Vec<String> {
        build_args(
            s,
            Path::new("/run/user/1000/cosmic-ext-avd/x/connection.rdp"),
        )
        .unwrap()
    }

    #[test]
    fn password_host_automatic() {
        let a = args(&spec(LogonSetting::Automatic, Some("p@ss w0rd,;"), false));
        assert_eq!(a[0], "/run/user/1000/cosmic-ext-avd/x/connection.rdp");
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
        for s in [
            spec(LogonSetting::EntraSignIn, Some("pw"), false),
            spec(LogonSetting::Automatic, Some("pw"), true),
        ] {
            let a = args(&s);
            assert!(a.contains(&"/sec:aad".into()));
            assert!(
                !a.iter()
                    .any(|x| x.starts_with("/p:") || x.starts_with("/d:"))
            );
        }
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
                 env | grep -q '^COSMIC_EXT_AVD_SIGNIN_PROFILE=/tmp/p$' && echo PROFILE_OK >> \"$out\"\n\
                 echo '[INFO][x] - Loading Dynamic Virtual Channel rdpgfx'\n\
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
        assert!(!got.contains("DRIVES"));
        assert!(!got.contains("LEAKED_ENV"));
        // The connection file and its folder are gone.
        assert_eq!(
            fs::read_dir(runtime.join("cosmic-ext-avd"))
                .unwrap()
                .count(),
            0
        );
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
                SessionEvent::Connected => panic!("unexpected Connected"),
            }
        }
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
            "authentication level:i:1",
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
            "gatewayhostname:s:127.0.0.1:1",
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
