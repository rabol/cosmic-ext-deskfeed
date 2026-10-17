// SPDX-License-Identifier: MIT

//! Per-account plumbing (spec §6): account IDs, the sign-in web view
//! profile and RAM cookie folder of each account, and saving, renewing and
//! deleting the refresh token in the keyring. Blocking functions; call them
//! from `spawn_blocking`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use sha2::Digest;

use crate::auth::attempt::ClientConfig;
use crate::auth::id_token::{self, Identity};
use crate::auth::signin::SignedIn;
use crate::auth::token::{self, TokenError};
use crate::fl;
use crate::secrets::{self, SecretsError, Stored};
use crate::service::http::Http;

/// Folder of the RAM cookie folders; shared with the helper
/// (`signin/src/session_jar.rs`).
const JAR_DIR: &str = "cosmic-ext-deskfeed-web";
/// The profile name used before multiple accounts existed.
const LEGACY_PROFILE: &str = "default";

/// Opaque, stable ID for an account: a hash of tenant and object ID, in
/// hex (so never `new-…` or `default`). Used for folder names and config
/// keys, never shown.
pub fn account_id(identity: &Identity) -> String {
    use std::fmt::Write as _;
    let mut h = sha2::Sha256::new();
    for part in [identity.tenant_id.as_str(), identity.object_id.as_str()] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    h.finalize()[..10].iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn profiles_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("cosmic-ext-deskfeed").join("signin"))
}

fn jars_root() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    runtime.is_absolute().then(|| runtime.join(JAR_DIR))
}

/// The account's sign-in web view profile (cookies), spec §6.
pub fn profile_dir(id: &str) -> Option<PathBuf> {
    Some(profiles_root()?.join(id))
}

/// A fresh profile for adding an account whose ID isn't known yet.
pub fn new_profile_dir() -> Option<PathBuf> {
    let mut bytes = [0u8; 9];
    getrandom::fill(&mut bytes).ok()?;
    let name = format!(
        "new-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    );
    Some(profiles_root()?.join(name))
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

/// Before multiple accounts, the one profile was called `default`. The
/// first account restored takes it over.
pub fn adopt_legacy_profile(id: &str) {
    let (Some(root), Some(target)) = (profiles_root(), profile_dir(id)) else {
        return;
    };
    let legacy = root.join(LEGACY_PROFILE);
    if legacy.exists() && !target.exists() {
        adopt_profile(&legacy, id);
    }
}

/// Removes an account's cookies and RAM cookie folder (sign-out).
pub fn remove_files(id: &str) -> Result<(), ()> {
    let mut ok = true;
    for dir in [profile_dir(id), jars_root().map(|j| j.join(id))]
        .into_iter()
        .flatten()
    {
        if dir.exists() && fs::remove_dir_all(&dir).is_err() {
            ok = false;
        }
    }
    if ok { Ok(()) } else { Err(()) }
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

/// One token renewal or keyring write at a time (spec §6: serialize
/// refresh per account; serializing all accounts is simpler and enough).
static RENEWAL: Mutex<()> = Mutex::new(());

/// Sign-outs per account. Work that started before an account's sign-out
/// must not save its tokens afterwards (spec §6: late callbacks must not
/// restore the account).
static SIGN_OUTS: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn sign_out_count(id: &str) -> u64 {
    SIGN_OUTS
        .lock()
        .map(|m| m.get(id).copied().unwrap_or(0))
        .unwrap_or(0)
}

fn note_sign_out(id: &str) {
    if let Ok(mut m) = SIGN_OUTS.lock() {
        *m.entry(id.to_owned()).or_insert(0) += 1;
    }
}

fn config() -> ClientConfig {
    ClientConfig::public_remote_desktop()
}

/// Saves the account's refresh token, unless it was signed out since
/// `started` (a [`sign_out_count`]). The error is the notice to show.
pub fn save(account: &SignedIn, started: u64) -> Result<(), String> {
    let _renewal = RENEWAL.lock();
    save_locked(account, started)
}

fn save_locked(account: &SignedIn, started: u64) -> Result<(), String> {
    if sign_out_count(&account_id(&account.identity)) != started {
        crate::app_log!("saved sign-in: not stored (signed out meanwhile)");
        return Ok(());
    }
    let Some(refresh_token) = &account.tokens.refresh_token else {
        crate::app_log!("saved sign-in: not stored (no refresh token in the response)");
        return Err(fl!("notice-not-saved", detail = "no refresh token"));
    };
    let result = secrets::save(&config(), &account.identity, refresh_token);
    // Diagnostics: outcome and error kind only, never tokens or IDs.
    match &result {
        Ok(()) => crate::app_log!("saved sign-in: stored"),
        Err(e) => crate::app_log!("saved sign-in: not stored ({e})"),
    }
    result.map_err(|e| fl!("notice-not-saved", detail = keyring_detail(&e)))
}

/// Signs the account out: from now on its tokens aren't saved, and its
/// keyring item is deleted.
pub fn delete(identity: &Identity) -> Result<(), String> {
    note_sign_out(&account_id(identity));
    let _renewal = RENEWAL.lock();
    let deleted = secrets::delete(&config(), identity);
    crate::app_log!("saved sign-in: deleted (sign-out): {deleted:?}");
    deleted.map_err(|_| fl!("error-sign-out-incomplete"))
}

pub fn keyring_detail(e: &SecretsError) -> String {
    match e {
        SecretsError::Dismissed => fl!("keyring-dismissed"),
        SecretsError::Malformed => fl!("keyring-malformed"),
        SecretsError::Unavailable(_) => fl!("keyring-unavailable"),
    }
}

/// The kind of a token error, for diagnostics (no server text).
fn token_error_kind(e: &TokenError) -> String {
    match e {
        TokenError::Transient(_) => "network".into(),
        TokenError::InvalidGrant { aadsts } => format!("invalid_grant, AADSTS {aadsts:?}"),
        TokenError::InteractionRequired { aadsts } => {
            format!("interaction_required, AADSTS {aadsts:?}")
        }
        TokenError::ConsentRequired { aadsts } => format!("consent_required, AADSTS {aadsts:?}"),
        TokenError::Denied { aadsts } => format!("denied by policy, AADSTS {aadsts:?}"),
        TokenError::Other { error, aadsts } => format!("{error}, AADSTS {aadsts:?}"),
        TokenError::Malformed => "malformed response".into(),
    }
}

/// All saved sign-ins, or the text to show if the keyring can't be read.
pub fn load_saved() -> Result<Vec<Stored>, String> {
    let _renewal = RENEWAL.lock();
    match secrets::load_all(&config()) {
        Ok(all) => {
            crate::app_log!("saved sign-in: {} found", all.len());
            Ok(all)
        }
        Err(e) => {
            crate::app_log!("saved sign-in: can't load ({e})");
            Err(fl!("error-keyring-load", detail = keyring_detail(&e)))
        }
    }
}

/// The outcome of signing in with a saved refresh token.
#[derive(Debug, Clone)]
pub enum Restore {
    /// Signed in. The text is a notice if the renewed token couldn't be
    /// saved.
    Ready(Arc<SignedIn>, Option<String>),
    /// The saved sign-in can't be used any more; sign in again.
    SignInAgain(String),
    /// Network trouble; the saved sign-in is kept for another try.
    Retry(String),
}

/// Signs in with a saved refresh token.
pub fn restore(http: &Http, stored: &Stored) -> Restore {
    let config = config();
    let id = account_id(&stored.identity);
    let started = sign_out_count(&id);
    let _renewal = RENEWAL.lock();
    let renewed = token::refresh(http, &config, &stored.refresh_token);
    if let Err(e) = &renewed {
        crate::app_log!("saved sign-in: renewal failed ({})", token_error_kind(e));
    }
    let tokens = match renewed {
        Ok(tokens) => tokens,
        Err(TokenError::Transient(_)) => return Restore::Retry(fl!("error-restore-network")),
        Err(TokenError::InvalidGrant { aadsts }) => {
            // Revoked or expired for good: forget it.
            let deleted = secrets::delete(&config, &stored.identity);
            crate::app_log!("saved sign-in: deleted (invalid_grant): {deleted:?}");
            // AADSTS70043 "BadTokenDueToSignInFrequency": the organization's
            // Conditional Access asks for a new sign-in after a set time
            // (Microsoft Entra error code reference).
            return Restore::SignInAgain(if aadsts == Some(70043) {
                fl!("error-sign-in-frequency")
            } else {
                fl!("error-saved-sign-in-expired")
            });
        }
        // Policy or interaction needed: an interactive sign-in replaces it.
        Err(_) => return Restore::SignInAgain(fl!("error-saved-sign-in-expired")),
    };
    // A renewed ID token must name the same account (and is checked like
    // any other); without one, the saved identity stands.
    let identity = match tokens.id_token.as_deref() {
        None => stored.identity.clone(),
        Some(raw) => {
            let Ok(keys) = id_token::fetch_keys(http) else {
                return Restore::Retry(fl!("error-restore-network"));
            };
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            match id_token::validate_renewed(raw, &keys, &config.client_id, now, &stored.identity) {
                Ok(identity) => identity,
                Err(_) => return Restore::SignInAgain(fl!("error-saved-sign-in-expired")),
            }
        }
    };
    let rotated = tokens
        .refresh_token
        .as_ref()
        .is_some_and(|t| t.expose() != stored.refresh_token.expose());
    crate::app_log!(
        "saved sign-in: renewed (refresh token {})",
        if rotated { "rotated" } else { "unchanged" }
    );
    let account = SignedIn { identity, tokens };
    let notice = if rotated {
        save_locked(&account, started).err()
    } else {
        None
    };
    Restore::Ready(Arc::new(account), notice)
}

/// Renews the tokens when the access token expires within the margin, and
/// saves a rotated refresh token.
pub fn renew_if_needed(http: &Http, account: Arc<SignedIn>) -> Result<Arc<SignedIn>, String> {
    if !account.tokens.needs_refresh(Instant::now()) {
        return Ok(account);
    }
    let started = sign_out_count(&account_id(&account.identity));
    let _renewal = RENEWAL.lock();
    let Some(refresh_token) = &account.tokens.refresh_token else {
        return Err(fl!("error-session-expired"));
    };
    match token::refresh(http, &config(), refresh_token) {
        Ok(tokens) => {
            let renewed = SignedIn {
                identity: account.identity.clone(),
                tokens,
            };
            let rotated = renewed
                .tokens
                .refresh_token
                .as_ref()
                .is_some_and(|t| t.expose() != refresh_token.expose());
            if rotated {
                // Best effort: if this fails, the next start asks to sign in.
                let _ = save_locked(&renewed, started);
            }
            Ok(Arc::new(renewed))
        }
        Err(TokenError::Transient(_)) => Err(fl!("error-generic")),
        Err(_) => Err(fl!("error-session-expired")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(oid: &str) -> Identity {
        Identity {
            tenant_id: "11111111-2222-3333-4444-555555555555".into(),
            object_id: oid.into(),
            username: Some("anna@example.com".into()),
            display_name: None,
        }
    }

    #[test]
    fn account_ids_are_stable_opaque_and_distinct() {
        let a = account_id(&identity("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"));
        let b = account_id(&identity("ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee"));
        assert_eq!(
            a,
            account_id(&identity("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"))
        );
        assert_ne!(a, b);
        assert_eq!(a.len(), 20);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn sign_outs_are_counted_per_account() {
        let before_a = sign_out_count("test-a");
        let before_b = sign_out_count("test-b");
        note_sign_out("test-a");
        assert_eq!(sign_out_count("test-a"), before_a + 1);
        assert_eq!(sign_out_count("test-b"), before_b);
    }
}
