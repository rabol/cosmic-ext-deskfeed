// SPDX-License-Identifier: MIT

//! Each account's sign-in web view profile (cookies, spec §6) and its RAM
//! cookie folder.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;

/// Folder of the RAM cookie folders; shared with the helper
/// (`signin/src/session_jar.rs`).
const JAR_DIR: &str = "cosmic-ext-deskfeed-web";

fn profiles_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("cosmic-ext-deskfeed").join("signin"))
}

pub(super) fn jars_root() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    runtime.is_absolute().then(|| runtime.join(JAR_DIR))
}

/// The account's sign-in web view profile (cookies), spec §6.
pub fn profile_dir(id: &str) -> Option<PathBuf> {
    Some(profiles_root()?.join(id))
}

/// The account's profile, with it and its RAM cookie folder created
/// (private). Only the app creates these; the sign-in helper uses them
/// and never recreates them after a sign-out deleted them.
pub fn prepared_profile(id: &str) -> Option<PathBuf> {
    let profile = profile_dir(id)?;
    prepare(&profile).then_some(profile)
}

/// A fresh, prepared profile for adding an account whose ID isn't known
/// yet.
pub fn new_profile_dir() -> Option<PathBuf> {
    let mut bytes = [0u8; 9];
    getrandom::fill(&mut bytes).ok()?;
    let name = format!(
        "new-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    );
    let profile = profiles_root()?.join(name);
    prepare(&profile).then_some(profile)
}

/// Creates `profile` and its RAM cookie folder, private to the user.
fn prepare(profile: &Path) -> bool {
    let private = |dir: &Path| crate::files::private_dir(dir).is_ok();
    let jar = profile
        .file_name()
        .and_then(|name| Some(jars_root()?.join(name)));
    private(profile) && jar.is_none_or(|j| private(&j))
}

/// Removes the profile (and RAM cookie folder) of a sign-in that didn't
/// add an account. Only after its sign-in window has closed.
pub fn remove_unused_profile(profile: &Path) {
    let Some(name) = profile.file_name() else {
        return;
    };
    if !name.to_string_lossy().starts_with("new-") {
        return;
    }
    let _ = fs::remove_dir_all(profile);
    if let Some(jars) = jars_root() {
        let _ = fs::remove_dir_all(jars.join(name));
    }
}

/// Files the profile (and its RAM cookie folder) used for a sign-in under
/// the account's ID, replacing what was there. Best effort: a failure only
/// means the account signs in again later.
pub fn adopt_profile(used: &Path, id: &str) {
    let (Some(target), Some(name)) = (profile_dir(id), used.file_name()) else {
        return;
    };
    if used == target {
        return;
    }
    if used.exists() {
        let _ = fs::remove_dir_all(&target);
        let _ = fs::rename(used, &target);
    }
    if let Some(jars) = jars_root() {
        let from = jars.join(name);
        if from.exists() {
            let to = jars.join(id);
            let _ = fs::remove_dir_all(&to);
            let _ = fs::rename(from, to);
        }
    }
}

/// Removes profiles of sign-ins that never finished.
pub fn clean_unfinished_profiles() {
    if let Some(root) = profiles_root()
        && let Ok(entries) = fs::read_dir(root)
    {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("new-") {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}
