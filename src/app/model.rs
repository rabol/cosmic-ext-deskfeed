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
    /// Opaque key: hash of account, tenant and resource ID.
    pub(super) key: String,
    pub(super) title: String,
    pub(super) kind: ResourceKind,
    pub(super) connection_file: Option<Url>,
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
    /// The keyring saved (or failed to save) the sign-in.
    Saved(String, u64, Result<(), String>),
    Refresh(String),
    Discovered(String, u64, Result<Vec<WorkspaceView>, String>),
    SignOut(String),
    SignOutSelected,
    SignOutFinished(Result<(), String>),
    Connect(String),
    /// Account ID, epoch, resource key, and the connection file with the
    /// account (tokens renewed first if needed).
    ConnectionFileReady(
        String,
        u64,
        String,
        Result<(Arc<ConnectionFile>, Arc<SignedIn>), String>,
    ),
    PasswordInput(PasswordText),
    TogglePasswordVisible,
    LogonSelected(usize),
    CustomDomainInput(String),
    DialogConnect,
    DialogCancel,
    /// Account ID, epoch, resource key, event.
    Session(String, u64, String, SessionEvent),
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
pub(super) fn resource_key(account: &SignedIn, tenant: &str, resource_id: &str) -> String {
    let mut h = sha2::Sha256::new();
    for part in [
        account.identity.tenant_id.as_str(),
        account.identity.object_id.as_str(),
        tenant,
        resource_id,
    ] {
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
        Ok(ws) => WorkspaceView {
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
                    key: resource_key(account, &tenant, &r.id),
                    title: r.title,
                    kind: r.kind,
                    connection_file: r.connection_file,
                })
                .collect(),
        },
        Err(e) => WorkspaceView {
            name: fallback,
            error: Some(describe_fetch_error(&e)),
            resources: Vec::new(),
        },
    }
}
