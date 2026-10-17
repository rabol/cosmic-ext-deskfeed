// SPDX-License-Identifier: MIT

//! The app's log (spec §12): one private, bounded, rotating file, so
//! problems can be looked at when the app was started from the launcher.
//!
//! - `$XDG_STATE_HOME/cosmic-ext-deskfeed/app.log` (usually
//!   `~/.local/state/cosmic-ext-deskfeed/`), folder 0700, file 0600.
//! - Rotated at 1 MiB; one older file (`app.log.1`) is kept.
//! - Lines also go to stderr, for runs from a terminal.
//! - Callers never log secrets; as a last guard, anything shaped like a JWT
//!   (access, refresh or ID token) is replaced before writing.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const FILE: &str = "app.log";
const OLD: &str = "app.log.1";
const MAX_SIZE: u64 = 1024 * 1024;
const MAX_LINE: usize = 2000;

/// Writes one log line: `crate::app_log!("format {}", args)`.
#[macro_export]
macro_rules! app_log {
    ($($arg:tt)*) => {
        $crate::log::write(&format!($($arg)*))
    };
}

static OUT: Mutex<Option<File>> = Mutex::new(None);

/// The folder holding the log files.
pub fn dir() -> Option<PathBuf> {
    Some(dirs::state_dir()?.join("cosmic-ext-deskfeed"))
}

pub fn write(message: &str) {
    let line = format!("{} {}", timestamp(SystemTime::now()), redact(message));
    eprintln!("{line}");
    // Tests never write to the user's real log file.
    if cfg!(test) {
        return;
    }
    let Ok(mut out) = OUT.lock() else { return };
    if out.is_none() {
        *out = dir().and_then(|d| open_in(&d));
    }
    let Some(file) = out.as_mut() else { return };
    let _ = writeln!(file, "{line}");
    if file.metadata().is_ok_and(|m| m.len() > MAX_SIZE) {
        *out = None;
        rotate();
        *out = dir().and_then(|d| open_in(&d));
    }
}

/// Opens (creating if needed) the log file in `dir`: folder 0700, file
/// 0600.
fn open_in(dir: &Path) -> Option<File> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .ok()?;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(dir.join(FILE))
        .ok()
}

fn rotate() {
    if let Some(dir) = dir() {
        let _ = fs::rename(dir.join(FILE), dir.join(OLD));
    }
}

/// Bounded, one line, and no token-shaped words.
fn redact(message: &str) -> String {
    let one_line: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_LINE)
        .collect();
    one_line
        .split(' ')
        .map(|word| {
            if looks_like_jwt(word) {
                "<redacted>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `eyJ…` (base64 of `{"`), two dots, long: the shape of a JWT.
fn looks_like_jwt(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let token = w.find("eyJ").map_or("", |i| &w[i..]);
    token.len() > 40 && token.matches('.').count() >= 2
}

/// `2026-10-17T18:18:10Z` (UTC).
fn timestamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_utc_iso() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // 2026-10-17T18:18:10Z, as Unix seconds (what the code converts).
        #[allow(clippy::duration_suboptimal_units)]
        let t = UNIX_EPOCH + Duration::from_secs(1_792_261_090);
        assert_eq!(timestamp(t), "2026-10-17T18:18:10Z");
        // Leap day.
        #[allow(clippy::duration_suboptimal_units)]
        let t = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(timestamp(t), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn writes_a_private_file() {
        let base = std::env::temp_dir().join(format!("deskfeed-log-{}", std::process::id()));
        let dir = base.join("cosmic-ext-deskfeed");
        let mut file = open_in(&dir).unwrap();
        writeln!(file, "hello from the test").unwrap();
        let text = fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(text, "hello from the test\n");
        let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.join(FILE)), 0o600);
        assert_eq!(mode(dir.clone()), 0o700);
        // Appends, never truncates.
        writeln!(open_in(&dir).unwrap(), "second").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(FILE)).unwrap(),
            "hello from the test\nsecond\n"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn tokens_and_line_breaks_are_removed() {
        let jwt = concat!(
            "eyJhbGciOiJSUzI1NiJ9",
            ".",
            "eyJzdWIiOiIxMjM0NTY3ODkwIn0",
            ".",
            "c2lnbmF0dXJlLXNpZ25hdHVyZQ"
        );
        let out = redact(&format!("bearer:{jwt} then\nnext"));
        assert!(!out.contains("eyJ"));
        assert!(out.contains("<redacted>") && out.contains("then next"));
        // Ordinary text stays.
        assert_eq!(redact("saved sign-in: 1 found"), "saved sign-in: 1 found");
        assert_eq!(redact(&"x".repeat(5000)).len(), MAX_LINE);
    }
}
