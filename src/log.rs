// SPDX-License-Identifier: MIT

//! The app's log (spec §12): one private, bounded, rotating file, so
//! problems can be looked at when the app was started from the launcher.
//!
//! - `$XDG_STATE_HOME/cosmic-ext-deskfeed/app.log` (usually
//!   `~/.local/state/cosmic-ext-deskfeed/`), folder 0700, file 0600.
//! - Rotated at a size set in the app's settings (default 1 MiB,
//!   [`MAX_SIZE_MIB`]); one older file (`app.log.1`) is kept.
//! - Lines also go to stderr, for runs from a terminal.
//! - Callers never log secrets or tenant data. As a last guard every line
//!   is redacted before it is written (`redact`): tokens, IDs, user names,
//!   IP addresses and host names outside Microsoft's service domains.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

const FILE: &str = "app.log";
const OLD: &str = "app.log.1";
/// Smallest, largest and default rotation size, in MiB.
pub const MAX_SIZE_MIB: (u32, u32, u32) = (1, 100, 1);
static MAX_SIZE: AtomicU64 = AtomicU64::new(1024 * 1024);
const MAX_LINE: usize = 2000;

/// Sets the rotation size (App settings → Diagnostics), clamped to
/// [`MAX_SIZE_MIB`].
pub fn set_max_size_mib(mib: u32) {
    let (min, max, _) = MAX_SIZE_MIB;
    MAX_SIZE.store(
        u64::from(mib.clamp(min, max)) * 1024 * 1024,
        Ordering::Relaxed,
    );
}

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
    if file
        .metadata()
        .is_ok_and(|m| m.len() > MAX_SIZE.load(Ordering::Relaxed))
    {
        *out = None;
        if let Some(d) = dir() {
            rotate_in(&d);
        }
        *out = dir().and_then(|d| open_in(&d));
    }
}

/// Opens (creating if needed) the log file in `dir`: folder 0700, file
/// 0600.
fn open_in(dir: &Path) -> Option<File> {
    crate::files::private_dir(dir).ok()?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(dir.join(FILE))
        .ok()
}

/// The current file becomes the older one (replacing it); the next
/// write starts a new file.
fn rotate_in(dir: &Path) {
    let _ = fs::rename(dir.join(FILE), dir.join(OLD));
}

/// Bounded, one line, with secrets and tenant data replaced: JWTs and
/// other long tokens, GUIDs (tenant, object and other IDs), user names
/// (`user@domain`, `DOMAIN\user`), IP addresses, and host names except
/// Microsoft's service domains. Also for text from other programs
/// (FreeRDP's log lines); redact before shortening, so a cut can't hide a
/// token from it.
pub fn redact(message: &str) -> String {
    let one_line: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_LINE)
        .collect();
    // IPv6 addresses first: `:` separates words below.
    let one_line = redact_ipv6(&one_line);
    let mut out = String::with_capacity(one_line.len());
    let mut word = String::new();
    for c in one_line.chars() {
        if is_word_char(c) {
            word.push(c);
        } else {
            out.push_str(replace_word(&word).unwrap_or(&word));
            word.clear();
            out.push(c);
        }
    }
    out.push_str(replace_word(&word).unwrap_or(&word));
    out
}

/// Characters that make up tokens, names and addresses; anything else
/// (spaces, `:/=,;"'()[]<>`, …) separates them.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | '\\' | '%' | '+' | '~')
}

fn replace_word(word: &str) -> Option<&'static str> {
    if word.is_empty() {
        return None;
    }
    if looks_like_jwt(word) {
        return Some("<redacted>");
    }
    if crate::auth::id_token::is_guid(word) {
        return Some("<id>");
    }
    // `DOMAIN\\user`, `user@domain.example`, and `user@DOMAIN`.
    if word.contains('\\')
        || word
            .split_once('@')
            .is_some_and(|(user, domain)| !user.is_empty() && !domain.is_empty())
    {
        return Some("<user>");
    }
    if is_ipv4(word) {
        return Some("<ip>");
    }
    match host_kind(word) {
        Some(Host::Private) => return Some("<host>"),
        Some(Host::Public) => return None,
        None => {}
    }
    // Opaque tokens, codes and cookie values: long, letters and digits.
    if word.len() >= 32
        && word.chars().any(|c| c.is_ascii_digit())
        && word.chars().any(|c| c.is_ascii_alphabetic())
    {
        return Some("<redacted>");
    }
    None
}

/// Replaces IPv6 addresses (also with a `%zone`) by `<ip>`: runs of hex
/// digits, `:`, `.` and `%zone` that parse as one. Timestamps like
/// `12:16:11:369` don't parse, so they stay.
fn redact_ipv6(line: &str) -> String {
    let is_part = |c: char| c.is_ascii_hexdigit() || matches!(c, ':' | '.');
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find(is_part) {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(|c: char| !is_part(c)).unwrap_or(rest.len());
        let candidate = &rest[..end];
        // A `%zone` after the address belongs to it.
        let zone_end = rest[end..].strip_prefix('%').map_or(end, |z| {
            end + 1
                + z.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(z.len())
        });
        // `addr:port` without brackets can't be told apart; try the whole
        // run, then without a trailing `:port`-like part.
        let whole = candidate.trim_end_matches('.');
        // Where the address ends: after its `%zone`, else before any
        // trailing dots (a sentence's period stays).
        let next = if zone_end > end {
            zone_end
        } else {
            whole.len()
        };
        // A whole word only (`freerdp::core` holds `::c`, which parses),
        // with a digit or letter in it (`::` alone isn't an address here).
        let word_char = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let bounded =
            !word_char(out.chars().next_back()) && !word_char(rest[next..].chars().next());
        let parsed = bounded
            && whole.chars().any(|c| c.is_ascii_hexdigit())
            && whole.matches(':').count() >= 2
            && whole.parse::<std::net::Ipv6Addr>().is_ok();
        if parsed {
            out.push_str("<ip>");
            rest = &rest[next..];
        } else {
            out.push_str(candidate);
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out
}

fn is_ipv4(word: &str) -> bool {
    let parts: Vec<&str> = word.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok())
}

/// Microsoft's service domains: not tenant data, and useful to see.
const PUBLIC_DOMAINS: &[&str] = &[
    "microsoft.com",
    "microsoftonline.com",
    "windows.net",
    "azure.com",
    "azure.net",
    "msftauth.net",
    "msauth.net",
];

/// Dotted names that are not host names: logger tags, D-Bus and app IDs,
/// and file names.
const NOT_HOSTS: &[&str] = &["com.freerdp.", "org.freedesktop.", "io.github."];
const FILE_ENDINGS: &[&str] = &[
    "c", "cpp", "h", "hpp", "rs", "so", "dll", "exe", "json", "xml", "rdp", "log", "txt", "md",
    "png", "html", "js", "sh", "py", "toml", "ftl",
];

enum Host {
    /// One of Microsoft's service domains.
    Public,
    Private,
}

/// Whether `word` is a DNS name (two or more labels, alphabetic last
/// label), and whose.
fn host_kind(word: &str) -> Option<Host> {
    let name = word.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = name.split('.').collect();
    let last = labels.last().copied().unwrap_or_default();
    let dns = labels.len() >= 2
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && last.len() >= 2
        && last.chars().all(|c| c.is_ascii_alphabetic());
    let public = PUBLIC_DOMAINS
        .iter()
        .any(|d| name == *d || name.ends_with(&format!(".{d}")));
    if !dns || NOT_HOSTS.iter().any(|p| name.starts_with(p)) || FILE_ENDINGS.contains(&last) {
        return None;
    }
    Some(if public { Host::Public } else { Host::Private })
}

/// `eyJ…` (base64 of `{"`), two dots, long: the shape of a JWT.
fn looks_like_jwt(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let token = w.find("eyJ").map_or("", |i| &w[i..]);
    token.len() > 40 && token.matches('.').count() >= 2
}

/// `2026-10-17T18:18:10Z` (UTC, whole seconds).
fn timestamp(t: SystemTime) -> String {
    jiff::Timestamp::try_from(t)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, UNIX_EPOCH};

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
        // Whole seconds only.
        assert_eq!(
            timestamp(t + Duration::from_millis(999)),
            "2000-02-29T00:00:00Z"
        );
    }

    #[test]
    fn writes_a_private_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cosmic-ext-deskfeed");
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
        // Rotation: one older file is kept, a new one starts private.
        rotate_in(&dir);
        writeln!(open_in(&dir).unwrap(), "after").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(OLD)).unwrap(),
            "hello from the test\nsecond\n"
        );
        assert_eq!(fs::read_to_string(dir.join(FILE)).unwrap(), "after\n");
        assert_eq!(mode(dir.join(FILE)), 0o600);
        rotate_in(&dir);
        assert_eq!(
            fs::read_to_string(dir.join(OLD)).unwrap(),
            "after\n",
            "replaced"
        );
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

    #[test]
    fn tenant_data_and_opaque_secrets_are_removed() {
        let cases = [
            (
                "tenant 11111111-2222-3333-4444-555555555555 ok",
                "tenant <id> ok",
            ),
            ("user anna@contoso.com signed in", "user <user> signed in"),
            (r"logon CONTOSO\anna failed", "logon <user> failed"),
            (
                "connecting to avd-host-0.contoso.local:3389",
                "connecting to <host>:3389",
            ),
            ("peer 10.1.2.3:443", "peer <ip>:443"),
            (
                "refused Feed URL (NotAllowed): https://evil.example:443/api/x",
                "refused Feed URL (NotAllowed): https://<host>:443/api/x",
            ),
            (
                "cookie=0.AXwAq3bF9hX2kUe7Zr1pN8mQ4sT6vW8yZ0aB2cD4eF6gH8iJ",
                "cookie=<redacted>",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(redact(input), want, "{input}");
        }
    }

    #[test]
    fn useful_diagnostics_stay() {
        for keep in [
            "[ERROR][com.freerdp.core.connection] - [rdp_client_wait_for_activation]: \
             ERRCONNECT_ACTIVATION_TIMEOUT [0x0002001C]",
            "gateway afdfp-rdgateway-r1.wvd.microsoft.com:443",
            "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            "[sdl_disp.cpp:437] FreeRDP 3.32.2",
            "STATE_NEGO --> STATE_AAD",
        ] {
            assert_eq!(redact(keep), keep);
        }
    }

    /// Lines shaped like real FreeRDP and app log lines, with synthetic
    /// sensitive data: what must go, and what must stay
    /// readable.
    #[test]
    fn redacts_realistic_lines() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "[ERROR][com.freerdp.core.transport] - [freerdp_tcp_connect]: failed to connect to fe80::1%eth0:3389",
                &["fe80", "eth0"],
                &["com.freerdp.core.transport", "freerdp_tcp_connect", "<ip>"],
            ),
            (
                "connecting to [2001:db8:85a3::8a2e:370:7334]:443 via proxy",
                &["2001:db8", "7334"],
                &["<ip>", ":443"],
            ),
            (
                "[12:16:11:369] [1:2] [INFO][com.freerdp.client.common] - Logon Info V2 [CORP\\anna [3]]",
                &["anna", "CORP"],
                &["12:16:11:369", "<user>"],
            ),
            ("signed in as anna@CORP", &["anna", "CORP"], &["<user>"]),
            (
                "user anna@contoso.example",
                &["anna", "contoso"],
                &["<user>"],
            ),
            (
                "gateway rdgateway-c101-weu-r1.wvd.microsoft.com, host desk-7.corp.example.internal",
                &["corp.example", "desk-7"],
                &["rdgateway-c101-weu-r1.wvd.microsoft.com", "<host>"],
            ),
            (
                "tenant 00000000-0000-0000-0000-0000000000aa refused: AADSTS50076",
                &["0000000000aa"],
                &["<id>", "AADSTS50076"],
            ),
            (
                "address 10.17.8.107 and the app's app::model module",
                &["10.17.8.107"],
                &["<ip>", "app::model"],
            ),
        ];
        for (line, gone, kept) in cases {
            let out = redact(line);
            for g in *gone {
                assert!(!out.contains(g), "{g:?} left in {out:?}");
            }
            for k in *kept {
                assert!(out.contains(k), "{k:?} lost from {out:?}");
            }
        }
        assert_eq!(redact("unreachable: fe80::1."), "unreachable: <ip>.");
        // Rust paths and timestamps are never addresses.
        assert_eq!(
            redact("freerdp::core and 10:20:30:400"),
            "freerdp::core and 10:20:30:400"
        );
    }

    #[test]
    fn random_lines_never_break_the_redactor() {
        let samples: [&[u8]; 3] = [
            b"[ERROR][com.freerdp.core] - failed to connect to fe80::1%eth0:3389 as CORP\\anna",
            b"anna@contoso.example 10.0.0.1 [2001:db8::1]:443 00000000-0000-0000-0000-000000000000",
            b"bearer:eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.c2lnbmF0dXJl",
        ];
        crate::test_util::random_inputs("redact", &samples, |input| {
            let out = redact(&String::from_utf8_lossy(input));
            assert!(!out.contains('\n'));
            // Replacements are short; the cut happens before them.
            assert!(out.chars().count() <= MAX_LINE * 2);
        });
    }
}
