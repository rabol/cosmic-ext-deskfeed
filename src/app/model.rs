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
    /// Shown above the desktop list when the keyring couldn't save the
    /// sign-in: it lasts only while the app runs. "Try again" saves again
    /// (spec §6: memory-only or retry).
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
    /// Drawn at each load; spreads the next refresh (`decide::refresh_wait`).
    pub(super) jitter: u32,
    /// When the desktop list last loaded successfully.
    pub(super) last_refreshed: Option<SystemTime>,
    /// The offline list and when it was saved; shown, read-only, until a
    /// live list has loaded in this run.
    pub(super) cached: Option<(SystemTime, Vec<WorkspaceView>)>,
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

    pub(super) fn workspaces(&self) -> &[WorkspaceView] {
        match &self.state {
            AccountState::Ready { workspaces, .. } => workspaces,
            _ => &[],
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
    /// The feed's PNG icon.
    pub(super) icon: Option<Url>,
}

/// A FreeRDP session and the account it belongs to (sign-out ends it even
/// if its desktop has left the list).
pub(super) struct Running {
    /// Its connection attempt; events of other attempts are ignored.
    pub(super) attempt: u64,
    pub(super) account: String,
    pub(super) session: launcher::Session,
    /// Its window's title, to find the window (Show).
    pub(super) title: String,
    /// Started with "Send shortcuts … to the desktop": its row says how
    /// to get them back.
    pub(super) grabs_shortcuts: bool,
    /// Started with "Keep the screen on in full screen".
    pub(super) keep_awake: bool,
    /// A RemoteApp, not a desktop (ADR 0020).
    pub(super) remote_app: bool,
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
    /// Its window's title, as FreeRDP got it (`launcher::safe_title`).
    pub(super) title: String,
    pub(super) grabs_shortcuts: bool,
    pub(super) keep_awake: bool,
    pub(super) remote_app: bool,
}

/// What a sign-out couldn't remove, to try again (spec §6: report that
/// removal is incomplete and offer retry).
#[derive(Debug, Clone)]
pub(super) struct CleanupRetry {
    pub(super) id: String,
    /// The keyring entries (saved sign-in, remembered passwords).
    pub(super) keyring: Option<crate::auth::id_token::Identity>,
    /// The sign-in window's files (cookies).
    pub(super) files: bool,
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
    /// Getting the connection file.
    Preparing,
    /// The password dialog is open.
    AwaitingUser,
    /// FreeRDP is connecting.
    Connecting,
    /// FreeRDP is signing in to Windows (Entra or password).
    Authenticating,
    Connected,
    /// Disconnect was pressed; FreeRDP is ending.
    Disconnecting,
    /// Waiting before reconnect attempt `attempt` of `of`.
    Reconnecting {
        attempt: u32,
        of: u32,
    },
    Failed(String),
}

/// A desktop the app is reconnecting (ADR 0014).
pub(super) struct Reconnect {
    /// Attempts started so far.
    pub(super) attempt: u32,
    /// The pending wait; a wait that isn't this one is stale.
    pub(super) timer: u64,
}

/// An untrusted session host certificate, shown for approval or as
/// blocked (ADR 0016).
pub(super) struct CertificateDialog {
    pub(super) key: String,
    pub(super) title: String,
    pub(super) certificate: launcher::CertificateInfo,
    /// The approved fingerprint it differs from; then it can't be trusted
    /// here.
    pub(super) changed_from: Option<String>,
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
    /// "Remember this password" (off unless the user ticks it).
    pub(super) remember: bool,
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
    /// Icons fetched for an account (account, epoch, URL and PNG bytes).
    IconsLoaded(String, u64, Vec<(Url, Vec<u8>)>),
    /// Saves an account's sign-in to the keyring again.
    RetrySave(String),
    SignOut(String),
    SignOutSelected,
    /// The sign-out question: disconnect its sessions and sign out.
    SignOutConfirm,
    SignOutCancel,
    /// The keyring item of a signed-out account is gone (or not).
    /// The keyring part of a sign-out: the account ID, its identity (to
    /// try again), and how it went.
    SignOutFinished(
        String,
        Box<crate::auth::id_token::Identity>,
        Result<(), String>,
    ),
    /// Sign-out's "Try again": removes what was left (`CleanupRetry`).
    RetrySignOutCleanup,
    DismissSignOutCleanup,
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
    /// The password dialog's "Remember this password".
    ToggleRememberPassword(bool),
    /// A remembered password was read (account, epoch, key, logon
    /// setting, connection file, result).
    SavedPasswordLoaded(
        String,
        u64,
        String,
        LogonSetting,
        Arc<ConnectionFile>,
        Result<Option<Secret>, String>,
    ),
    /// Resource key; whether its password was remembered.
    PasswordSaved(String, Result<(), String>),
    /// Resource key: forget its remembered password.
    ForgetPassword(String),
    /// Trust the certificate in the certificate dialog and connect.
    TrustCertificate,
    /// Close the certificate dialog.
    CloseCertificateDialog,
    /// Resource key: forget its approved certificate.
    ForgetCertificate(String),
    /// Resource key: bring its session's window to the front.
    ShowSession(String),
    /// Outcome of Show (log text).
    SessionShown(Result<(), String>),
    /// Resource key, wait: time for the next reconnect attempt.
    ReconnectDue(String, u64),
    Disconnect(String),
    CheckDependencies,
    DependenciesChecked(Vec<Check>),
    OpenLogFolder,
    /// An account's offline list was read (account, epoch, list).
    CacheLoaded(String, u64, Option<crate::cache::Cached>),
    /// From the tray (background operation).
    Tray(crate::tray::TrayEvent),
    /// Keep the screen on: for the log (`awake.rs`).
    Awake(crate::awake::AwakeEvent),
    /// Deskfeed was started again, with the launcher's activation token.
    StartedAgain(Option<String>),
    /// App settings → Background.
    AppBackground(bool),
    /// App settings → Background → Start when you log in.
    AppAutostart(bool),
    /// Quit dialog: close the window, keep the desktops.
    QuitToBackground,
    /// View → Diagnostics: build the report and show it.
    OpenDiagnostics,
    DiagnosticsReady(String),
    SaveDiagnostics,
    /// Where it was saved, or why not (`None`: cancelled).
    DiagnosticsSaved(Result<String, Option<String>>),
    /// The search field's text.
    SearchInput(String),
    /// Resource key.
    ToggleFavorite(String),
    OpenAccountSettings(String),
    OpenDesktopSettings(String),
    AppToggle(Toggle, bool),
    AppNumber(Number, u32),
    /// Index into `Choice::options`.
    AppChoice(Choice, usize),
    /// 0 follows the app's setting, then `Choice::options` from 1.
    AccountChoice(String, Choice, usize),
    /// 0 follows the account's setting, then `Choice::options` from 1.
    DesktopChoice(String, Choice, usize),
    /// Log rotation size in MiB.
    AppLogSize(u32),
    /// Appearance: index into `THEMES`.
    AppTheme(usize),
    /// Account override: 0 follows the app's setting, 1 on, 2 off.
    AccountToggle(String, Toggle, usize),
    AccountNumber(String, Number, Option<u32>),
    /// Resource key, setting, dropdown choice (0: follow the account).
    DesktopToggle(String, Toggle, usize),
    /// `None`: follow the account.
    DesktopNumber(String, Number, Option<u32>),
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
/// The part of a desktop list the offline cache keeps: names, titles,
/// kinds and keys; no URLs (spec §7, `cache.rs`).
pub(super) fn to_cached(list: &[WorkspaceView]) -> Vec<crate::cache::CachedWorkspace> {
    list.iter()
        .map(|w| crate::cache::CachedWorkspace {
            name: w.name.clone(),
            resources: w
                .resources
                .iter()
                .map(|r| crate::cache::CachedResource {
                    key: r.key.clone(),
                    title: r.title.clone(),
                    kind: match r.kind {
                        ResourceKind::Desktop => "desktop",
                        ResourceKind::RemoteApp => "remoteapp",
                        ResourceKind::Other => "other",
                    }
                    .to_owned(),
                })
                .collect(),
        })
        .collect()
}

/// An offline list as the list view shows it: nothing to connect to.
pub(super) fn from_cached(c: crate::cache::Cached) -> (SystemTime, Vec<WorkspaceView>) {
    let list = c
        .workspaces
        .into_iter()
        .map(|w| WorkspaceView {
            name: w.name,
            error: None,
            resources: w
                .resources
                .into_iter()
                .map(|r| ResourceView {
                    legacy_key: String::new(),
                    kind: match r.kind.as_str() {
                        "desktop" => ResourceKind::Desktop,
                        "remoteapp" => ResourceKind::RemoteApp,
                        _ => ResourceKind::Other,
                    },
                    key: r.key,
                    title: r.title,
                    connection_file: None,
                    icon: None,
                })
                .collect(),
        })
        .collect();
    (c.saved, list)
}

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
                        icon: r.icon_png,
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
                expires_wall: SystemTime::now(),
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
