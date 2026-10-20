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
use crate::auth::token::{self, Secret, TokenError, TokenSet};
use crate::fl;
use crate::secrets::{self, SecretsError, Stored};
use crate::service::http::Http;

/// Folder of the RAM cookie folders; shared with the helper
/// (`signin/src/session_jar.rs`).
const JAR_DIR: &str = "cosmic-ext-deskfeed-web";

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

/// Removes an account's cookies, RAM cookie folder and offline desktop
/// list (sign-out, spec §6: credentials and account caches).
pub fn remove_files(id: &str) -> Result<(), ()> {
    // Under the account's lock, like the cache's saves (`save_cache`).
    let lock = account_lock(id);
    let _account = lock.lock();
    let mut ok = crate::cache::Store::user().is_none_or(|s| s.remove(id).is_ok());
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

/// One token renewal or keyring write at a time per account (spec §6).
/// Per account, so one account waiting (for example on a keyring prompt)
/// doesn't hold up another's renewal.
static ACCOUNT_LOCKS: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn account_lock(id: &str) -> Arc<Mutex<()>> {
    ACCOUNT_LOCKS
        .lock()
        .expect("not poisoned")
        .entry(id.to_owned())
        .or_default()
        .clone()
}

/// One call to the keyring at a time, so it shows at most one unlock
/// prompt.
static KEYRING: Mutex<()> = Mutex::new(());

/// Sign-outs per account. Work that started before an account's sign-out
/// must not save its tokens afterwards (spec §6: late callbacks must not
/// restore the account).
static SIGN_OUTS: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn sign_out_count(id: &str) -> u64 {
    SIGN_OUTS
        .lock()
        .map_or(0, |m| m.get(id).copied().unwrap_or(0))
}

/// From now on, work that started earlier for this account saves nothing
/// and its newest tokens are forgotten.
pub fn note_sign_out(id: &str) {
    if let Ok(mut m) = SIGN_OUTS.lock() {
        *m.entry(id.to_owned()).or_insert(0) += 1;
    }
    if let Ok(mut latest) = LATEST.lock() {
        latest.remove(id);
    }
}

/// The newest tokens per account (spec §6: a renewal must not reuse a
/// refresh token another renewal already replaced). Connections and
/// refreshes each hold their own copy; renewals look here first.
static LATEST: LazyLock<Mutex<HashMap<String, Arc<SignedIn>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Records `account`'s tokens as the newest, unless newer ones are known
/// or the account was signed out since `started`.
fn note_tokens(account: &Arc<SignedIn>, started: u64) {
    let id = account_id(&account.identity);
    if sign_out_count(&id) != started {
        return;
    }
    if let Ok(mut latest) = LATEST.lock() {
        let newer = latest
            .get(&id)
            .is_none_or(|known| account.tokens.expires_at >= known.tokens.expires_at);
        if newer {
            latest.insert(id, account.clone());
        }
    }
}

/// The newest tokens known for the account, if newer than `account`'s.
fn newest(account: &Arc<SignedIn>) -> Option<Arc<SignedIn>> {
    let latest = LATEST.lock().ok()?;
    latest
        .get(&account_id(&account.identity))
        .filter(|known| known.tokens.expires_at > account.tokens.expires_at)
        .cloned()
}

/// An interactive sign-in finished: its tokens are the newest.
pub fn signed_in(account: &Arc<SignedIn>, started: u64) {
    note_tokens(account, started);
}

/// Where saved sign-ins live: the desktop keyring, or a fake in tests.
trait Store {
    fn load_all(&self, config: &ClientConfig) -> Result<Vec<Stored>, SecretsError>;
    fn save(
        &self,
        config: &ClientConfig,
        identity: &Identity,
        refresh_token: &Secret,
    ) -> Result<(), SecretsError>;
    fn delete(&self, config: &ClientConfig, identity: &Identity) -> Result<(), SecretsError>;
    /// A remembered Windows password for `desktop`.
    fn save_password(
        &self,
        identity: &Identity,
        desktop: &str,
        title: &str,
        password: &Secret,
    ) -> Result<(), SecretsError>;
    /// The account's remembered passwords, or one desktop's.
    fn delete_passwords(
        &self,
        identity: &Identity,
        desktop: Option<&str>,
    ) -> Result<(), SecretsError>;
}

/// The desktop keyring (Secret Service).
struct Keyring;

impl Store for Keyring {
    fn load_all(&self, config: &ClientConfig) -> Result<Vec<Stored>, SecretsError> {
        let _keyring = KEYRING.lock();
        secrets::load_all(config)
    }
    fn save(
        &self,
        config: &ClientConfig,
        identity: &Identity,
        refresh_token: &Secret,
    ) -> Result<(), SecretsError> {
        let _keyring = KEYRING.lock();
        secrets::save(config, identity, refresh_token)
    }
    fn delete(&self, config: &ClientConfig, identity: &Identity) -> Result<(), SecretsError> {
        let _keyring = KEYRING.lock();
        secrets::delete(config, identity)
    }
    fn save_password(
        &self,
        identity: &Identity,
        desktop: &str,
        title: &str,
        password: &Secret,
    ) -> Result<(), SecretsError> {
        let _keyring = KEYRING.lock();
        secrets::save_password(identity, desktop, title, password)
    }
    fn delete_passwords(
        &self,
        identity: &Identity,
        desktop: Option<&str>,
    ) -> Result<(), SecretsError> {
        let _keyring = KEYRING.lock();
        secrets::delete_passwords(identity, desktop)
    }
}

/// What the functions below work with: the client identity (its token
/// endpoint) and the store.
struct Env<'a> {
    config: ClientConfig,
    store: &'a dyn Store,
}

fn system() -> Env<'static> {
    Env {
        config: ClientConfig::public_remote_desktop(),
        store: &Keyring,
    }
}

/// Saves the account's refresh token, unless it was signed out since
/// `started` (a [`sign_out_count`]). The error is the notice to show.
pub fn save(account: &SignedIn, started: u64) -> Result<(), String> {
    let lock = account_lock(&account_id(&account.identity));
    let _account = lock.lock();
    save_locked(&system(), account, started)
}

fn save_locked(env: &Env, account: &SignedIn, started: u64) -> Result<(), String> {
    if sign_out_count(&account_id(&account.identity)) != started {
        crate::app_log!("saved sign-in: not stored (signed out meanwhile)");
        return Ok(());
    }
    let Some(refresh_token) = &account.tokens.refresh_token else {
        crate::app_log!("saved sign-in: not stored (no refresh token in the response)");
        return Err(fl!("notice-not-saved", detail = "no refresh token"));
    };
    let result = env
        .store
        .save(&env.config, &account.identity, refresh_token);
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
    delete_in(&system(), identity)
}

fn delete_in(env: &Env, identity: &Identity) -> Result<(), String> {
    note_sign_out(&account_id(identity));
    let lock = account_lock(&account_id(identity));
    let _account = lock.lock();
    let deleted = env.store.delete(&env.config, identity);
    crate::app_log!("saved sign-in: deleted (sign-out): {deleted:?}");
    // Under the same lock as their saves (`save_password`).
    let passwords = env.store.delete_passwords(identity, None);
    crate::app_log!(
        "remembered passwords deleted (sign-out): {:?}",
        passwords.is_ok()
    );
    deleted
        .and(passwords)
        .map_err(|_| fl!("error-sign-out-incomplete"))
}

/// Saves the account's offline list, unless it was signed out since
/// `started` ([`sign_out_count`] when the save was scheduled). Under the
/// account's lock, like [`remove_files`], so a save queued before a
/// sign-out can't bring the list back after it.
pub fn save_cache(
    store: &crate::cache::Store,
    id: &str,
    list: &[crate::cache::CachedWorkspace],
    started: u64,
) -> Result<(), String> {
    let lock = account_lock(id);
    let _account = lock.lock();
    if sign_out_count(id) != started {
        crate::app_log!("offline desktop list: not saved (signed out meanwhile)");
        return Ok(());
    }
    store.save(id, list)
}

/// "Forget" per account and desktop, like [`SIGN_OUTS`]: a password save
/// scheduled before a Forget writes nothing after it.
static FORGETS: LazyLock<Mutex<HashMap<(String, String), u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// What a remembered-password save was scheduled under: the account's
/// sign-outs and the desktop's forgets at that moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordTicket {
    sign_outs: u64,
    forgets: u64,
}

pub fn password_ticket(id: &str, desktop: &str) -> PasswordTicket {
    PasswordTicket {
        sign_outs: sign_out_count(id),
        forgets: FORGETS.lock().map_or(0, |m| {
            m.get(&(id.to_owned(), desktop.to_owned()))
                .copied()
                .unwrap_or(0)
        }),
    }
}

/// Per account and desktop: the forget count the last stored password was
/// scheduled under. A Forget whose deletion runs after a newer save (one
/// scheduled after that Forget) must leave the newer password alone.
static SAVED_AFTER: LazyLock<Mutex<HashMap<(String, String), u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The user forgot this desktop's password: saves scheduled earlier write
/// nothing. Call at once, before deleting; give the result to
/// [`forget_password`].
pub fn note_forget(id: &str, desktop: &str) -> u64 {
    FORGETS.lock().map_or(0, |mut m| {
        let n = m.entry((id.to_owned(), desktop.to_owned())).or_insert(0);
        *n += 1;
        *n
    })
}

/// Remembers a Windows password, unless the account signed out or the
/// password was forgotten since `ticket`. `Ok(false)`: not saved for that
/// reason. Under the account's lock, like the deletions.
pub fn save_password(
    identity: &Identity,
    desktop: &str,
    title: &str,
    password: &Secret,
    ticket: PasswordTicket,
) -> Result<bool, String> {
    save_password_in(&system(), identity, desktop, title, password, ticket)
}

fn save_password_in(
    env: &Env,
    identity: &Identity,
    desktop: &str,
    title: &str,
    password: &Secret,
    ticket: PasswordTicket,
) -> Result<bool, String> {
    let id = account_id(identity);
    let lock = account_lock(&id);
    let _account = lock.lock();
    if password_ticket(&id, desktop) != ticket {
        crate::app_log!("remembered password: not stored (signed out or forgotten meanwhile)");
        return Ok(false);
    }
    env.store
        .save_password(identity, desktop, title, password)
        .map_err(|e| keyring_detail(&e))?;
    if let Ok(mut m) = SAVED_AFTER.lock() {
        m.insert((id, desktop.to_owned()), ticket.forgets);
    }
    Ok(true)
}

/// Deletes one desktop's remembered password. `forget`: what
/// [`note_forget`] returned for this Forget.
pub fn forget_password(identity: &Identity, desktop: &str, forget: u64) -> Result<(), String> {
    forget_password_in(&system(), identity, desktop, forget)
}

fn forget_password_in(
    env: &Env,
    identity: &Identity,
    desktop: &str,
    forget: u64,
) -> Result<(), String> {
    let id = account_id(identity);
    let lock = account_lock(&id);
    let _account = lock.lock();
    // A password saved after this Forget was asked for is the user's
    // newer choice: keep it.
    let newer = SAVED_AFTER
        .lock()
        .ok()
        .and_then(|m| m.get(&(id, desktop.to_owned())).copied())
        .is_some_and(|saved| saved >= forget);
    if newer {
        return Ok(());
    }
    env.store
        .delete_passwords(identity, Some(desktop))
        .map_err(|e| keyring_detail(&e))
}

pub fn keyring_detail(e: &SecretsError) -> String {
    match e {
        SecretsError::Dismissed => fl!("keyring-dismissed"),
        SecretsError::NoAnswer => fl!("keyring-no-answer"),
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
    load_saved_in(&system())
}

fn load_saved_in(env: &Env) -> Result<Vec<Stored>, String> {
    match env.store.load_all(&env.config) {
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

/// Signs in with a saved refresh token. `started`: [`sign_out_count`] when
/// this was scheduled, so a restore queued before a sign-out does nothing
/// after it.
pub fn restore(http: &Http, stored: &Stored, started: u64) -> Restore {
    restore_at(http, &system(), stored, started)
}

/// Tests: `restore_at` scheduled now.
#[cfg(test)]
fn restore_in(http: &Http, env: &Env, stored: &Stored) -> Restore {
    restore_at(
        http,
        env,
        stored,
        sign_out_count(&account_id(&stored.identity)),
    )
}

fn restore_at(http: &Http, env: &Env, stored: &Stored, started: u64) -> Restore {
    let config = &env.config;
    let id = account_id(&stored.identity);
    let lock = account_lock(&id);
    let _account = lock.lock();
    // Signed out since this was scheduled: no network, no saving.
    if sign_out_count(&id) != started {
        return Restore::SignInAgain(fl!("error-session-expired"));
    }
    let renewed = token::refresh(http, config, &stored.refresh_token);
    if let Err(e) = &renewed {
        crate::app_log!("saved sign-in: renewal failed ({})", token_error_kind(e));
    }
    let tokens = match renewed {
        Ok(tokens) => tokens,
        Err(TokenError::Transient(_)) => return Restore::Retry(fl!("error-restore-network")),
        Err(TokenError::InvalidGrant { aadsts }) => {
            // Revoked or expired for good: forget it.
            let deleted = env.store.delete(config, &stored.identity);
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
    let identity = match renewed_identity(http, config, &tokens, &stored.identity) {
        Ok(identity) => identity,
        Err(IdentityCheck::KeysUnavailable) => return Restore::Retry(fl!("error-restore-network")),
        Err(IdentityCheck::Invalid) => {
            return Restore::SignInAgain(fl!("error-saved-sign-in-expired"));
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
    let account = Arc::new(SignedIn { identity, tokens });
    let notice = if rotated {
        save_locked(env, &account, started).err()
    } else {
        None
    };
    note_tokens(&account, started);
    Restore::Ready(account, notice)
}

/// Why renewed tokens can't be used.
enum IdentityCheck {
    /// Microsoft's signing keys couldn't be fetched: try again later.
    KeysUnavailable,
    /// The renewed ID token is invalid or names another account.
    Invalid,
}

/// The identity for renewed tokens. A renewed ID token must name the same
/// account and is checked like any other (signature, issuer, audience,
/// lifetime); without one, the known identity stands.
fn renewed_identity(
    http: &Http,
    config: &ClientConfig,
    tokens: &TokenSet,
    expected: &Identity,
) -> Result<Identity, IdentityCheck> {
    let Some(raw) = tokens.id_token.as_deref() else {
        return Ok(expected.clone());
    };
    let keys = id_token::fetch_keys(http).map_err(|_| IdentityCheck::KeysUnavailable)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    id_token::validate_renewed(raw, &keys, &config.client_id, now, expected).map_err(|e| {
        crate::app_log!("renewed ID token refused: {e:?}");
        IdentityCheck::Invalid
    })
}

/// Why the tokens couldn't be renewed; the text is for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenewError {
    /// Temporary (network): the sign-in stays, try again later.
    Retry(String),
    /// The sign-in can't be renewed any more: sign in again.
    SignInAgain(String),
}

/// Renews the tokens when the access token expires within the margin, and
/// saves a rotated refresh token.
pub fn renew_if_needed(
    http: &Http,
    account: Arc<SignedIn>,
    started: u64,
) -> Result<Arc<SignedIn>, RenewError> {
    renew_at(http, &system(), account, false, started)
}

/// Renews the tokens now: the service refused an access token that looked
/// valid (revoked, or the clocks disagree).
pub fn renew_now(
    http: &Http,
    account: Arc<SignedIn>,
    started: u64,
) -> Result<Arc<SignedIn>, RenewError> {
    renew_at(http, &system(), account, true, started)
}

/// Tests: `renew_at` scheduled now.
#[cfg(test)]
fn renew_in(
    http: &Http,
    env: &Env,
    account: Arc<SignedIn>,
    force: bool,
) -> Result<Arc<SignedIn>, RenewError> {
    let started = sign_out_count(&account_id(&account.identity));
    renew_at(http, env, account, force, started)
}

/// `started`: [`sign_out_count`] when the work was scheduled.
fn renew_at(
    http: &Http,
    env: &Env,
    account: Arc<SignedIn>,
    force: bool,
    started: u64,
) -> Result<Arc<SignedIn>, RenewError> {
    if !force && !account.tokens.needs_refresh(Instant::now()) {
        return Ok(account);
    }
    let lock = account_lock(&account_id(&account.identity));
    let _account = lock.lock();
    // Signed out since this was scheduled: no network, no saving.
    if sign_out_count(&account_id(&account.identity)) != started {
        return Err(RenewError::SignInAgain(fl!("error-session-expired")));
    }
    // Another renewal may have finished while this one waited for the
    // lock: use its result, or at least its (rotated) refresh token.
    let refused = account.tokens.access_token.expose().to_owned();
    let account = match newest(&account) {
        Some(known)
            if !known.tokens.needs_refresh(Instant::now())
                && (!force || known.tokens.access_token.expose() != refused) =>
        {
            return Ok(known);
        }
        Some(known) => known,
        None => account,
    };
    let Some(refresh_token) = &account.tokens.refresh_token else {
        return Err(RenewError::SignInAgain(fl!("error-session-expired")));
    };
    let tokens = match token::refresh(http, &env.config, refresh_token) {
        Ok(tokens) => tokens,
        Err(e) => {
            crate::app_log!("renewal failed ({})", token_error_kind(&e));
            return Err(match e {
                TokenError::Transient(_) => RenewError::Retry(fl!("error-generic")),
                TokenError::InvalidGrant {
                    aadsts: Some(70043),
                } => RenewError::SignInAgain(fl!("error-sign-in-frequency")),
                _ => RenewError::SignInAgain(fl!("error-session-expired")),
            });
        }
    };
    let identity = match renewed_identity(http, &env.config, &tokens, &account.identity) {
        Ok(identity) => identity,
        Err(IdentityCheck::KeysUnavailable) => {
            return Err(RenewError::Retry(fl!("error-generic")));
        }
        Err(IdentityCheck::Invalid) => {
            return Err(RenewError::SignInAgain(fl!("error-session-expired")));
        }
    };
    let renewed = SignedIn { identity, tokens };
    let rotated = renewed
        .tokens
        .refresh_token
        .as_ref()
        .is_some_and(|t| t.expose() != refresh_token.expose());
    if rotated {
        // Best effort: if this fails, the next start asks to sign in.
        let _ = save_locked(env, &renewed, started);
    }
    let renewed = Arc::new(renewed);
    note_tokens(&renewed, started);
    Ok(renewed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::endpoint::EndpointPolicy;
    use crate::service::http::BearerToken;
    use crate::test_util::serve_once;
    use std::time::Duration;
    use url::Url;

    /// An in-memory keyring. `fail` makes every call fail like a locked or
    /// missing Secret Service.
    #[derive(Default)]
    struct FakeStore {
        saved: Mutex<Vec<(Identity, String)>>,
        /// Remembered passwords: account and desktop.
        passwords: Mutex<Vec<(Identity, String)>>,
        fail: bool,
    }

    impl FakeStore {
        fn with(identity: &Identity, token: &str) -> Self {
            let s = Self::default();
            s.saved
                .lock()
                .unwrap()
                .push((identity.clone(), token.to_owned()));
            s
        }
        fn tokens(&self) -> Vec<String> {
            self.saved
                .lock()
                .unwrap()
                .iter()
                .map(|(_, t)| t.clone())
                .collect()
        }
        fn err() -> SecretsError {
            SecretsError::Unavailable("test".into())
        }
    }

    impl Store for FakeStore {
        fn load_all(&self, _: &ClientConfig) -> Result<Vec<Stored>, SecretsError> {
            if self.fail {
                return Err(Self::err());
            }
            Ok(self
                .saved
                .lock()
                .unwrap()
                .iter()
                .map(|(identity, t)| Stored {
                    identity: identity.clone(),
                    refresh_token: Secret::new(t.clone()),
                })
                .collect())
        }
        fn save(
            &self,
            _: &ClientConfig,
            identity: &Identity,
            rt: &Secret,
        ) -> Result<(), SecretsError> {
            if self.fail {
                return Err(Self::err());
            }
            let mut saved = self.saved.lock().unwrap();
            saved.retain(|(i, _)| i != identity);
            saved.push((identity.clone(), rt.expose().to_owned()));
            Ok(())
        }
        fn delete(&self, _: &ClientConfig, identity: &Identity) -> Result<(), SecretsError> {
            if self.fail {
                return Err(Self::err());
            }
            self.saved.lock().unwrap().retain(|(i, _)| i != identity);
            Ok(())
        }
        fn save_password(
            &self,
            identity: &Identity,
            desktop: &str,
            _: &str,
            _: &Secret,
        ) -> Result<(), SecretsError> {
            self.passwords
                .lock()
                .unwrap()
                .push((identity.clone(), desktop.to_owned()));
            Ok(())
        }
        fn delete_passwords(
            &self,
            identity: &Identity,
            desktop: Option<&str>,
        ) -> Result<(), SecretsError> {
            self.passwords
                .lock()
                .unwrap()
                .retain(|(i, d)| i != identity || desktop.is_some_and(|k| k != d));
            Ok(())
        }
    }

    /// The token endpoint on a local test server; the store given.
    fn env(port: u16, store: &FakeStore) -> Env<'_> {
        let mut config = ClientConfig::public_remote_desktop();
        config.authority = Url::parse(&format!("http://127.0.0.1:{port}/organizations")).unwrap();
        Env { config, store }
    }

    fn http(port: u16) -> Http {
        Http::new(EndpointPolicy::local_test(port))
    }

    fn stored(identity: &Identity, token: &str) -> Stored {
        Stored {
            identity: identity.clone(),
            refresh_token: Secret::new(token.into()),
        }
    }

    /// A signed-in account whose access token expires in `valid_for`.
    fn signed_in(identity: &Identity, refresh: Option<&str>, valid_for: Duration) -> Arc<SignedIn> {
        Arc::new(SignedIn {
            identity: identity.clone(),
            tokens: TokenSet {
                access_token: BearerToken::new("old-access".into()),
                refresh_token: refresh.map(|t| Secret::new(t.into())),
                id_token: None,
                expires_at: Instant::now() + valid_for,
                expires_wall: SystemTime::now() + valid_for,
            },
        })
    }

    const OK_SAME: &str = r#"{"access_token":"new-access","token_type":"Bearer","expires_in":3600,"refresh_token":"rt-old"}"#;
    const OK_ROTATED: &str = r#"{"access_token":"new-access","token_type":"Bearer","expires_in":3600,"refresh_token":"rt-new"}"#;
    const BAD: &str = "400 Bad Request";

    #[test]
    fn restore_without_rotation_saves_nothing() {
        let me = identity("00000000-0000-0000-0000-0000000000a1");
        let (port, _) = serve_once("200 OK", OK_SAME);
        let store = FakeStore::default();
        match restore_in(&http(port), &env(port, &store), &stored(&me, "rt-old")) {
            Restore::Ready(account, notice) => {
                assert_eq!(account.identity, me);
                assert!(notice.is_none());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(store.tokens(), Vec::<String>::new());
    }

    #[test]
    fn restore_saves_a_rotated_token() {
        let me = identity("00000000-0000-0000-0000-0000000000a2");
        let (port, sent) = serve_once("200 OK", OK_ROTATED);
        let store = FakeStore::with(&me, "rt-old");
        let outcome = restore_in(&http(port), &env(port, &store), &stored(&me, "rt-old"));
        assert!(matches!(outcome, Restore::Ready(_, None)), "{outcome:?}");
        assert_eq!(store.tokens(), ["rt-new"]);
        // The renewal used the saved token.
        assert!(sent.lock().unwrap().contains("refresh_token=rt-old"));
    }

    #[test]
    fn restore_reports_when_the_rotated_token_cant_be_saved() {
        let me = identity("00000000-0000-0000-0000-0000000000a3");
        let (port, _) = serve_once("200 OK", OK_ROTATED);
        let store = FakeStore {
            fail: true,
            ..FakeStore::default()
        };
        match restore_in(&http(port), &env(port, &store), &stored(&me, "rt-old")) {
            // Signed in all the same, with a notice.
            Restore::Ready(_, Some(notice)) => {
                assert_eq!(
                    notice,
                    fl!(
                        "notice-not-saved",
                        detail = keyring_detail(&FakeStore::err())
                    )
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn invalid_saved_sign_in_is_deleted() {
        let me = identity("00000000-0000-0000-0000-0000000000a4");
        let (port, _) = serve_once(BAD, r#"{"error":"invalid_grant","error_codes":[70008]}"#);
        let store = FakeStore::with(&me, "rt-old");
        let outcome = restore_in(&http(port), &env(port, &store), &stored(&me, "rt-old"));
        assert!(
            matches!(&outcome, Restore::SignInAgain(m) if *m == fl!("error-saved-sign-in-expired")),
            "{outcome:?}"
        );
        assert_eq!(store.tokens(), Vec::<String>::new());
    }

    #[test]
    fn sign_in_frequency_is_explained() {
        let me = identity("00000000-0000-0000-0000-0000000000a5");
        let (port, _) = serve_once(BAD, r#"{"error":"invalid_grant","error_codes":[70043]}"#);
        let store = FakeStore::with(&me, "rt-old");
        let outcome = restore_in(&http(port), &env(port, &store), &stored(&me, "rt-old"));
        assert!(
            matches!(&outcome, Restore::SignInAgain(m) if *m == fl!("error-sign-in-frequency")),
            "{outcome:?}"
        );
        assert_eq!(store.tokens(), Vec::<String>::new());
    }

    #[test]
    fn interaction_required_keeps_the_saved_sign_in() {
        // An interactive sign-in will replace it; nothing is deleted.
        let me = identity("00000000-0000-0000-0000-0000000000a6");
        let (port, _) = serve_once(
            BAD,
            r#"{"error":"interaction_required","error_codes":[50076]}"#,
        );
        let store = FakeStore::with(&me, "rt-old");
        let outcome = restore_in(&http(port), &env(port, &store), &stored(&me, "rt-old"));
        assert!(matches!(outcome, Restore::SignInAgain(_)), "{outcome:?}");
        assert_eq!(store.tokens(), ["rt-old"]);
    }

    #[test]
    fn a_restore_queued_before_sign_out_does_nothing_after_it() {
        let me = identity("00000000-0000-0000-0000-0000000000f1");
        let store = FakeStore::with(&me, "rt-old");
        // Scheduled, then the sign-out completes before it runs.
        let started = sign_out_count(&account_id(&me));
        delete_in(&env(1, &store), &me).unwrap();
        assert_eq!(store.tokens(), Vec::<String>::new());
        // Port 1: any network attempt would end as Retry, not SignInAgain.
        let outcome = restore_at(&http(1), &env(1, &store), &stored(&me, "rt-old"), started);
        assert!(matches!(outcome, Restore::SignInAgain(_)), "{outcome:?}");
        assert_eq!(
            store.tokens(),
            Vec::<String>::new(),
            "the sign-in came back"
        );
    }

    #[test]
    fn a_renewal_queued_before_sign_out_saves_nothing_after_it() {
        let me = identity("00000000-0000-0000-0000-0000000000f2");
        let store = FakeStore::with(&me, "rt-old");
        let account = signed_in(&me, Some("rt-old"), Duration::ZERO);
        let started = sign_out_count(&account_id(&me));
        delete_in(&env(1, &store), &me).unwrap();
        // A server that would hand out a rotated token, if asked.
        let (port, _) = serve_once("200 OK", OK_ROTATED);
        for force in [false, true] {
            let result = renew_at(
                &http(port),
                &env(port, &store),
                account.clone(),
                force,
                started,
            );
            assert!(
                matches!(result, Err(RenewError::SignInAgain(_))),
                "{result:?}"
            );
        }
        assert_eq!(
            store.tokens(),
            Vec::<String>::new(),
            "a rotated token was saved"
        );
        assert!(newest(&account).is_none(), "its tokens are still known");
    }

    #[test]
    fn a_password_save_queued_before_sign_out_or_forget_writes_nothing() {
        let me = identity("00000000-0000-0000-0000-0000000000f3");
        let id = account_id(&me);
        let pw = Secret::new("hunter2".into());
        let store = FakeStore::default();
        let e = env(1, &store);
        // Normal: saved.
        let ticket = password_ticket(&id, "desk-1");
        assert_eq!(
            save_password_in(&e, &me, "desk-1", "D", &pw, ticket),
            Ok(true)
        );
        // Saved first, deleted by the sign-out after it: gone.
        delete_in(&e, &me).unwrap();
        assert_eq!(*store.passwords.lock().unwrap(), []);
        // Scheduled before the sign-out, run after it: nothing written.
        let other = identity("00000000-0000-0000-0000-0000000000f4");
        let other_id = account_id(&other);
        let ticket = password_ticket(&other_id, "desk-1");
        delete_in(&e, &other).unwrap();
        assert_eq!(
            save_password_in(&e, &other, "desk-1", "D", &pw, ticket),
            Ok(false)
        );
        assert_eq!(*store.passwords.lock().unwrap(), []);
        // Scheduled before Forget, run after it: nothing written, and
        // another desktop's password is untouched.
        let third = identity("00000000-0000-0000-0000-0000000000f5");
        let third_id = account_id(&third);
        let keep = password_ticket(&third_id, "desk-2");
        assert_eq!(
            save_password_in(&e, &third, "desk-2", "D", &pw, keep),
            Ok(true)
        );
        let ticket = password_ticket(&third_id, "desk-1");
        let forget = note_forget(&third_id, "desk-1");
        forget_password_in(&e, &third, "desk-1", forget).unwrap();
        assert_eq!(
            save_password_in(&e, &third, "desk-1", "D", &pw, ticket),
            Ok(false)
        );
        assert_eq!(
            *store.passwords.lock().unwrap(),
            [(third, "desk-2".to_owned())]
        );
    }

    #[test]
    fn a_late_forget_keeps_a_password_saved_after_it() {
        let me = identity("00000000-0000-0000-0000-0000000000f7");
        let id = account_id(&me);
        let pw = Secret::new("hunter2".into());
        let store = FakeStore::default();
        let e = env(1, &store);
        // Forget asked for (its deletion is held up), then the desktop is
        // connected again with "remember", and that save runs first.
        let forget = note_forget(&id, "desk");
        let ticket = password_ticket(&id, "desk");
        assert_eq!(
            save_password_in(&e, &me, "desk", "D", &pw, ticket),
            Ok(true)
        );
        forget_password_in(&e, &me, "desk", forget).unwrap();
        assert_eq!(
            *store.passwords.lock().unwrap(),
            [(me.clone(), "desk".to_owned())]
        );
        // A Forget asked for after that save does delete it.
        let forget = note_forget(&id, "desk");
        forget_password_in(&e, &me, "desk", forget).unwrap();
        assert_eq!(*store.passwords.lock().unwrap(), []);
    }

    #[test]
    fn a_cache_save_queued_before_sign_out_writes_nothing() {
        let id = account_id(&identity("00000000-0000-0000-0000-0000000000f6"));
        let root = std::env::temp_dir().join(format!("deskfeed-cache-race-{}", std::process::id()));
        let store = crate::cache::Store::at(root.clone());
        let list = vec![crate::cache::CachedWorkspace {
            name: "Workspace".into(),
            resources: Vec::new(),
        }];
        // Normal: saved.
        save_cache(&store, &id, &list, sign_out_count(&id)).unwrap();
        assert!(store.load(&id, Duration::from_hours(1)).is_some());
        // Scheduled, then the sign-out (its counter and the removal), then
        // the save runs: nothing comes back.
        let started = sign_out_count(&id);
        note_sign_out(&id);
        store.remove(&id).unwrap();
        save_cache(&store, &id, &list, started).unwrap();
        assert!(
            store.load(&id, Duration::from_hours(1)).is_none(),
            "the offline list came back"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn network_trouble_offers_retry_and_keeps_the_saved_sign_in() {
        let me = identity("00000000-0000-0000-0000-0000000000a7");
        let (port, _) = serve_once("503 Service Unavailable", "busy");
        let store = FakeStore::with(&me, "rt-old");
        let outcome = restore_in(&http(port), &env(port, &store), &stored(&me, "rt-old"));
        assert!(
            matches!(&outcome, Restore::Retry(m) if *m == fl!("error-restore-network")),
            "{outcome:?}"
        );
        assert_eq!(store.tokens(), ["rt-old"]);
    }

    #[test]
    fn a_renewed_id_token_is_never_taken_unchecked() {
        // Its signature must be checked against Microsoft's keys; when they
        // can't be fetched (here the test server answers only the token
        // request), retry later.
        let me = identity("00000000-0000-0000-0000-0000000000a8");
        let (port, _) = serve_once(
            "200 OK",
            r#"{"access_token":"a","token_type":"Bearer","refresh_token":"rt-old","id_token":"x.y.z"}"#,
        );
        let store = FakeStore::default();
        let outcome = restore_in(&http(port), &env(port, &store), &stored(&me, "rt-old"));
        assert!(matches!(outcome, Restore::Retry(_)), "{outcome:?}");
    }

    #[test]
    fn renewal_is_skipped_while_the_token_is_fresh() {
        let me = identity("00000000-0000-0000-0000-0000000000b1");
        let store = FakeStore::default();
        let account = signed_in(&me, Some("rt-old"), Duration::from_secs(3600));
        // Port 1: any request would fail, so success means none was made.
        let renewed = renew_in(&http(1), &env(1, &store), account.clone(), false).unwrap();
        assert!(Arc::ptr_eq(&renewed, &account));
    }

    #[test]
    fn renewal_counts_time_asleep() {
        // The monotonic clock says an hour is left, but the computer slept
        // through it: the wall clock says it has expired.
        let me = identity("00000000-0000-0000-0000-0000000000b7");
        let (port, _) = serve_once("200 OK", OK_ROTATED);
        let store = FakeStore::with(&me, "rt-old");
        let mut account =
            Arc::into_inner(signed_in(&me, Some("rt-old"), Duration::from_secs(3600))).unwrap();
        account.tokens.expires_wall = SystemTime::now() - Duration::from_hours(5);
        let renewed = renew_in(&http(port), &env(port, &store), Arc::new(account), false).unwrap();
        assert_eq!(renewed.tokens.access_token.expose(), "new-access");
    }

    #[test]
    fn forced_renewal_renews_a_fresh_looking_token() {
        let me = identity("00000000-0000-0000-0000-0000000000b8");
        let (port, _) = serve_once("200 OK", OK_ROTATED);
        let store = FakeStore::with(&me, "rt-old");
        let account = signed_in(&me, Some("rt-old"), Duration::from_secs(3600));
        let renewed = renew_in(&http(port), &env(port, &store), account, true).unwrap();
        assert_eq!(renewed.tokens.access_token.expose(), "new-access");
    }

    #[test]
    fn renewal_saves_a_rotated_token() {
        let me = identity("00000000-0000-0000-0000-0000000000b2");
        let (port, _) = serve_once("200 OK", OK_ROTATED);
        let store = FakeStore::with(&me, "rt-old");
        let account = signed_in(&me, Some("rt-old"), Duration::ZERO);
        let renewed = renew_in(&http(port), &env(port, &store), account, false).unwrap();
        assert_eq!(renewed.tokens.access_token.expose(), "new-access");
        assert_eq!(store.tokens(), ["rt-new"]);
    }

    #[test]
    fn renewal_failures_are_explained() {
        let me = identity("00000000-0000-0000-0000-0000000000b3");
        let store = FakeStore::default();
        let (port, _) = serve_once("503 Service Unavailable", "busy");
        let account = signed_in(&me, Some("rt"), Duration::ZERO);
        assert_eq!(
            renew_in(&http(port), &env(port, &store), account, false).unwrap_err(),
            RenewError::Retry(fl!("error-generic"))
        );
        let (port, _) = serve_once(BAD, r#"{"error":"invalid_grant","error_codes":[70008]}"#);
        let account = signed_in(&me, Some("rt"), Duration::ZERO);
        assert_eq!(
            renew_in(&http(port), &env(port, &store), account, false).unwrap_err(),
            RenewError::SignInAgain(fl!("error-session-expired"))
        );
        let (port, _) = serve_once(BAD, r#"{"error":"invalid_grant","error_codes":[70043]}"#);
        let account = signed_in(&me, Some("rt"), Duration::ZERO);
        assert_eq!(
            renew_in(&http(port), &env(port, &store), account, false).unwrap_err(),
            RenewError::SignInAgain(fl!("error-sign-in-frequency"))
        );
        // No refresh token: nothing to renew with, no request.
        let account = signed_in(&me, None, Duration::ZERO);
        assert_eq!(
            renew_in(&http(1), &env(1, &store), account, false).unwrap_err(),
            RenewError::SignInAgain(fl!("error-session-expired"))
        );
    }

    #[test]
    fn a_second_renewal_reuses_the_first_ones_result() {
        // Two connections start with the same expired copy. The first
        // renews (and rotates the refresh token); the second must not send
        // the old refresh token again. The test server answers only once.
        let me = identity("00000000-0000-0000-0000-0000000000b5");
        let (port, sent) = serve_once("200 OK", OK_ROTATED);
        let store = FakeStore::with(&me, "rt-old");
        let copy = signed_in(&me, Some("rt-old"), Duration::ZERO);
        let first = renew_in(&http(port), &env(port, &store), copy.clone(), false).unwrap();
        let second = renew_in(&http(port), &env(port, &store), copy, false).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            sent.lock()
                .unwrap()
                .matches("grant_type=refresh_token")
                .count(),
            1
        );
        assert_eq!(store.tokens(), ["rt-new"]);
    }

    #[test]
    fn signing_out_forgets_the_newest_tokens() {
        let me = identity("00000000-0000-0000-0000-0000000000b6");
        let fresh = signed_in(&me, Some("rt"), Duration::from_secs(3600));
        signed_in_now(&fresh);
        let stale = signed_in(&me, Some("rt"), Duration::ZERO);
        assert!(newest(&stale).is_some());
        note_sign_out(&account_id(&me));
        assert!(newest(&stale).is_none());
        // Late work from before the sign-out records nothing.
        note_tokens(&fresh, sign_out_count(&account_id(&me)) - 1);
        assert!(newest(&stale).is_none());
    }

    fn signed_in_now(account: &Arc<SignedIn>) {
        super::signed_in(account, sign_out_count(&account_id(&account.identity)));
    }

    #[test]
    fn renewal_checks_a_renewed_id_token_too() {
        // Same rule as restoring: the ID token's signature is checked
        // against Microsoft's keys. Here they can't be fetched (the test
        // server answers only the token request), so the renewal is
        // retried later and nothing is saved.
        let me = identity("00000000-0000-0000-0000-0000000000b4");
        let (port, _) = serve_once(
            "200 OK",
            r#"{"access_token":"a","token_type":"Bearer","refresh_token":"rt-new","id_token":"x.y.z"}"#,
        );
        let store = FakeStore::with(&me, "rt-old");
        let account = signed_in(&me, Some("rt-old"), Duration::ZERO);
        assert!(matches!(
            renew_in(&http(port), &env(port, &store), account, false),
            Err(RenewError::Retry(_))
        ));
        assert_eq!(store.tokens(), ["rt-old"]);
    }

    #[test]
    fn work_started_before_a_sign_out_never_saves_afterwards() {
        // Spec §6: late callbacks must not restore the account.
        let me = identity("00000000-0000-0000-0000-0000000000c1");
        let store = FakeStore::default();
        let account = signed_in(&me, Some("rt-late"), Duration::from_secs(3600));
        let started = sign_out_count(&account_id(&me));
        delete_in(&env(1, &store), &me).unwrap();
        assert_eq!(save_locked(&env(1, &store), &account, started), Ok(()));
        assert_eq!(store.tokens(), Vec::<String>::new());
        // A sign-in after the sign-out saves normally.
        let started = sign_out_count(&account_id(&me));
        save_locked(&env(1, &store), &account, started).unwrap();
        assert_eq!(store.tokens(), ["rt-late"]);
    }

    #[test]
    fn sign_out_blocks_late_saves_even_if_the_keyring_fails() {
        let me = identity("00000000-0000-0000-0000-0000000000c2");
        let failing = FakeStore {
            fail: true,
            ..FakeStore::default()
        };
        let before = sign_out_count(&account_id(&me));
        assert_eq!(
            delete_in(&env(1, &failing), &me).unwrap_err(),
            fl!("error-sign-out-incomplete")
        );
        assert_eq!(sign_out_count(&account_id(&me)), before + 1);
    }

    #[test]
    fn loading_saved_sign_ins() {
        let me = identity("00000000-0000-0000-0000-0000000000d1");
        let store = FakeStore::with(&me, "rt");
        let all = load_saved_in(&env(1, &store)).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].identity, me);
        let failing = FakeStore {
            fail: true,
            ..FakeStore::default()
        };
        assert_eq!(
            load_saved_in(&env(1, &failing)).unwrap_err(),
            fl!(
                "error-keyring-load",
                detail = keyring_detail(&FakeStore::err())
            )
        );
    }

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
