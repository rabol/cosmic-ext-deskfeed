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

/// Signed-in accounts' shared state and the work on their saved sign-ins:
/// saving, deleting and renewing tokens, remembered passwords and the
/// offline list. Per account, one renewal or keyring write at a time
/// (spec §6), and work scheduled before a sign-out never saves afterwards
/// (spec §6: late callbacks must not restore the account).
pub struct Accounts {
    /// The client identity (its token endpoint and keyring attributes).
    config: ClientConfig,
    /// Per account ID.
    state: Mutex<HashMap<String, AccountState>>,
}

#[derive(Default)]
struct AccountState {
    /// Held during a renewal or keyring write for this account. Per
    /// account, so one account waiting (for example on a keyring prompt)
    /// doesn't hold up another's.
    busy: Arc<Mutex<()>>,
    /// Sign-outs so far; work records the count it was scheduled under.
    sign_outs: u64,
    /// The newest tokens (spec §6: a renewal must not reuse a refresh
    /// token another renewal already replaced). Connections and refreshes
    /// each hold their own copy; renewals look here first.
    latest: Option<Arc<SignedIn>>,
    /// Per desktop: "Forget password" so far, like `sign_outs`.
    forgets: HashMap<String, u64>,
    /// Per desktop: the forget count the stored password was saved under.
    /// A Forget whose deletion runs after a newer save (one scheduled after
    /// that Forget) leaves the newer password alone.
    saved_after: HashMap<String, u64>,
}

/// What a remembered-password save was scheduled under: the account's
/// sign-outs and the desktop's forgets at that moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordTicket {
    sign_outs: u64,
    forgets: u64,
}

/// When to renew an account's tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renewal {
    /// When the access token expires within the margin.
    IfNeeded,
    /// Now: the service refused an access token that looked valid
    /// (revoked, or the clocks disagree).
    Now,
}

/// The app's accounts, with Microsoft's public Remote Desktop client.
pub fn shared() -> &'static Accounts {
    static SHARED: LazyLock<Accounts> =
        LazyLock::new(|| Accounts::new(ClientConfig::public_remote_desktop()));
    &SHARED
}

impl Accounts {
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            state: Mutex::default(),
        }
    }

    /// Runs `f` on the account's state.
    fn with<R>(&self, id: &str, f: impl FnOnce(&mut AccountState) -> R) -> R {
        let mut all = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(all.entry(id.to_owned()).or_default())
    }

    /// The account's lock for renewals and keyring writes.
    fn busy(&self, id: &str) -> Arc<Mutex<()>> {
        self.with(id, |a| a.busy.clone())
    }

    pub fn sign_out_count(&self, id: &str) -> u64 {
        self.with(id, |a| a.sign_outs)
    }

    /// From now on, work that started earlier for this account saves nothing
    /// and its newest tokens are forgotten.
    pub fn note_sign_out(&self, id: &str) {
        self.with(id, |a| {
            a.sign_outs += 1;
            a.latest = None;
        });
    }

    /// Records `account`'s tokens as the newest, unless newer ones are known
    /// or the account was signed out since `started`.
    fn note_tokens(&self, account: &Arc<SignedIn>, started: u64) {
        self.with(&account_id(&account.identity), |a| {
            let newer = a
                .latest
                .as_ref()
                .is_none_or(|known| account.tokens.expires_at >= known.tokens.expires_at);
            if a.sign_outs == started && newer {
                a.latest = Some(account.clone());
            }
        });
    }

    /// The newest tokens known for the account, if newer than `account`'s.
    fn newest(&self, account: &Arc<SignedIn>) -> Option<Arc<SignedIn>> {
        self.with(&account_id(&account.identity), |a| {
            a.latest
                .clone()
                .filter(|known| known.tokens.expires_at > account.tokens.expires_at)
        })
    }

    /// An interactive sign-in finished: its tokens are the newest.
    pub fn signed_in(&self, account: &Arc<SignedIn>, started: u64) {
        self.note_tokens(account, started);
    }

    /// Saves the account's refresh token, unless it was signed out since
    /// `started` (a [`Self::sign_out_count`]). The error is the notice to
    /// show.
    pub fn save(&self, account: &SignedIn, started: u64) -> Result<(), String> {
        let busy = self.busy(&account_id(&account.identity));
        let _busy = busy.lock();
        self.save_while_busy(account, started)
    }

    /// [`Self::save`], for a caller that holds the account's lock.
    fn save_while_busy(&self, account: &SignedIn, started: u64) -> Result<(), String> {
        if self.sign_out_count(&account_id(&account.identity)) != started {
            crate::app_log!("saved sign-in: not stored (signed out meanwhile)");
            return Ok(());
        }
        let Some(refresh_token) = &account.tokens.refresh_token else {
            crate::app_log!("saved sign-in: not stored (no refresh token in the response)");
            return Err(fl!("notice-not-saved", detail = "no refresh token"));
        };
        let result = secrets::save(&self.config, &account.identity, refresh_token);
        // Diagnostics: outcome and error kind only, never tokens or IDs.
        match &result {
            Ok(()) => crate::app_log!("saved sign-in: stored"),
            Err(e) => crate::app_log!("saved sign-in: not stored ({e})"),
        }
        result.map_err(|e| fl!("notice-not-saved", detail = keyring_detail(&e)))
    }

    /// Signs the account out: from now on its tokens aren't saved, and its
    /// keyring items (sign-in and remembered passwords) are deleted.
    pub fn delete(&self, identity: &Identity) -> Result<(), String> {
        let id = account_id(identity);
        self.note_sign_out(&id);
        let busy = self.busy(&id);
        let _busy = busy.lock();
        let deleted = secrets::delete(&self.config, identity);
        crate::app_log!("saved sign-in: deleted (sign-out): {deleted:?}");
        // Under the same lock as their saves (`save_password`).
        let passwords = secrets::delete_passwords(identity, None);
        crate::app_log!(
            "remembered passwords deleted (sign-out): {:?}",
            passwords.is_ok()
        );
        deleted
            .and(passwords)
            .map_err(|_| fl!("error-sign-out-incomplete"))
    }

    /// Removes an account's cookies, RAM cookie folder and offline desktop
    /// list (sign-out, spec §6: credentials and account caches). Under the
    /// account's lock, like the offline list's saves.
    pub fn remove_files(&self, id: &str) -> Result<(), ()> {
        let busy = self.busy(id);
        let _busy = busy.lock();
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

    /// Saves the account's offline list, unless it was signed out since
    /// `started`. Under the account's lock, like [`Self::remove_files`], so
    /// a save queued before a sign-out can't bring the list back after it.
    pub fn save_cache(
        &self,
        store: &crate::cache::Store,
        id: &str,
        list: &[crate::cache::CachedWorkspace],
        started: u64,
    ) -> Result<(), String> {
        let busy = self.busy(id);
        let _busy = busy.lock();
        if self.sign_out_count(id) != started {
            crate::app_log!("offline desktop list: not saved (signed out meanwhile)");
            return Ok(());
        }
        store.save(id, list)
    }

    pub fn password_ticket(&self, id: &str, desktop: &str) -> PasswordTicket {
        self.with(id, |a| PasswordTicket {
            sign_outs: a.sign_outs,
            forgets: a.forgets.get(desktop).copied().unwrap_or(0),
        })
    }

    /// The user forgot this desktop's password: saves scheduled earlier
    /// write nothing. Call at once, before deleting; give the result to
    /// [`Self::forget_password`].
    pub fn note_forget(&self, id: &str, desktop: &str) -> u64 {
        self.with(id, |a| {
            let n = a.forgets.entry(desktop.to_owned()).or_insert(0);
            *n += 1;
            *n
        })
    }

    /// Remembers a Windows password, unless the account signed out or the
    /// password was forgotten since `ticket`. `Ok(false)`: not saved for
    /// that reason. Under the account's lock, like the deletions.
    pub fn save_password(
        &self,
        identity: &Identity,
        desktop: &str,
        title: &str,
        password: &Secret,
        ticket: PasswordTicket,
    ) -> Result<bool, String> {
        let id = account_id(identity);
        let busy = self.busy(&id);
        let _busy = busy.lock();
        if self.password_ticket(&id, desktop) != ticket {
            crate::app_log!("remembered password: not stored (signed out or forgotten meanwhile)");
            return Ok(false);
        }
        secrets::save_password(identity, desktop, title, password)
            .map_err(|e| keyring_detail(&e))?;
        self.with(&id, |a| {
            a.saved_after.insert(desktop.to_owned(), ticket.forgets);
        });
        Ok(true)
    }

    /// Deletes one desktop's remembered password. `forget`: what
    /// [`Self::note_forget`] returned for this Forget.
    pub fn forget_password(
        &self,
        identity: &Identity,
        desktop: &str,
        forget: u64,
    ) -> Result<(), String> {
        let id = account_id(identity);
        let busy = self.busy(&id);
        let _busy = busy.lock();
        // A password saved after this Forget was asked for is the user's
        // newer choice: keep it.
        let newer = self.with(&id, |a| {
            a.saved_after
                .get(desktop)
                .is_some_and(|&saved| saved >= forget)
        });
        if newer {
            return Ok(());
        }
        secrets::delete_passwords(identity, Some(desktop)).map_err(|e| keyring_detail(&e))
    }
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

impl Accounts {
    /// All saved sign-ins, or the text to show if the keyring can't be read.
    pub fn load_saved(&self) -> Result<Vec<Stored>, String> {
        match secrets::load_all(&self.config) {
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

    /// Signs in with a saved refresh token. `started`:
    /// [`Self::sign_out_count`] when this was scheduled, so a restore
    /// queued before a sign-out does nothing after it.
    pub fn restore(&self, http: &Http, stored: &Stored, started: u64) -> Restore {
        let config = &self.config;
        let id = account_id(&stored.identity);
        let busy = self.busy(&id);
        let _busy = busy.lock();
        // Signed out since this was scheduled: no network, no saving.
        if self.sign_out_count(&id) != started {
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
                let deleted = secrets::delete(config, &stored.identity);
                crate::app_log!("saved sign-in: deleted (invalid_grant): {deleted:?}");
                // AADSTS70043 "BadTokenDueToSignInFrequency": the
                // organization's Conditional Access asks for a new sign-in
                // after a set time (Microsoft Entra error code reference).
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
            Err(IdentityCheck::KeysUnavailable) => {
                return Restore::Retry(fl!("error-restore-network"));
            }
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
            self.save_while_busy(&account, started).err()
        } else {
            None
        };
        self.note_tokens(&account, started);
        Restore::Ready(account, notice)
    }

    /// Renews the tokens (see [`Renewal`]) and saves a rotated refresh
    /// token. `started`: [`Self::sign_out_count`] when the work was
    /// scheduled.
    pub fn renew(
        &self,
        http: &Http,
        account: Arc<SignedIn>,
        when: Renewal,
        started: u64,
    ) -> Result<Arc<SignedIn>, RenewError> {
        let force = when == Renewal::Now;
        if !force && !account.tokens.needs_refresh(Instant::now()) {
            return Ok(account);
        }
        let busy = self.busy(&account_id(&account.identity));
        let _busy = busy.lock();
        // Signed out since this was scheduled: no network, no saving.
        if self.sign_out_count(&account_id(&account.identity)) != started {
            return Err(RenewError::SignInAgain(fl!("error-session-expired")));
        }
        // Another renewal may have finished while this one waited for the
        // lock: use its result, or at least its (rotated) refresh token.
        let refused = account.tokens.access_token.expose().to_owned();
        let account = match self.newest(&account) {
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
        let tokens = match token::refresh(http, &self.config, refresh_token) {
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
        let identity = match renewed_identity(http, &self.config, &tokens, &account.identity) {
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
            let _ = self.save_while_busy(&renewed, started);
        }
        let renewed = Arc::new(renewed);
        self.note_tokens(&renewed, started);
        Ok(renewed)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::fake::{Answer, FakeKeyring};
    use crate::service::endpoint::EndpointPolicy;
    use crate::service::http::BearerToken;
    use crate::test_util::token_server;
    use std::time::Duration;
    use url::Url;

    /// Accounts whose token endpoint is a local test server on `port`.
    /// Their keyring is this thread's fake one, if a test started it;
    /// without one, every keyring call fails as with no Secret Service.
    fn accounts(port: u16) -> Accounts {
        let mut config = ClientConfig::public_remote_desktop();
        config.authority = Url::parse(&format!("http://127.0.0.1:{port}/organizations")).unwrap();
        Accounts::new(config)
    }

    /// A fake keyring holding `token` for `identity`.
    fn keyring_with(a: &Accounts, identity: &Identity, token: &str) -> FakeKeyring {
        let keyring = FakeKeyring::start(Answer::Unlock);
        secrets::save(&a.config, identity, &Secret::new(token.into())).unwrap();
        keyring
    }

    /// The saved refresh tokens.
    fn tokens(a: &Accounts) -> Vec<String> {
        secrets::load_all(&a.config)
            .unwrap()
            .iter()
            .map(|s| s.refresh_token.expose().to_owned())
            .collect()
    }

    /// Remembered passwords: (account object ID, desktop).
    fn passwords(keyring: &FakeKeyring) -> Vec<(String, String)> {
        let mut all: Vec<_> = keyring
            .items()
            .into_iter()
            .filter(|i| {
                i.attributes
                    .get("type")
                    .is_some_and(|t| t == "windows-password")
            })
            .map(|i| {
                (
                    i.attributes["account"].clone(),
                    i.attributes["desktop"].clone(),
                )
            })
            .collect();
        all.sort();
        all
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

    /// `a.restore`, scheduled now.
    fn restore_now(a: &Accounts, port: u16, stored: &Stored) -> Restore {
        a.restore(
            &http(port),
            stored,
            a.sign_out_count(&account_id(&stored.identity)),
        )
    }

    /// `a.renew`, scheduled now.
    fn renew_now(
        a: &Accounts,
        port: u16,
        account: Arc<SignedIn>,
        when: Renewal,
    ) -> Result<Arc<SignedIn>, RenewError> {
        let started = a.sign_out_count(&account_id(&account.identity));
        a.renew(&http(port), account, when, started)
    }

    fn unavailable() -> String {
        keyring_detail(&SecretsError::Unavailable(String::new()))
    }

    const OK_SAME: &str = r#"{"access_token":"new-access","token_type":"Bearer","expires_in":3600,"refresh_token":"rt-old"}"#;
    const OK_ROTATED: &str = r#"{"access_token":"new-access","token_type":"Bearer","expires_in":3600,"refresh_token":"rt-new"}"#;
    const BAD: &str = "400 Bad Request";

    #[test]
    fn restore_without_rotation_saves_nothing() {
        let me = identity("00000000-0000-0000-0000-0000000000a1");
        let server = token_server("200 OK", OK_SAME);
        let port = server.port;
        let a = accounts(port);
        let _keyring = FakeKeyring::start(Answer::Unlock);
        match restore_now(&a, port, &stored(&me, "rt-old")) {
            Restore::Ready(account, notice) => {
                assert_eq!(account.identity, me);
                assert!(notice.is_none());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(tokens(&a), Vec::<String>::new());
    }

    #[test]
    fn restore_saves_a_rotated_token() {
        let me = identity("00000000-0000-0000-0000-0000000000a2");
        let server = token_server("200 OK", OK_ROTATED);
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let outcome = restore_now(&a, port, &stored(&me, "rt-old"));
        assert!(matches!(outcome, Restore::Ready(_, None)), "{outcome:?}");
        assert_eq!(tokens(&a), ["rt-new"]);
        // The renewal used the saved token.
        assert!(server.sent().contains("refresh_token=rt-old"));
    }

    #[test]
    fn restore_reports_when_the_rotated_token_cant_be_saved() {
        let me = identity("00000000-0000-0000-0000-0000000000a3");
        let server = token_server("200 OK", OK_ROTATED);
        let port = server.port;
        // No keyring.
        match restore_now(&accounts(port), port, &stored(&me, "rt-old")) {
            // Signed in all the same, with a notice.
            Restore::Ready(_, Some(notice)) => {
                assert_eq!(notice, fl!("notice-not-saved", detail = unavailable()));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn invalid_saved_sign_in_is_deleted() {
        let me = identity("00000000-0000-0000-0000-0000000000a4");
        let server = token_server(BAD, r#"{"error":"invalid_grant","error_codes":[70008]}"#);
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let outcome = restore_now(&a, port, &stored(&me, "rt-old"));
        assert!(
            matches!(&outcome, Restore::SignInAgain(m) if *m == fl!("error-saved-sign-in-expired")),
            "{outcome:?}"
        );
        assert_eq!(tokens(&a), Vec::<String>::new());
    }

    #[test]
    fn sign_in_frequency_is_explained() {
        let me = identity("00000000-0000-0000-0000-0000000000a5");
        let server = token_server(BAD, r#"{"error":"invalid_grant","error_codes":[70043]}"#);
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let outcome = restore_now(&a, port, &stored(&me, "rt-old"));
        assert!(
            matches!(&outcome, Restore::SignInAgain(m) if *m == fl!("error-sign-in-frequency")),
            "{outcome:?}"
        );
        assert_eq!(tokens(&a), Vec::<String>::new());
    }

    #[test]
    fn interaction_required_keeps_the_saved_sign_in() {
        // An interactive sign-in will replace it; nothing is deleted.
        let me = identity("00000000-0000-0000-0000-0000000000a6");
        let server = token_server(
            BAD,
            r#"{"error":"interaction_required","error_codes":[50076]}"#,
        );
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let outcome = restore_now(&a, port, &stored(&me, "rt-old"));
        assert!(matches!(outcome, Restore::SignInAgain(_)), "{outcome:?}");
        assert_eq!(tokens(&a), ["rt-old"]);
    }

    #[test]
    fn a_restore_queued_before_sign_out_does_nothing_after_it() {
        let me = identity("00000000-0000-0000-0000-0000000000f1");
        // Port 1: any network attempt would end as Retry, not SignInAgain.
        let a = accounts(1);
        let _keyring = keyring_with(&a, &me, "rt-old");
        // Scheduled, then the sign-out completes before it runs.
        let started = a.sign_out_count(&account_id(&me));
        a.delete(&me).unwrap();
        assert_eq!(tokens(&a), Vec::<String>::new());
        let outcome = a.restore(&http(1), &stored(&me, "rt-old"), started);
        assert!(matches!(outcome, Restore::SignInAgain(_)), "{outcome:?}");
        assert_eq!(tokens(&a), Vec::<String>::new(), "the sign-in came back");
    }

    #[test]
    fn a_renewal_queued_before_sign_out_saves_nothing_after_it() {
        let me = identity("00000000-0000-0000-0000-0000000000f2");
        // A server that would hand out a rotated token, if asked.
        let server = token_server("200 OK", OK_ROTATED);
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let account = signed_in(&me, Some("rt-old"), Duration::ZERO);
        let started = a.sign_out_count(&account_id(&me));
        a.delete(&me).unwrap();
        for when in [Renewal::IfNeeded, Renewal::Now] {
            let result = a.renew(&http(port), account.clone(), when, started);
            assert!(
                matches!(result, Err(RenewError::SignInAgain(_))),
                "{result:?}"
            );
        }
        assert_eq!(
            tokens(&a),
            Vec::<String>::new(),
            "a rotated token was saved"
        );
        assert!(a.newest(&account).is_none(), "its tokens are still known");
    }

    #[test]
    fn a_password_save_queued_before_sign_out_or_forget_writes_nothing() {
        let me = identity("00000000-0000-0000-0000-0000000000f3");
        let id = account_id(&me);
        let pw = Secret::new("hunter2".into());
        let a = accounts(1);
        let keyring = FakeKeyring::start(Answer::Unlock);
        // Normal: saved.
        let ticket = a.password_ticket(&id, "desk-1");
        assert_eq!(a.save_password(&me, "desk-1", "D", &pw, ticket), Ok(true));
        // Saved first, deleted by the sign-out after it: gone.
        a.delete(&me).unwrap();
        assert_eq!(passwords(&keyring), []);
        // Scheduled before the sign-out, run after it: nothing written.
        let other = identity("00000000-0000-0000-0000-0000000000f4");
        let other_id = account_id(&other);
        let ticket = a.password_ticket(&other_id, "desk-1");
        a.delete(&other).unwrap();
        assert_eq!(
            a.save_password(&other, "desk-1", "D", &pw, ticket),
            Ok(false)
        );
        assert_eq!(passwords(&keyring), []);
        // Scheduled before Forget, run after it: nothing written, and
        // another desktop's password is untouched.
        let third = identity("00000000-0000-0000-0000-0000000000f5");
        let third_id = account_id(&third);
        let keep = a.password_ticket(&third_id, "desk-2");
        assert_eq!(a.save_password(&third, "desk-2", "D", &pw, keep), Ok(true));
        let ticket = a.password_ticket(&third_id, "desk-1");
        let forget = a.note_forget(&third_id, "desk-1");
        a.forget_password(&third, "desk-1", forget).unwrap();
        assert_eq!(
            a.save_password(&third, "desk-1", "D", &pw, ticket),
            Ok(false)
        );
        assert_eq!(
            passwords(&keyring),
            [(third.object_id.clone(), "desk-2".to_owned())]
        );
    }

    #[test]
    fn a_late_forget_keeps_a_password_saved_after_it() {
        let me = identity("00000000-0000-0000-0000-0000000000f7");
        let id = account_id(&me);
        let pw = Secret::new("hunter2".into());
        let a = accounts(1);
        let keyring = FakeKeyring::start(Answer::Unlock);
        // Forget asked for (its deletion is held up), then the desktop is
        // connected again with "remember", and that save runs first.
        let forget = a.note_forget(&id, "desk");
        let ticket = a.password_ticket(&id, "desk");
        assert_eq!(a.save_password(&me, "desk", "D", &pw, ticket), Ok(true));
        a.forget_password(&me, "desk", forget).unwrap();
        assert_eq!(
            passwords(&keyring),
            [(me.object_id.clone(), "desk".to_owned())]
        );
        // A Forget asked for after that save does delete it.
        let forget = a.note_forget(&id, "desk");
        a.forget_password(&me, "desk", forget).unwrap();
        assert_eq!(passwords(&keyring), []);
    }

    #[test]
    fn a_cache_save_queued_before_sign_out_writes_nothing() {
        let id = account_id(&identity("00000000-0000-0000-0000-0000000000f6"));
        let a = accounts(1);
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::cache::Store::at(tmp.path().to_path_buf());
        let list = vec![crate::cache::CachedWorkspace {
            name: "Workspace".into(),
            resources: Vec::new(),
        }];
        // Normal: saved.
        a.save_cache(&store, &id, &list, a.sign_out_count(&id))
            .unwrap();
        assert!(store.load(&id, Duration::from_hours(1)).is_some());
        // Scheduled, then the sign-out (its counter and the removal), then
        // the save runs: nothing comes back.
        let started = a.sign_out_count(&id);
        a.note_sign_out(&id);
        store.remove(&id).unwrap();
        a.save_cache(&store, &id, &list, started).unwrap();
        assert!(
            store.load(&id, Duration::from_hours(1)).is_none(),
            "the offline list came back"
        );
    }

    #[test]
    fn network_trouble_offers_retry_and_keeps_the_saved_sign_in() {
        let me = identity("00000000-0000-0000-0000-0000000000a7");
        let server = token_server("503 Service Unavailable", "busy");
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let outcome = restore_now(&a, port, &stored(&me, "rt-old"));
        assert!(
            matches!(&outcome, Restore::Retry(m) if *m == fl!("error-restore-network")),
            "{outcome:?}"
        );
        assert_eq!(tokens(&a), ["rt-old"]);
    }

    #[test]
    fn a_renewed_id_token_is_never_taken_unchecked() {
        // Its signature must be checked against Microsoft's keys; when they
        // can't be fetched (here the test server answers only the token
        // request), retry later.
        let me = identity("00000000-0000-0000-0000-0000000000a8");
        let server = token_server(
            "200 OK",
            r#"{"access_token":"a","token_type":"Bearer","refresh_token":"rt-old","id_token":"x.y.z"}"#,
        );
        let port = server.port;
        let outcome = restore_now(&accounts(port), port, &stored(&me, "rt-old"));
        assert!(matches!(outcome, Restore::Retry(_)), "{outcome:?}");
    }

    #[test]
    fn renewal_is_skipped_while_the_token_is_fresh() {
        let me = identity("00000000-0000-0000-0000-0000000000b1");
        let account = signed_in(&me, Some("rt-old"), Duration::from_secs(3600));
        // Port 1: any request would fail, so success means none was made.
        let renewed = renew_now(&accounts(1), 1, account.clone(), Renewal::IfNeeded).unwrap();
        assert!(Arc::ptr_eq(&renewed, &account));
    }

    #[test]
    fn renewal_counts_time_asleep() {
        // The monotonic clock says an hour is left, but the computer slept
        // through it: the wall clock says it has expired.
        let me = identity("00000000-0000-0000-0000-0000000000b7");
        let server = token_server("200 OK", OK_ROTATED);
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let mut account =
            Arc::into_inner(signed_in(&me, Some("rt-old"), Duration::from_secs(3600))).unwrap();
        account.tokens.expires_wall = SystemTime::now() - Duration::from_hours(5);
        let renewed = renew_now(&a, port, Arc::new(account), Renewal::IfNeeded).unwrap();
        assert_eq!(renewed.tokens.access_token.expose(), "new-access");
    }

    #[test]
    fn forced_renewal_renews_a_fresh_looking_token() {
        let me = identity("00000000-0000-0000-0000-0000000000b8");
        let server = token_server("200 OK", OK_ROTATED);
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let account = signed_in(&me, Some("rt-old"), Duration::from_secs(3600));
        let renewed = renew_now(&a, port, account, Renewal::Now).unwrap();
        assert_eq!(renewed.tokens.access_token.expose(), "new-access");
    }

    #[test]
    fn renewal_saves_a_rotated_token() {
        let me = identity("00000000-0000-0000-0000-0000000000b2");
        let server = token_server("200 OK", OK_ROTATED);
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let account = signed_in(&me, Some("rt-old"), Duration::ZERO);
        let renewed = renew_now(&a, port, account, Renewal::IfNeeded).unwrap();
        assert_eq!(renewed.tokens.access_token.expose(), "new-access");
        assert_eq!(tokens(&a), ["rt-new"]);
    }

    #[test]
    fn renewal_failures_are_explained() {
        let me = identity("00000000-0000-0000-0000-0000000000b3");
        let server = token_server("503 Service Unavailable", "busy");
        let port = server.port;
        let account = signed_in(&me, Some("rt"), Duration::ZERO);
        assert_eq!(
            renew_now(&accounts(port), port, account, Renewal::IfNeeded).unwrap_err(),
            RenewError::Retry(fl!("error-generic"))
        );
        let server = token_server(BAD, r#"{"error":"invalid_grant","error_codes":[70008]}"#);
        let port = server.port;
        let account = signed_in(&me, Some("rt"), Duration::ZERO);
        assert_eq!(
            renew_now(&accounts(port), port, account, Renewal::IfNeeded).unwrap_err(),
            RenewError::SignInAgain(fl!("error-session-expired"))
        );
        let server = token_server(BAD, r#"{"error":"invalid_grant","error_codes":[70043]}"#);
        let port = server.port;
        let account = signed_in(&me, Some("rt"), Duration::ZERO);
        assert_eq!(
            renew_now(&accounts(port), port, account, Renewal::IfNeeded).unwrap_err(),
            RenewError::SignInAgain(fl!("error-sign-in-frequency"))
        );
        // No refresh token: nothing to renew with, no request.
        let account = signed_in(&me, None, Duration::ZERO);
        assert_eq!(
            renew_now(&accounts(1), 1, account, Renewal::IfNeeded).unwrap_err(),
            RenewError::SignInAgain(fl!("error-session-expired"))
        );
    }

    #[test]
    fn a_second_renewal_reuses_the_first_ones_result() {
        // Two connections start with the same expired copy. The first
        // renews (and rotates the refresh token); the second must not send
        // the old refresh token again: only one renewal request is sent.
        let me = identity("00000000-0000-0000-0000-0000000000b5");
        let server = token_server("200 OK", OK_ROTATED);
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let copy = signed_in(&me, Some("rt-old"), Duration::ZERO);
        let first = renew_now(&a, port, copy.clone(), Renewal::IfNeeded).unwrap();
        let second = renew_now(&a, port, copy, Renewal::IfNeeded).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(server.sent().matches("grant_type=refresh_token").count(), 1);
        assert_eq!(tokens(&a), ["rt-new"]);
    }

    #[test]
    fn signing_out_forgets_the_newest_tokens() {
        let me = identity("00000000-0000-0000-0000-0000000000b6");
        let a = accounts(1);
        let fresh = signed_in(&me, Some("rt"), Duration::from_secs(3600));
        a.signed_in(&fresh, a.sign_out_count(&account_id(&me)));
        let stale = signed_in(&me, Some("rt"), Duration::ZERO);
        assert!(a.newest(&stale).is_some());
        a.note_sign_out(&account_id(&me));
        assert!(a.newest(&stale).is_none());
        // Late work from before the sign-out records nothing.
        a.note_tokens(&fresh, a.sign_out_count(&account_id(&me)) - 1);
        assert!(a.newest(&stale).is_none());
    }

    #[test]
    fn renewal_checks_a_renewed_id_token_too() {
        // Same rule as restoring: the ID token's signature is checked
        // against Microsoft's keys. Here they can't be fetched (the test
        // server answers only the token request), so the renewal is
        // retried later and nothing is saved.
        let me = identity("00000000-0000-0000-0000-0000000000b4");
        let server = token_server(
            "200 OK",
            r#"{"access_token":"a","token_type":"Bearer","refresh_token":"rt-new","id_token":"x.y.z"}"#,
        );
        let port = server.port;
        let a = accounts(port);
        let _keyring = keyring_with(&a, &me, "rt-old");
        let account = signed_in(&me, Some("rt-old"), Duration::ZERO);
        assert!(matches!(
            renew_now(&a, port, account, Renewal::IfNeeded),
            Err(RenewError::Retry(_))
        ));
        assert_eq!(tokens(&a), ["rt-old"]);
    }

    #[test]
    fn work_started_before_a_sign_out_never_saves_afterwards() {
        // Spec §6: late callbacks must not restore the account.
        let me = identity("00000000-0000-0000-0000-0000000000c1");
        let a = accounts(1);
        let _keyring = FakeKeyring::start(Answer::Unlock);
        let account = signed_in(&me, Some("rt-late"), Duration::from_secs(3600));
        let started = a.sign_out_count(&account_id(&me));
        a.delete(&me).unwrap();
        assert_eq!(a.save(&account, started), Ok(()));
        assert_eq!(tokens(&a), Vec::<String>::new());
        // A sign-in after the sign-out saves normally.
        let started = a.sign_out_count(&account_id(&me));
        a.save(&account, started).unwrap();
        assert_eq!(tokens(&a), ["rt-late"]);
    }

    #[test]
    fn sign_out_blocks_late_saves_even_if_the_keyring_fails() {
        let me = identity("00000000-0000-0000-0000-0000000000c2");
        // No keyring.
        let a = accounts(1);
        let before = a.sign_out_count(&account_id(&me));
        assert_eq!(a.delete(&me).unwrap_err(), fl!("error-sign-out-incomplete"));
        assert_eq!(a.sign_out_count(&account_id(&me)), before + 1);
    }

    #[test]
    fn loading_saved_sign_ins() {
        let me = identity("00000000-0000-0000-0000-0000000000d1");
        let a = accounts(1);
        let keyring = keyring_with(&a, &me, "rt");
        let all = a.load_saved().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].identity, me);
        // No keyring any more.
        drop(keyring);
        assert_eq!(
            a.load_saved().unwrap_err(),
            fl!("error-keyring-load", detail = unavailable())
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
        let a = accounts(1);
        a.note_sign_out("test-a");
        assert_eq!(a.sign_out_count("test-a"), 1);
        assert_eq!(a.sign_out_count("test-b"), 0);
    }
}
