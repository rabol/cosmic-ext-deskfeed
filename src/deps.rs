// SPDX-License-Identifier: MIT

//! Runtime dependency check (ADR 0010): run at startup and on request.
//!
//! Each check tests the capability the app uses, not just that a file
//! exists: FreeRDP's version and Entra support, that the sign-in helper
//! starts (so WebKitGTK and GTK load), that a Secret Service answers.
//! Blocking; call from `spawn_blocking`.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::auth::signin;
use crate::sessions::launcher;

/// The FreeRDP release whose behaviour the app is built and tested against
/// (helper protocol, `/args-from`, ARM gateway; ADR 0004).
pub const MIN_FREERDP: (u32, u32, u32) = (3, 32, 2);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_OUTPUT: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dependency {
    FreeRdp,
    /// FreeRDP built with Entra (`WITH_AAD`) support.
    FreeRdpEntra,
    SignInHelper,
    SecretService,
    RuntimeDir,
    /// COSMIC's window management, for "open on a new workspace".
    CosmicWindows,
}

impl Dependency {
    /// Required ones block connecting or signing in; optional ones only
    /// remove a feature.
    pub fn required(self) -> bool {
        !matches!(self, Self::SecretService | Self::CosmicWindows)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Found; the text says what (for example the version).
    Ok(String),
    Missing,
    TooOld(String),
    /// Present but not usable; the text says why.
    Broken(String),
}

impl Status {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub dependency: Dependency,
    pub status: Status,
}

/// Runs every check.
pub fn check_all() -> Vec<Check> {
    let freerdp = launcher::find_freerdp();
    let (freerdp_status, entra_status) = match &freerdp {
        None => (Status::Missing, Status::Missing),
        Some(bin) => (check_freerdp_version(bin), check_freerdp_entra(bin)),
    };
    vec![
        Check {
            dependency: Dependency::FreeRdp,
            status: freerdp_status,
        },
        Check {
            dependency: Dependency::FreeRdpEntra,
            status: entra_status,
        },
        Check {
            dependency: Dependency::SignInHelper,
            status: check_helper(),
        },
        Check {
            dependency: Dependency::SecretService,
            status: match crate::secrets::check() {
                Ok(()) => Status::Ok(String::new()),
                Err(e) => Status::Broken(e.to_string()),
            },
        },
        Check {
            dependency: Dependency::RuntimeDir,
            status: check_runtime_dir(),
        },
        Check {
            dependency: Dependency::CosmicWindows,
            status: match crate::workspace::check() {
                Ok(()) => Status::Ok(String::new()),
                Err(e) => Status::Broken(e),
            },
        },
    ]
}

/// True when every required dependency is usable.
pub fn required_ok(checks: &[Check]) -> bool {
    checks
        .iter()
        .all(|c| !c.dependency.required() || c.status.is_ok())
}

fn check_freerdp_version(bin: &Path) -> Status {
    match run(bin, &["/version"]) {
        Err(e) => Status::Broken(e),
        Ok(out) => match parse_version(&out) {
            None => Status::Broken("version not recognised".into()),
            Some(v) if v < MIN_FREERDP => Status::TooOld(format!("{}.{}.{}", v.0, v.1, v.2)),
            Some(v) => Status::Ok(format!("{}.{}.{}", v.0, v.1, v.2)),
        },
    }
}

fn check_freerdp_entra(bin: &Path) -> Status {
    match run(bin, &["/buildconfig"]) {
        Err(e) => Status::Broken(e),
        Ok(out) if has_build_flag(&out, "WITH_AAD") => Status::Ok(String::new()),
        Ok(_) => Status::Broken("built without WITH_AAD".into()),
    }
}

/// `--check` makes the helper exit right after start, so a missing
/// WebKitGTK or GTK library shows up as a failed start.
fn check_helper() -> Status {
    let Some(helper) = signin::find_helper() else {
        return Status::Missing;
    };
    match run(&helper, &["--check"]) {
        Ok(out) if out.trim() == "ok" => Status::Ok(String::new()),
        Ok(_) => Status::Broken("unexpected answer".into()),
        Err(e) => Status::Broken(e),
    }
}

fn check_runtime_dir() -> Status {
    match std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from) {
        Some(p) if p.is_absolute() && p.is_dir() => Status::Ok(String::new()),
        Some(_) => Status::Broken("not a directory".into()),
        None => Status::Missing,
    }
}

/// `This is FreeRDP version 3.32.2 (97eff6ddfb)` → (3, 32, 2).
fn parse_version(out: &str) -> Option<(u32, u32, u32)> {
    let rest = out.split("FreeRDP version ").nth(1)?;
    let word = rest.split_whitespace().next()?;
    let mut parts = word.split(['.', '-']).map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// `WITH_AAD=ON` (or `=1`) in `/buildconfig` output.
fn has_build_flag(out: &str, flag: &str) -> bool {
    out.split_whitespace().any(|w| {
        w.strip_prefix(flag)
            .and_then(|r| r.strip_prefix('='))
            .is_some_and(|v| matches!(v, "ON" | "1" | "TRUE" | "YES"))
    })
}

/// Runs a short command with no input and a bounded time and output.
fn run(bin: &Path, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.kind().to_string())?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > COMMAND_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("no answer".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.kind().to_string()),
        }
    };
    let mut out = String::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(MAX_OUTPUT).read_to_string(&mut out);
    }
    if status.success() {
        Ok(out)
    } else {
        Err(format!("exit status {}", status.code().unwrap_or(-1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_freerdp_versions() {
        assert_eq!(
            parse_version("This is FreeRDP version 3.32.2 (97eff6ddfb)\n"),
            Some((3, 32, 2))
        );
        assert_eq!(
            parse_version("This is FreeRDP version 3.4.0-dev (x)"),
            Some((3, 4, 0))
        );
        assert_eq!(parse_version("something else"), None);
        assert!((3, 31, 9) < MIN_FREERDP && (3, 32, 2) >= MIN_FREERDP && (4, 0, 0) > MIN_FREERDP);
    }

    #[test]
    fn reads_build_flags() {
        let cfg = "Build configuration: WITH_AAD=ON WITH_AADX=OFF WITH_PULSE=ON";
        assert!(has_build_flag(cfg, "WITH_AAD"));
        assert!(!has_build_flag("WITH_AAD=OFF", "WITH_AAD"));
        assert!(!has_build_flag("WITH_AADX=ON", "WITH_AAD"));
        assert!(has_build_flag("WITH_AAD=1", "WITH_AAD"));
    }

    /// On a complete desktop: `cargo test -- --ignored this_machine`.
    #[test]
    #[ignore = "depends on the installed system"]
    fn this_machine_has_everything() {
        let checks = check_all();
        for c in &checks {
            println!("{:?}: {:?}", c.dependency, c.status);
        }
        // The test binary lives in `target/*/deps`, away from the helper.
        assert!(
            checks
                .iter()
                .filter(|c| c.dependency != Dependency::SignInHelper)
                .all(|c| c.status.is_ok())
        );
    }

    #[test]
    fn bounded_commands() {
        let out = run(Path::new("/bin/sh"), &["-c", "echo hi"]).unwrap();
        assert_eq!(out.trim(), "hi");
        assert!(run(Path::new("/bin/sh"), &["-c", "exit 3"]).is_err());
        assert!(run(Path::new("/nonexistent/x"), &[]).is_err());
    }
}
