// SPDX-License-Identifier: MIT

//! Starting programs: the environment they get, the argument pipe, the
//! spawner thread, and supervising FreeRDP while it runs.

// The launcher's parts share its imports and items (mod.rs).
#[allow(clippy::wildcard_imports)]
use super::*;

/// Longest FreeRDP output line looked at; the rest of a line is skipped.
pub(super) const MAX_OUTPUT_LINE: usize = 4096;

/// Environment every child process (FreeRDP, the sign-in helper) gets;
/// everything else is dropped. See [`pass_environment`].
pub(super) const BASE_ENV: &[&str] = &[
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

/// Copies the allowed variables ([`BASE_ENV`] plus `extra`) from the app's
/// environment into `cmd`, which should start with `env_clear()`.
pub fn pass_environment(cmd: &mut Command, extra: &[&str]) {
    for key in BASE_ENV.iter().chain(extra) {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
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
pub(super) fn output_terminal() -> std::io::Result<(fs::File, OwnedFd)> {
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

pub(super) fn spawn_freerdp(
    freerdp: &Path,
    profile: &Path,
    args_pipe: &OwnedFd,
    output: OwnedFd,
    proxy: &FreeRdpProxy,
    proxy_file: Option<&Path>,
    account: Option<&HelperAccount>,
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
    // Identifiers from the verified ID token; plain text only, since a
    // NUL would stop the launch (the helper checks the format again).
    let plain = |s: &str| !s.is_empty() && !s.chars().any(char::is_control);
    // Entra IDs are GUIDs (checked in id_token.rs); no dot, which separates them here.
    let id = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if let Some(a) = account {
        if id(&a.object_id) && id(&a.tenant_id) {
            cmd.env(ACCOUNT_ENV, format!("{}.{}", a.object_id, a.tenant_id));
        }
        if let Some(name) = a.username.as_deref().filter(|n| plain(n)) {
            cmd.env(LOGIN_HINT_ENV, name);
        }
    }
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
pub(super) type SpawnRequest = (Command, std::sync::mpsc::Sender<std::io::Result<Child>>);

/// Starts `cmd` so that it ends when Deskfeed does, even after a crash or
/// a kill (spec §6, §8: no orphaned sessions). The kernel sends it SIGKILL
/// when its parent goes (`PR_SET_PDEATHSIG`): uncatchable, because after a
/// crash nothing is left to escalate a SIGTERM that FreeRDP ignored
/// (ADR 0022 addendum). Normal quitting stays graceful
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
pub(super) fn spawn_retrying(cmd: &mut Command) -> std::io::Result<Child> {
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
/// Never blocks: the whole payload
/// must fit in the pipe's buffer, and the write end is non-blocking, so a
/// FreeRDP that never reads them can't hold up launching or quitting. On
/// an error the caller stops FreeRDP.
pub(super) fn send_args(write_end: OwnedFd, args: &[String]) -> std::io::Result<()> {
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
    if fits && nonblocking {
        w.write_all(payload.as_bytes())
    } else {
        Err(std::io::Error::other("arguments don't fit the pipe"))
    }
}

/// Reads FreeRDP's log, waits for it to exit, removes the runtime folder
/// and reports the end.
pub(super) fn supervise(
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
            // Only before connecting: a later end is about that session.
            _ if log.wrong_account && !log.connected => SessionEnd::WrongAccount,
            _ if log.proxy_auth => SessionEnd::ProxyAuthRefused,
            _ if log.replaced && status.code().is_some() => SessionEnd::Replaced,
            _ if log.activation_timeout && !log.connected && !log.logon_failure => {
                SessionEnd::HostTimedOut
            }
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

pub(super) fn pipe() -> Result<(OwnedFd, OwnedFd), LaunchError> {
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
