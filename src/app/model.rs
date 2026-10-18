// SPDX-License-Identifier: MIT

//! What the app keeps per account and per desktop, and how resources are keyed.

use super::*;

pub(super) struct Account {
    /// [`accounts::account_id`].
    pub(super) id: String,
    pub(super) identity: Identity,
    pub(super) state: AccountState,
    /// The saved sign-in this account was restored from, for "Try again".
    pub(super) stored: Option<Stored>,
    /// Shown above the desktop list, for example when the keyring couldn't
    /// save the sign-in.
    pub(super) notice: Option<String>,
    /// The saved sign-in couldn't be renewed for a temporary reason.
    pub(super) can_retry: bool,
    /// Changes on every sign-in attempt and refresh of this account.
    pub(super) generation: u64,
    /// The generation of the latest interactive sign-in; the keyring's
    /// answer for that sign-in carries it.
    pub(super) sign_in: u64,
    /// When the desktop list was last requested (wall clock, so the time
    /// spent suspended counts).
    pub(super) last_load: Option<SystemTime>,
    /// Fixed for the life of this entry; session and connection events
    /// carry it, so events from before a sign-out are ignored.
    pub(super) epoch: u64,
}

pub(super) enum AccountState {
    /// Signing in with the refresh token saved in the keyring.
    Restoring,
    NeedsSignIn {
        error: Option<String>,
    },
    SigningIn {
        cancel: Cancel,
    },
    Ready {
        signed_in: Arc<SignedIn>,
        loading: bool,
        workspaces: Vec<WorkspaceView>,
        error: Option<String>,
    },
}

impl Account {
    pub(super) fn label(&self) -> String {
        self.identity
            .username
            .clone()
            .or_else(|| self.identity.display_name.clone())
            .unwrap_or_else(|| fl!("unnamed-account"))
    }

    pub(super) fn signed_in(&self) -> Option<&Arc<SignedIn>> {
        match &self.state {
            AccountState::Ready { signed_in, .. } => Some(signed_in),
            _ => None,
        }
    }

    pub(super) fn resource(&self, key: &str) -> Option<&ResourceView> {
        match &self.state {
            AccountState::Ready { workspaces, .. } => workspaces
                .iter()
                .flat_map(|w| &w.resources)
                .find(|r| r.key == key),
            _ => None,
        }
    }

    pub(super) fn resource_keys(&self) -> Vec<String> {
        match &self.state {
            AccountState::Ready { workspaces, .. } => workspaces
                .iter()
                .flat_map(|w| &w.resources)
                .map(|r| r.key.clone())
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceView {
    pub(super) name: String,
    pub(super) error: Option<String>,
    pub(super) resources: Vec<ResourceView>,
}

#[derive(Debug, Clone)]
pub struct ResourceView {
    /// Opaque key: hash of account, tenant, workspace and resource ID.
    pub(super) key: String,
    /// The key before workspaces were part of it, to move saved settings.
    pub(super) legacy_key: String,
    pub(super) title: String,
    pub(super) kind: ResourceKind,
    pub(super) connection_file: Option<Url>,
}

/// A FreeRDP session and the account it belongs to (sign-out ends it even
/// if its desktop has left the list).
pub(super) struct Running {
    /// Its connection attempt; events of other attempts are ignored.
    pub(super) attempt: u64,
    pub(super) account: String,
    pub(super) session: launcher::Session,
}

/// A FreeRDP start in progress.
pub(super) struct Launching {
    pub(super) attempt: u64,
    /// How long it may take to connect (spec §8: every attempt ends).
    pub(super) deadline: std::time::Duration,
    pub(super) account: String,
    /// Stop it as soon as it has started (disconnected or signed out
    /// meanwhile).
    pub(super) stop: bool,
}

/// Quitting the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Quit {
    No,
    /// Desktops are open: asking whether to disconnect them and quit.
    Asking,
    /// Ending the sessions; the app exits when they are gone.
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Connection {
    Preparing,
    Connecting,
    Connected,
    Failed(String),
}

pub(super) struct PasswordDialog {
    pub(super) account: String,
    pub(super) key: String,
    pub(super) title: String,
    pub(super) connection: Arc<ConnectionFile>,
    pub(super) password: String,
    pub(super) password_hidden: bool,
    pub(super) logon: usize,
    pub(super) custom_domain: String,
    pub(super) error: Option<String>,
}

/// Text from the password field; never shown in `Debug` output.
#[derive(Clone)]
pub struct PasswordText(pub(super) String);

impl std::fmt::Debug for PasswordText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PasswordText(<redacted>)")
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    LaunchUrl(String),
    ToggleContextPage(ContextPage),
    UpdateConfig(Config),
    DismissNotice,
    /// The saved sign-ins, or why they couldn't be read.
    SavedLoaded(Result<Vec<Stored>, String>),
    /// Account ID, generation, outcome.
    Restored(String, u64, Restore),
    RetryRestore(String),
    AddAccount,
    /// Sign in again to an account in the list.
    SignInAccount(String),
    /// `None`: the sign-in for a new account.
    CancelSignIn(Option<String>),
    /// Account it was for (`None`: a new one), generation, profile used.
    SignInFinished(
        Option<String>,
        u64,
        PathBuf,
        Result<Arc<SignedIn>, SignInFailure>,
    ),
    /// The keyring saved (or failed to save) the sign-in: account ID, the
    /// sign-in's generation, result.
    Saved(String, u64, Result<(), String>),
    Refresh(String),
    /// Periodic check: refresh the desktop lists that are due.
    RefreshDue,
    /// Account ID, epoch, generation; the desktops with the account's
    /// tokens (renewed first if needed).
    Discovered(
        String,
        u64,
        u64,
        Result<(Vec<WorkspaceView>, Arc<SignedIn>), StepError>,
    ),
    SignOut(String),
    SignOutSelected,
    /// The keyring item of a signed-out account is gone (or not).
    SignOutFinished(Result<(), String>),
    /// Account ID: its sign-in window and sessions have ended and its
    /// files are deleted (`Err`: some couldn't be).
    SignOutFilesRemoved(String, Result<(), ()>),
    Connect(String),
    /// Account ID, epoch, resource key, and the connection file with the
    /// account (tokens renewed first if needed).
    ConnectionFileReady(
        String,
        u64,
        String,
        Result<(Arc<ConnectionFile>, Arc<SignedIn>), StepError>,
    ),
    PasswordInput(PasswordText),
    TogglePasswordVisible,
    LogonSelected(usize),
    CustomDomainInput(String),
    DialogConnect,
    DialogCancel,
    /// Account ID, epoch, resource key, attempt; FreeRDP started, or why
    /// not.
    Launched(String, u64, String, u64, Result<launcher::Session, String>),
    /// Account ID, epoch, resource key, attempt, event.
    Session(String, u64, String, u64, SessionEvent),
    /// Resource key, attempt: the time to connect is up.
    ConnectDeadline(String, u64),
    Disconnect(String),
    CheckDependencies,
    DependenciesChecked(Vec<Check>),
    OpenLogFolder,
    OpenAccountSettings(String),
    OpenDesktopSettings(String),
    AccountToggle(String, Toggle, bool),
    AccountTimeout(String, u32),
    /// Resource key, setting, dropdown choice (0: follow the account).
    DesktopToggle(String, Toggle, usize),
    /// `None`: follow the account.
    DesktopTimeout(String, Option<u32>),
    /// Resource key; outcome of "open on a new workspace" (log text).
    WorkspaceMoved(String, Result<(), String>),
    /// The window's close button or the desktop asked the app to close.
    QuitRequested,
    QuitConfirm,
    QuitCancel,
    /// All sessions have ended: exit.
    ReadyToQuit,
}

/// Why a background step for an account failed.
#[derive(Debug, Clone)]
pub enum StepError {
    /// Shown where the step was started (the desktop list or row).
    Show(String),
    /// The sign-in can't be renewed any more: the account needs an
    /// interactive sign-in.
    SignInAgain(String),
}

impl From<accounts::RenewError> for StepError {
    fn from(e: accounts::RenewError) -> Self {
        match e {
            accounts::RenewError::Retry(t) => Self::Show(t),
            accounts::RenewError::SignInAgain(t) => Self::SignInAgain(t),
        }
    }
}

/// Sign-in failures, cheap to clone for messages.
#[derive(Debug, Clone)]
pub struct SignInFailure(pub(super) Option<String>);

pub(super) const LOGON_AUTOMATIC: usize = 0;
pub(super) const LOGON_EMPTY: usize = 1;
pub(super) const LOGON_CUSTOM: usize = 2;
pub(super) const LOGON_ENTRA: usize = 3;

pub(super) fn logon_index(setting: &LogonSetting) -> (usize, String) {
    match setting {
        LogonSetting::Automatic => (LOGON_AUTOMATIC, String::new()),
        LogonSetting::EmptyDomain => (LOGON_EMPTY, String::new()),
        LogonSetting::CustomDomain(d) => (LOGON_CUSTOM, d.clone()),
        LogonSetting::EntraSignIn => (LOGON_ENTRA, String::new()),
    }
}

/// Opaque key for a resource: hash of account, tenant and resource ID.
/// Opaque key for a resource (spec §7): account, tenant, workspace (the
/// feed's publisher ID) and resource ID, so the same resource ID in two
/// workspaces gets two keys.
pub(super) fn resource_key(
    account: &SignedIn,
    tenant: &str,
    workspace: &str,
    resource_id: &str,
) -> String {
    hash_key(&[
        "v2",
        &account.identity.tenant_id,
        &account.identity.object_id,
        tenant,
        workspace,
        resource_id,
    ])
}

/// The key before the workspace was part of it.
pub(super) fn legacy_resource_key(account: &SignedIn, tenant: &str, resource_id: &str) -> String {
    hash_key(&[
        &account.identity.tenant_id,
        &account.identity.object_id,
        tenant,
        resource_id,
    ])
}

fn hash_key(parts: &[&str]) -> String {
    let mut h = sha2::Sha256::new();
    for part in parts {
        h.update(part.as_bytes());
        h.update([0]);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&h.finalize()[..18])
}

/// Moves saved values from legacy keys to the new ones (`(legacy, new)`).
/// A value already under the new key wins. Returns the new map if anything
/// changed.
pub(super) fn migrate_keys<V: Clone + PartialEq>(
    map: &BTreeMap<String, V>,
    pairs: &[(String, String)],
) -> Option<BTreeMap<String, V>> {
    let mut out = map.clone();
    for (legacy, new) in pairs {
        if let Some(value) = map.get(legacy) {
            out.entry(new.clone()).or_insert_with(|| value.clone());
        }
    }
    for (legacy, _) in pairs {
        out.remove(legacy);
    }
    (out != *map).then_some(out)
}

pub(super) fn valid_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 255
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

pub(super) fn workspace_view(account: &SignedIn, feed: discovery::WorkspaceFeed) -> WorkspaceView {
    let fallback = feed
        .tenant
        .workspace_name
        .clone()
        .or_else(|| feed.tenant.tenant_display_name.clone())
        .unwrap_or_else(|| fl!("unnamed-workspace"));
    let tenant = feed.tenant.tenant_id.clone();
    match feed.result {
        Ok(ws) => {
            let workspace = ws.publisher_id.clone().unwrap_or_default();
            WorkspaceView {
                name: if ws.name.is_empty() {
                    fallback
                } else {
                    ws.name
                },
                error: None,
                resources: ws
                    .resources
                    .into_iter()
                    .filter(|r| r.show_by_default)
                    .map(|r| ResourceView {
                        key: resource_key(account, &tenant, &workspace, &r.id),
                        legacy_key: legacy_resource_key(account, &tenant, &r.id),
                        title: r.title,
                        kind: r.kind,
                        connection_file: r.connection_file,
                    })
                    .collect(),
            }
        }
        Err(e) => WorkspaceView {
            name: fallback,
            error: Some(describe_fetch_error(&e)),
            resources: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::token::TokenSet;
    use crate::service::http::BearerToken;

    fn me() -> SignedIn {
        SignedIn {
            identity: Identity {
                tenant_id: "t".into(),
                object_id: "o".into(),
                username: None,
                display_name: None,
            },
            tokens: TokenSet {
                access_token: BearerToken::new("a".into()),
                refresh_token: None,
                id_token: None,
                expires_at: Instant::now(),
            },
        }
    }

    #[test]
    fn the_same_resource_in_two_workspaces_has_two_keys() {
        let a = resource_key(&me(), "tenant", "workspace-a", "res-1");
        let b = resource_key(&me(), "tenant", "workspace-b", "res-1");
        assert_ne!(a, b);
        assert_eq!(a, resource_key(&me(), "tenant", "workspace-a", "res-1"));
        assert_ne!(a, legacy_resource_key(&me(), "tenant", "res-1"));
    }

    #[test]
    fn saved_settings_move_to_the_new_keys() {
        let map: BTreeMap<String, u32> = [
            ("old-1".into(), 1),
            ("old-2".into(), 2),
            ("new-2".into(), 20),
        ]
        .into();
        let pairs = [
            ("old-1".to_owned(), "new-1".to_owned()),
            ("old-2".to_owned(), "new-2".to_owned()),
        ];
        let moved = migrate_keys(&map, &pairs).unwrap();
        // Moved; a value already under the new key wins; legacy keys go.
        assert_eq!(moved, [("new-1".into(), 1), ("new-2".into(), 20)].into());
        // Nothing to move: no change.
        assert_eq!(migrate_keys(&moved, &pairs), None);
    }
}
