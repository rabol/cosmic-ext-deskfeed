// SPDX-License-Identifier: MIT

//! FreeRDP's arguments, and the proxy it and the sign-in helper get.

// The launcher's parts share its imports and items (mod.rs).
#[allow(clippy::wildcard_imports)]
use super::*;

/// Longest user name and password handed to FreeRDP, in bytes. Windows
/// allows 256 characters for a password and 104 for a user principal name;
/// these leave room for multi-byte characters. With the 16 KiB gateway
/// token the arguments stay far below a pipe's 64 KiB (`send_args`).
pub(super) const MAX_USER_NAME: usize = 1024;

pub(super) const MAX_PASSWORD: usize = 1024;

/// The variable that tells the sign-in helper where its proxy file is
/// (`signin/src/proxy.rs`); it holds only the path.
pub(super) const PROXY_FILE_ENV: &str = "COSMIC_EXT_DESKFEED_SIGNIN_PROXY_FILE";
/// The expected account for the helper, `<object ID>.<tenant ID>`, and
/// its user name (signin/src/account.rs, same names).
pub(super) const ACCOUNT_ENV: &str = "COSMIC_EXT_DESKFEED_SIGNIN_ACCOUNT";
pub(super) const LOGIN_HINT_ENV: &str = "COSMIC_EXT_DESKFEED_SIGNIN_LOGIN_HINT";

/// The proxy the app's own HTTP client uses: the first set of these, in
/// ureq's order (ureq 3.4.2 `Proxy::try_from_env`).
pub(super) const APP_PROXY_VARS: &[&str] = &[
    "ALL_PROXY",
    "all_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
];

/// The proxy the app itself uses (ureq 3.4.2, `Proxy::try_from_env`): the
/// first of [`APP_PROXY_VARS`] whose value ureq accepts, and `NO_PROXY`
/// before `no_proxy`. `get` reads an environment variable.
pub(super) fn app_proxy(get: impl Fn(&str) -> Option<String>) -> FreeRdpProxy {
    FreeRdpProxy {
        https_proxy: APP_PROXY_VARS
            .iter()
            .filter_map(|name| get(name))
            .find(|value| ureq::Proxy::new(value).is_ok()),
        no_proxy: ["NO_PROXY", "no_proxy"].iter().find_map(|name| get(name)),
    }
    .or_none()
}

/// The proxy FreeRDP gets: exactly the app's (spec §7). FreeRDP 3.32.2
/// accepts `http://`, `socks5://` or no scheme (`proxy_parse_uri`).
///
/// Its address may hold a user name and password, so it never goes into an
/// environment: FreeRDP gets it as `/proxy:` through the
/// private argument pipe, and only `no_proxy` as a variable, which FreeRDP
/// still reads with `/proxy:` set (`libfreerdp/core/proxy.c`,
/// `proxy_prepare`). The sign-in helper reads it from a private file.
pub(super) fn freerdp_proxy(
    get: impl Fn(&str) -> Option<String>,
) -> Result<FreeRdpProxy, LaunchError> {
    let proxy = app_proxy(get);
    let Some(uri) = &proxy.https_proxy else {
        return Ok(proxy);
    };
    let scheme = uri.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        None | Some("http" | "socks5") => Ok(proxy),
        // Connecting directly instead would bypass the proxy the app uses.
        Some(other) => Err(LaunchError::UnsupportedProxy(other.to_owned())),
    }
}

/// The app's proxy; `None`: not set.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct FreeRdpProxy {
    pub(super) https_proxy: Option<String>,
    pub(super) no_proxy: Option<String>,
}

impl FreeRdpProxy {
    /// `no_proxy` alone means nothing.
    pub(super) fn or_none(self) -> Self {
        if self.https_proxy.is_none() {
            return Self::default();
        }
        self
    }

    /// FreeRDP's `/proxy:` option, when there is a proxy; `None` for a
    /// value that would break the argument pipe.
    pub(super) fn arg(&self) -> Option<String> {
        self.https_proxy
            .as_ref()
            .filter(|p| !p.contains(['\n', '\r', '\0']))
            .map(|p| format!("/proxy:{p}"))
    }

    /// Writes the sign-in helper's proxy file (`signin/src/proxy.rs`) into
    /// the private folder `dir`, readable only by the user; `None` without
    /// a proxy.
    pub(super) fn write_file(&self, dir: &Path) -> Result<Option<PathBuf>, LaunchError> {
        let Some(uri) = &self.https_proxy else {
            return Ok(None);
        };
        let text = serde_json::json!({ "uri": uri, "no_proxy": self.no_proxy }).to_string();
        let path = dir.join("proxy.json");
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| f.write_all(text.as_bytes()))
            .map_err(|e| io_err(&e))?;
        Ok(Some(path))
    }
}

/// A private folder holding the sign-in helper's proxy file, for a sign-in
/// the app starts itself; removed when dropped.
pub struct HelperProxy {
    pub(super) _dir: Option<RuntimeDir>,
    pub(super) file: Option<PathBuf>,
}

impl HelperProxy {
    /// The app's proxy for its own sign-in window. Any scheme: the web view
    /// isn't limited like FreeRDP.
    pub fn new() -> Result<Self, LaunchError> {
        let proxy = app_proxy(|name| std::env::var(name).ok());
        if proxy.https_proxy.is_none() {
            return Ok(Self {
                _dir: None,
                file: None,
            });
        }
        let dir = RuntimeDir::create(&runtime_base()?)?;
        let file = proxy.write_file(&dir.0)?;
        Ok(Self {
            _dir: Some(dir),
            file,
        })
    }

    /// Tells the helper where the file is (`PROXY_FILE_ENV`), if any.
    pub fn apply(&self, cmd: &mut Command) {
        if let Some(file) = &self.file {
            cmd.env(PROXY_FILE_ENV, file);
        }
    }
}

/// The argument lines for `/args-from`. `rdp_path` must end in `.rdp`.
pub(super) fn build_args(spec: &LaunchSpec, rdp_path: &Path) -> Result<Vec<String>, LaunchError> {
    let gateway = match spec.gateway_token.as_ref().map(BearerToken::expose) {
        // A JWT: base64url parts and dots. Anything else could break the
        // comma-separated option, so FreeRDP then fetches its own token.
        Some(t)
            if !t.is_empty()
                && t.len() <= 16 * 1024
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) =>
        {
            format!("/gateway:type:arm,bearer:{t}")
        }
        _ => "/gateway:type:arm".to_owned(),
    };
    let mut args = vec![rdp_path.to_string_lossy().into_owned(), gateway];
    // FreeRDP runs `~/.config/freerdp/action.sh` for key combinations it
    // lists, and that can take keys from the session. The app sets the
    // session's keys itself (spec §5), so FreeRDP gets a script path that
    // doesn't exist, in the session's private folder (`client/X11/xf_utils.c`
    // `run_action_script`: no file, no script).
    args.push(format!(
        "/action-script:{}",
        rdp_path.with_file_name("no-action-script").display()
    ));
    // The chosen logon decides, not the file's `enablerdsaadauth`: FreeRDP
    // applies the file first and then the arguments, and a lone `/sec:nla`
    // switches Entra (AAD) security off (3.32.2 `client/common/cmdline.c`).
    let entra = spec.logon == LogonSetting::EntraSignIn;
    args.push(if entra { "/sec:aad" } else { "/sec:nla" }.to_owned());
    args.push(format!("/azure:auth-helper:{}", spec.helper.display()));
    if let Some(user) = &spec.username {
        if user.contains(['\n', '\r', '\0']) || user.len() > MAX_USER_NAME {
            return Err(LaunchError::Io("bad user name".into()));
        }
        args.push(format!("/u:{user}"));
    }
    if !entra {
        match &spec.logon {
            LogonSetting::Automatic | LogonSetting::EntraSignIn => {}
            LogonSetting::EmptyDomain => args.push("/d:".to_owned()),
            LogonSetting::CustomDomain(d) => args.push(format!("/d:{d}")),
        }
        if let Some(pw) = &spec.password {
            let pw = pw.expose();
            if pw.is_empty() || pw.contains(['\n', '\r', '\0']) || pw.len() > MAX_PASSWORD {
                return Err(LaunchError::BadPassword);
            }
            args.push(format!("/p:{pw}"));
        }
    }
    // Only well-formed values; otherwise FreeRDP asks (and refuses), which
    // ends in the approval dialog again.
    if let Some(fp) = spec
        .pinned_certificate
        .as_deref()
        .filter(|f| is_sha256_fingerprint(f))
    {
        args.push(format!("/cert:fingerprint:sha256:{fp}"));
    }
    args.extend([
        "/dynamic-resolution".to_owned(),
        "/log-level:info".to_owned(),
        // The app reconnects, with a fresh connection file and gateway
        // token (ADR 0014). FreeRDP's own loop reuses the first token and
        // shows its own window; this also overrides the feed's
        // `autoreconnection enabled`.
        "-auto-reconnect".to_owned(),
    ]);
    // Display, devices and `/timeout`. FreeRDP waits `TcpAckTimeout`
    // (default 9 s) for the session host to finish activation, including
    // the Entra exchange (`connection.c` `rdp_client_wait_for_activation`);
    // measured on the owner's host it took up to 49.8 s (settings.rs).
    args.extend(if spec.connection.remote_app() {
        spec.settings.freerdp_app_args()
    } else {
        spec.settings.freerdp_args()
    });
    let title = safe_title(&spec.window_title);
    if !title.is_empty() {
        args.push(format!("/title:{title}"));
    }
    if !spec.app_id.is_empty()
        && spec
            .app_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        args.push(format!("/wm-class:{}", spec.app_id));
    }
    if args.iter().any(|a| a.contains(['\n', '\r', '\0'])) {
        return Err(LaunchError::Io("bad argument".into()));
    }
    Ok(args)
}
