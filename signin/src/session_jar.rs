// SPDX-License-Identifier: MIT

//! Shares the sign-in page's session cookies between helper processes.
//!
//! FreeRDP starts a new helper for every token it needs (gateway, session
//! host), and the app starts one for its own sign-in. Microsoft's "signed
//! in" cookie is usually a session cookie, which WebKit keeps only in
//! memory, so every new helper would ask the user to sign in again.
//!
//! The helper therefore saves the session cookies (those without an expiry
//! date) after each sign-in and loads them before the next one. They go to
//! `$XDG_RUNTIME_DIR/cosmic-ext-deskfeed-web/<profile name>/cookies.json`:
//! RAM-only, private to the user (folder 0700, file 0600), gone at logout,
//! and removed by the app on sign-out. Persistent cookies stay in the
//! profile's own cookie database as before.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use webkit6::soup;

use common::helper::JAR_DIR;
const FILE: &str = "cookies.json";
const MAX_FILE: u64 = 512 * 1024;
const MAX_COOKIES: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    /// As soup stores it: a leading `.` marks a domain cookie, none a
    /// host-only cookie, so the round trip keeps the scope.
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SameSite {
    None,
    Lax,
    Strict,
}

/// The jar for `profile`, or `None` without a usable runtime directory.
pub fn jar_dir(profile: &Path) -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    let name = profile.file_name()?;
    runtime
        .is_absolute()
        .then(|| runtime.join(JAR_DIR).join(name))
}

/// Reads the jar. Missing, oversized or damaged files give an empty list:
/// the user then simply signs in again.
pub fn load(dir: &Path) -> Vec<StoredCookie> {
    let Ok(file) = fs::File::open(dir.join(FILE)) else {
        return Vec::new();
    };
    let mut text = String::new();
    if file.take(MAX_FILE + 1).read_to_string(&mut text).is_err() || text.len() as u64 > MAX_FILE {
        return Vec::new();
    }
    let mut cookies: Vec<StoredCookie> = serde_json::from_str(&text).unwrap_or_default();
    cookies.retain(StoredCookie::is_sane);
    cookies.truncate(MAX_COOKIES);
    cookies
}

/// Replaces the jar's contents. Errors are ignored: the jar only saves the
/// user a sign-in.
pub fn save(dir: &Path, cookies: &[StoredCookie]) {
    if crate::view::private_dir(dir).is_err() {
        return;
    }
    let cookies: Vec<&StoredCookie> = cookies
        .iter()
        .filter(|c| c.is_sane())
        .take(MAX_COOKIES)
        .collect();
    let Ok(text) = serde_json::to_string(&cookies) else {
        return;
    };
    let tmp = dir.join(format!("{FILE}.{}", std::process::id()));
    let written = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()));
    if written.is_err() || fs::rename(&tmp, dir.join(FILE)).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

impl StoredCookie {
    /// Session cookies only; persistent ones are in the profile already.
    pub fn from_soup(cookie: &soup::Cookie) -> Option<Self> {
        let mut c = cookie.clone();
        if c.expires().is_some() {
            return None;
        }
        let same_site = match c.same_site_policy() {
            soup::SameSitePolicy::Strict => SameSite::Strict,
            soup::SameSitePolicy::Lax => SameSite::Lax,
            _ => SameSite::None,
        };
        let stored = Self {
            name: c.name()?.into(),
            value: c.value()?.into(),
            domain: c.domain()?.into(),
            path: c.path().map_or_else(|| "/".into(), Into::into),
            secure: c.is_secure(),
            http_only: c.is_http_only(),
            same_site,
        };
        stored.is_sane().then_some(stored)
    }

    /// A session cookie (`max_age` -1) with the stored attributes.
    pub fn to_soup(&self) -> soup::Cookie {
        let mut c = soup::Cookie::new(&self.name, &self.value, &self.domain, &self.path, -1);
        c.set_secure(self.secure);
        c.set_http_only(self.http_only);
        c.set_same_site_policy(match self.same_site {
            SameSite::Strict => soup::SameSitePolicy::Strict,
            SameSite::Lax => soup::SameSitePolicy::Lax,
            SameSite::None => soup::SameSitePolicy::None,
        });
        c
    }

    /// Rejects values that could not have come from a real cookie.
    fn is_sane(&self) -> bool {
        let ok = |s: &str, max: usize| s.len() <= max && !s.chars().any(char::is_control);
        !self.name.is_empty()
            && !self.domain.trim_start_matches('.').is_empty()
            && self.path.starts_with('/')
            && ok(&self.name, 256)
            && ok(&self.value, 8192)
            && ok(&self.domain, 255)
            && ok(&self.path, 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(name: &str) -> StoredCookie {
        StoredCookie {
            name: name.into(),
            value: "v".into(),
            domain: "login.microsoftonline.com".into(),
            path: "/".into(),
            secure: true,
            http_only: true,
            same_site: SameSite::None,
        }
    }

    #[test]
    fn round_trip_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("default");
        fs::create_dir_all(&dir).unwrap();
        save(&dir, &[cookie("ESTSAUTH"), cookie("buid")]);
        assert_eq!(load(&dir), vec![cookie("ESTSAUTH"), cookie("buid")]);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(FILE)), 0o600);
    }

    #[test]
    fn a_removed_jar_is_not_recreated() {
        // Signed out while a helper was finishing: its save goes nowhere.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("jars/gone");
        save(&dir, &[cookie("ESTSAUTH")]);
        assert!(!dir.exists());
        assert!(!dir.parent().unwrap().exists());
    }

    #[test]
    fn damaged_or_missing_jar_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("default");
        assert_eq!(load(&dir), Vec::new());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE), "not json").unwrap();
        assert_eq!(load(&dir), Vec::new());
    }

    #[test]
    fn insane_cookies_are_dropped() {
        let mut bad_path = cookie("a");
        bad_path.path = "x".into();
        let mut no_domain = cookie("b");
        no_domain.domain = ".".into();
        let mut control = cookie("c");
        control.value = "x\ny".into();
        for c in [bad_path, no_domain, control, cookie("")] {
            assert!(!c.is_sane(), "{c:?}");
        }
    }

    #[test]
    fn jar_path_uses_runtime_dir_and_profile_name() {
        // Only checks the shape; XDG_RUNTIME_DIR is set in normal sessions.
        if let Some(p) = jar_dir(Path::new("/home/u/.local/share/x/signin/default")) {
            assert!(p.ends_with("cosmic-ext-deskfeed-web/default"));
        }
    }
}
