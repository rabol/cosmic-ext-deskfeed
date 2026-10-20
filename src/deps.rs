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
    pub const ALL: [Self; 6] = [
        Self::FreeRdp,
        Self::FreeRdpEntra,
        Self::SignInHelper,
        Self::SecretService,
        Self::RuntimeDir,
        Self::CosmicWindows,
    ];

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

/// Every dependency marked unusable for `why`: what the app shows when
/// the checks themselves couldn't run.
pub fn not_checked(why: &str) -> Vec<Check> {
    Dependency::ALL
        .into_iter()
        .map(|dependency| Check {
            dependency,
            status: Status::Broken(why.to_owned()),
        })
        .collect()
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

/// True when every required dependency was checked and is usable. A
/// dependency missing from `checks` counts as not usable: an empty list
/// (the checks didn't run) must never read as "all present".
pub fn required_ok(checks: &[Check]) -> bool {
    Dependency::ALL
        .into_iter()
        .filter(|d| d.required())
        .all(|d| checks.iter().any(|c| c.dependency == d && c.status.is_ok()))
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
    // Read while it runs: output larger than the pipe would otherwise
    // block it until the time limit. Past the limit it is read and
    // dropped, so the command never waits on us.
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = (&mut stdout).take(MAX_OUTPUT).read_to_end(&mut out);
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        out
    });
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
    // The pipe closes when the command (and any child holding it) ends.
    let out = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
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

    #[test]
    fn unchecked_dependencies_are_never_ok() {
        // An empty list once read as "all present": a crashed check would
        // have let the app go on as if FreeRDP were there.
        assert!(!required_ok(&[]));
        assert!(!required_ok(&not_checked("the check stopped")));
        let ok = |dependency| Check {
            dependency,
            status: Status::Ok(String::new()),
        };
        let all: Vec<Check> = Dependency::ALL.into_iter().map(ok).collect();
        assert!(required_ok(&all));
        // Optional ones may be missing; a required one may not.
        let required: Vec<Check> = Dependency::ALL
            .into_iter()
            .filter(|d| d.required())
            .map(ok)
            .collect();
        assert!(required_ok(&required));
        assert!(!required_ok(&required[1..]));
        // Every dependency is listed once, in check_all's order.
        assert_eq!(not_checked("x").len(), Dependency::ALL.len());
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

    #[test]
    fn long_output_doesnt_stall_the_check() {
        // More than a pipe holds (64 KiB): read while it runs, or the
        // command blocks writing and the check waits for its time limit.
        let started = Instant::now();
        let out = run(
            Path::new("/bin/sh"),
            &["-c", "head -c 204800 /dev/zero | tr '\\0' a"],
        )
        .unwrap();
        assert_eq!(out.len(), 204_800);
        // Beyond the limit: cut, still prompt.
        let out = run(
            Path::new("/bin/sh"),
            &["-c", "head -c 400000 /dev/zero | tr '\\0' a"],
        )
        .unwrap();
        assert_eq!(out.len() as u64, MAX_OUTPUT);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }
}
