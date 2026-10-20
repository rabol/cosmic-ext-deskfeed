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
    /// A new account entry: nothing loaded, refreshed or noted yet.
    pub(super) fn new(id: String, identity: Identity, state: AccountState, epoch: u64) -> Self {
        Self {
            id,
            identity,
            state,
            stored: None,
            notice: None,
            can_retry: false,
            generation: 0,
            sign_in: 0,
            last_load: None,
            jitter: 0,
            last_refreshed: None,
            cached: None,
            epoch,
        }
    }

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
    pub(super) title: String,
    pub(super) kind: ResourceKind,
    pub(super) connection_file: Option<Url>,
    /// The feed's PNG icon.
    pub(super) icon: Option<Url>,
}

/// What a FreeRDP start was asked for, kept while it starts and while it
/// runs.
// Independent facts about one start; a bitflag type would only hide them.
#[allow(clippy::struct_excessive_bools)]
pub(super) struct Start {
    /// Its connection attempt; events of other attempts are ignored.
    pub(super) attempt: u64,
    /// Its account (sign-out ends it even if its desktop has left the
    /// list).
    pub(super) account: String,
    /// Its window's title, as FreeRDP got it (`launcher::safe_title`), to
    /// find the window (Show).
    pub(super) title: String,
    /// Started with "Send shortcuts … to the desktop": its row says how
    /// to get them back.
    pub(super) grabs_shortcuts: bool,
    /// Started with "Keep the screen on in full screen".
    pub(super) keep_awake: bool,
    /// A RemoteApp, not a desktop (ADR 0020).
    pub(super) remote_app: bool,
}

/// A running FreeRDP session.
pub(super) struct Running {
    pub(super) start: Start,
    pub(super) session: launcher::Session,
}

/// A FreeRDP start in progress.
pub(super) struct Launching {
    pub(super) start: Start,
    /// How long it may take to connect (spec §8: every attempt ends).
    pub(super) deadline: std::time::Duration,
    /// Stop it as soon as it has started (disconnected or signed out
    /// meanwhile).
    pub(super) stop: bool,
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

impl Connection {
    /// On its way to connected: getting the file, asking for the password,
    /// FreeRDP connecting or signing in.
    pub(super) fn is_starting(&self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::AwaitingUser | Self::Connecting | Self::Authenticating
        )
    }

    /// The text for a state that is under way, the same on the row and in
    /// the tray; `None` for connected and failed, which each show their
    /// own way.
    pub(super) fn progress_label(&self) -> Option<String> {
        Some(match self {
            Self::Preparing => fl!("status-preparing"),
            Self::AwaitingUser => fl!("status-awaiting-user"),
            Self::Connecting => fl!("status-connecting"),
            Self::Authenticating => fl!("status-authenticating"),
            Self::Disconnecting => fl!("status-disconnecting"),
            Self::Reconnecting { attempt, of } => {
                let (attempt, of) = (*attempt, *of);
                fl!("status-reconnecting", attempt = attempt, of = of)
            }
            Self::Connected | Self::Failed(_) => return None,
        })
    }
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

/// Every message, by the part of the app that handles it (`route`), so
/// the compiler checks that each reaches its handler.
#[derive(Debug, Clone)]
pub enum Message {
    Account(AccountMessage),
    Connection(ConnectionMessage),
    Settings(SettingsMessage),
    Quit(QuitMessage),
    App(AppMessage),
}

/// Accounts: saved sign-ins, signing in and out, desktop lists (`account_flow.rs`).
#[derive(Debug, Clone)]
pub enum AccountMessage {
    /// The saved sign-ins, or why they couldn't be read.
    SavedLoaded(Result<Vec<Stored>, String>),
    /// A saved sign-in was restored at start, or why not.
    Restored {
        account: String,
        generation: u64,
        outcome: Result<(Arc<SignedIn>, Option<String>), StepError>,
    },
    RetryRestore(String),
    AddAccount,
    /// Sign in again to an account in the list.
    SignInAccount(String),
    /// `None`: the sign-in for a new account.
    CancelSignIn(Option<String>),
    /// A sign-in window finished.
    SignInFinished {
        /// The account it was for; `None`: a new one.
        account: Option<String>,
        generation: u64,
        /// The web view profile it used.
        profile: PathBuf,
        result: Result<Arc<SignedIn>, SignInFailure>,
    },
    /// The keyring saved (or failed to save) the sign-in.
    Saved {
        account: String,
        /// The sign-in's generation.
        generation: u64,
        result: Result<(), String>,
    },
    Refresh(String),
    /// Periodic check: refresh the desktop lists that are due.
    RefreshDue,
    /// The account's desktops, with its tokens (renewed first if needed).
    Discovered {
        account: String,
        epoch: u64,
        generation: u64,
        result: Result<(Vec<WorkspaceView>, Arc<SignedIn>), StepError>,
    },
    /// Icons fetched for an account: URL and PNG bytes.
    IconsLoaded {
        account: String,
        epoch: u64,
        icons: Vec<(Url, Vec<u8>)>,
    },
    /// Saves an account's sign-in to the keyring again.
    RetrySave(String),
    /// Signs the account out; it stays listed, signed out.
    SignOut(String),
    SignOutSelected,
    /// Signs the account out and takes it off the list.
    Remove(String),
    /// The sign-out question: disconnect its sessions and sign out.
    SignOutConfirm,
    SignOutCancel,
    /// The keyring part of a sign-out, with the identity to try again.
    SignOutFinished {
        account: String,
        identity: Box<crate::auth::id_token::Identity>,
        result: Result<(), String>,
    },
    /// Sign-out's "Try again": removes what was left (`CleanupRetry`).
    RetrySignOutCleanup,
    DismissSignOutCleanup,
    /// The account's sign-in window and sessions have ended and its files
    /// are deleted (`Err`: some couldn't be).
    SignOutFilesRemoved {
        account: String,
        result: Result<(), ()>,
    },
    /// An account's offline list was read.
    CacheLoaded {
        account: String,
        epoch: u64,
        cached: Option<crate::cache::Cached>,
    },
}

/// Connecting desktops, their dialogs and sessions (`connect.rs`).
#[derive(Debug, Clone)]
pub enum ConnectionMessage {
    Connect(String),
    /// The connection file, with the account's tokens (renewed first if
    /// needed).
    ConnectionFileReady {
        account: String,
        epoch: u64,
        key: String,
        result: Result<(Arc<ConnectionFile>, Arc<SignedIn>), StepError>,
    },
    PasswordInput(PasswordText),
    TogglePasswordVisible,
    LogonSelected(usize),
    CustomDomainInput(String),
    DialogConnect,
    DialogCancel,
    /// FreeRDP started, or why not.
    Launched {
        account: String,
        epoch: u64,
        key: String,
        attempt: u64,
        result: Result<launcher::Session, String>,
    },
    /// An event from a running session.
    Session {
        account: String,
        epoch: u64,
        key: String,
        attempt: u64,
        event: SessionEvent,
    },
    /// The time to connect is up.
    ConnectDeadline {
        key: String,
        attempt: u64,
    },
    /// The password dialog's "Remember this password".
    ToggleRememberPassword(bool),
    /// A remembered password was read.
    SavedPasswordLoaded {
        account: String,
        epoch: u64,
        key: String,
        logon: LogonSetting,
        connection: Arc<ConnectionFile>,
        result: Result<Option<Secret>, String>,
    },
    /// A remembered password's save, with the ticket it was scheduled
    /// under, and whether it was stored.
    PasswordSaved {
        key: String,
        account: String,
        ticket: crate::accounts::PasswordTicket,
        result: Result<bool, String>,
    },
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
    /// Time for the next reconnect attempt, if `wait` is still current.
    ReconnectDue {
        key: String,
        wait: u64,
    },
    Disconnect(String),
    /// "Open on a new workspace" finished. `stop`: that wait's stop flag,
    /// to tell it from a newer wait.
    WorkspaceMoved {
        key: String,
        stop: Arc<std::sync::atomic::AtomicBool>,
        result: Result<(), String>,
    },
}

/// The settings pages (`settings_ui.rs`).
#[derive(Debug, Clone)]
pub enum SettingsMessage {
    /// App settings → Background.
    AppBackground(bool),
    /// App settings → Background → Start when you log in.
    AppAutostart(bool),
    OpenAccountSettings(String),
    OpenDesktopSettings(String),
    /// A connection setting changed on the app's, an account's or a
    /// desktop's page.
    Edit(Level, settings::Edit),
    /// Log rotation size in MiB.
    AppLogSize(u32),
    /// Appearance: index into `THEMES`.
    AppTheme(usize),
}

/// Whose connection settings: built-in defaults → app → account →
/// desktop, each overriding the one above (`settings.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    App,
    /// Account ID.
    Account(String),
    /// Resource key.
    Desktop(String),
}

/// Quitting (`quit.rs`).
#[derive(Debug, Clone)]
pub enum QuitMessage {
    /// Quit dialog: close the window, keep the desktops.
    ToBackground,
    /// The window's close button or the desktop asked the app to close.
    Requested,
    /// Quit from the menu, Ctrl+Q or the tray: quits even with background
    /// operation on (asking first while desktops are open).
    Chosen,
    Confirm,
    Cancel,
    /// All sessions have ended: exit.
    ReadyToQuit,
}

/// App-wide: dependencies, the context drawer, links, notices, the log, the tray.
#[derive(Debug, Clone)]
pub enum AppMessage {
    LaunchUrl(String),
    ToggleContextPage(ContextPage),
    UpdateConfig(Box<Config>),
    DismissNotice,
    CheckDependencies,
    DependenciesChecked(Vec<Check>),
    OpenLogFolder,
    /// From the tray (background operation).
    Tray(crate::tray::TrayEvent),
    /// Keep the screen on: for the log (`awake.rs`).
    Awake(crate::awake::AwakeEvent),
    /// Deskfeed was started again, with the launcher's activation token.
    StartedAgain(Option<String>),
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
}

/// Leaving an account: signing out (it stays listed, signed out) or
/// removing it from the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leaving {
    SignOut,
    Remove,
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
                    kind: r.kind,
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
                    kind: r.kind,
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

fn hash_key(parts: &[&str]) -> String {
    let mut h = sha2::Sha256::new();
    for part in parts {
        h.update(part.as_bytes());
        h.update([0]);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&h.finalize()[..18])
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

    #[test]
    fn connection_states_under_way() {
        use Connection::*;
        let all = [
            Preparing,
            AwaitingUser,
            Connecting,
            Authenticating,
            Connected,
            Disconnecting,
            Reconnecting { attempt: 2, of: 5 },
            Failed("x".into()),
        ];
        let starting: Vec<bool> = all.iter().map(Connection::is_starting).collect();
        // Only the steps toward connected; not disconnecting or waiting
        // to reconnect.
        assert_eq!(
            starting,
            [true, true, true, true, false, false, false, false]
        );
        // A label for everything under way, the same text the row and the
        // tray show; connected and failed show their own way.
        for c in &all {
            let label = c.progress_label();
            assert_eq!(
                label.is_some(),
                !matches!(c, Connected | Failed(_)),
                "{c:?}"
            );
        }
        let reconnecting = all[6].progress_label().unwrap();
        assert!(
            reconnecting.contains('2') && reconnecting.contains('5'),
            "{reconnecting}"
        );
        assert_ne!(Preparing.progress_label(), Connecting.progress_label());
    }

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
    }
}
