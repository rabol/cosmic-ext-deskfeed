// SPDX-License-Identifier: MIT

//! Tests of the launcher.

use super::*;

fn spec(logon: LogonSetting, password: Option<&str>, entra_file: bool) -> LaunchSpec {
    let text = if entra_file {
        "full address:s:h\r\ngatewayhostname:s:rdgateway.wvd.microsoft.com\r\nenablerdsaadauth:i:1\r\n"
    } else {
        "full address:s:h\r\ngatewayhostname:s:rdgateway.wvd.microsoft.com\r\n"
    };
    LaunchSpec {
        connection: ConnectionFile::parse(text.as_bytes()).unwrap(),
        username: Some("anna@example.com".into()),
        password: password.map(|p| Secret::new(p.into())),
        logon,
        helper: PathBuf::from("/usr/libexec/cosmic-ext-deskfeed/cosmic-ext-deskfeed-signin"),
        profile: PathBuf::from("/tmp/p"),
        gateway_token: None,
        pinned_certificate: None,
        settings: crate::settings::DEFAULTS,
        window_title: String::new(),
        app_id: String::new(),
        account: Some(HelperAccount {
            object_id: "0a1b-2c".into(),
            tenant_id: "3d4e-5f".into(),
            username: Some("anna@example.com".into()),
        }),
    }
}

fn args(s: &LaunchSpec) -> Vec<String> {
    build_args(
        s,
        Path::new("/run/user/1000/cosmic-ext-deskfeed/x/connection.rdp"),
    )
    .unwrap()
}

#[test]
fn password_host_automatic() {
    let a = args(&spec(LogonSetting::Automatic, Some("p@ss w0rd,;"), false));
    assert_eq!(a[0], "/run/user/1000/cosmic-ext-deskfeed/x/connection.rdp");
    assert!(a.contains(&"/gateway:type:arm".into()));
    assert!(a.contains(&"/sec:nla".into()));
    assert!(a.contains(&"/p:p@ss w0rd,;".into()));
    assert!(a.contains(&"/u:anna@example.com".into()));
    assert!(
        a.iter()
            .any(|x| x.starts_with("/azure:auth-helper:/usr/libexec/"))
    );
    // Automatic: no domain at all, so FreeRDP applies `AzureAD`.
    assert!(!a.iter().any(|x| x.starts_with("/d:")));
}

#[test]
fn empty_and_custom_domain() {
    let a = args(&spec(LogonSetting::EmptyDomain, Some("pw"), false));
    assert!(a.contains(&"/d:".into()));
    let a = args(&spec(
        LogonSetting::CustomDomain("CORP".into()),
        Some("pw"),
        false,
    ));
    assert!(a.contains(&"/d:CORP".into()));
}

#[test]
fn entra_sign_in_sends_no_password() {
    for entra_file in [false, true] {
        let a = args(&spec(LogonSetting::EntraSignIn, Some("pw"), entra_file));
        assert!(a.contains(&"/sec:aad".into()));
        assert!(
            !a.iter()
                .any(|x| x.starts_with("/p:") || x.starts_with("/d:"))
        );
    }
}

#[test]
fn a_chosen_password_logon_wins_over_the_files_entra_hint() {
    let a = args(&spec(LogonSetting::EmptyDomain, Some("pw"), true));
    assert!(a.contains(&"/sec:nla".into()));
    assert!(!a.contains(&"/sec:aad".into()));
    assert!(a.contains(&"/p:pw".into()));
    assert!(a.contains(&"/d:".into()));
    let a = args(&spec(LogonSetting::Automatic, Some("pw"), true));
    assert!(a.contains(&"/sec:nla".into()));
    assert!(a.contains(&"/p:pw".into()));
}

#[test]
fn window_title_and_app_id() {
    let mut s = spec(LogonSetting::EntraSignIn, None, false);
    s.window_title = "Standard Desktop – anna@example.com\n/p:x".into();
    s.app_id = "io.github.rabol.cosmic-ext-deskfeed".into();
    let a = args(&s);
    // The line break is dropped, so nothing becomes a separate argument.
    assert!(a.contains(&"/title:Standard Desktop – anna@example.com/p:x".into()));
    assert!(a.contains(&"/wm-class:io.github.rabol.cosmic-ext-deskfeed".into()));
    assert!(!a.iter().any(|x| x == "/p:x"));
    s.app_id = "bad id;x".into();
    assert!(!args(&s).iter().any(|x| x.starts_with("/wm-class:")));
    s.window_title = "\n".into();
    assert!(!args(&s).iter().any(|x| x.starts_with("/title:")));
}

#[test]
fn a_remote_app_file_gets_the_app_options() {
    let mut s = spec(LogonSetting::EntraSignIn, None, false);
    s.settings.fullscreen = true;
    s.connection = ConnectionFile::parse(
        b"full address:s:h\r\ngatewayhostname:s:rdgateway.wvd.microsoft.com\r\n\
          remoteapplicationmode:i:1\r\nremoteapplicationprogram:s:||notepad\r\n",
    )
    .unwrap();
    let a = args(&s);
    assert!(!a.contains(&"+f".into()));
    assert!(a.contains(&"/multimon".into()));
    assert!(a.contains(&"-grab-keyboard".into()));
    // The same settings for a desktop.
    s.connection = spec(LogonSetting::EntraSignIn, None, false).connection;
    assert!(args(&s).contains(&"+f".into()));
}

#[test]
fn only_executable_files_count() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("avd-exec-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("xfreerdp3");
    fs::write(&file, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!is_executable(&file));
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(is_executable(&file));
    assert!(!is_executable(&dir));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn freerdp_gets_the_proxy_the_app_uses() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| (*v).to_owned())
        }
    };
    let set = |p: &str, n: Option<&str>| {
        Ok(FreeRdpProxy {
            https_proxy: Some(p.into()),
            no_proxy: n.map(Into::into),
        })
    };
    assert_eq!(freerdp_proxy(env(&[])), Ok(FreeRdpProxy::default()));
    assert_eq!(
        freerdp_proxy(env(&[("http_proxy", "http://p:3128")])),
        set("http://p:3128", None)
    );
    // The app's order wins: ALL_PROXY before HTTPS_PROXY.
    assert_eq!(
        freerdp_proxy(env(&[
            ("HTTPS_PROXY", "http://h:3128"),
            ("ALL_PROXY", "socks5://s:1080"),
        ])),
        set("socks5://s:1080", None)
    );
    assert_eq!(
        freerdp_proxy(env(&[("HTTP_PROXY", "p:3128")])),
        set("p:3128", None)
    );
    // NO_PROXY before no_proxy, as the app reads them.
    assert_eq!(
        freerdp_proxy(env(&[
            ("https_proxy", "http://p:3128"),
            ("no_proxy", "lower.example"),
            ("NO_PROXY", "upper.example"),
        ])),
        set("http://p:3128", Some("upper.example"))
    );
    // Values the app ignores are ignored here too.
    assert_eq!(
        freerdp_proxy(env(&[("http_proxy", "")])),
        Ok(FreeRdpProxy::default())
    );
    // A scheme FreeRDP can't use: an error, never a direct connection.
    assert_eq!(
        freerdp_proxy(env(&[("http_proxy", "https://p:443")])),
        Err(LaunchError::UnsupportedProxy("https".into()))
    );
}

#[test]
fn gateway_token_only_when_well_formed() {
    let mut s = spec(LogonSetting::EntraSignIn, None, false);
    s.gateway_token = Some(BearerToken::new("eyJ0.eyJh-b_c.sig".into()));
    assert_eq!(args(&s)[1], "/gateway:type:arm,bearer:eyJ0.eyJh-b_c.sig");
    for bad in ["a,type:rpc", "a b", "", "a:b"] {
        s.gateway_token = Some(BearerToken::new(bad.into()));
        assert_eq!(args(&s)[1], "/gateway:type:arm", "{bad:?}");
    }
}

#[test]
fn rejects_passwords_that_would_break_the_pipe() {
    for pw in ["a\nb", "a\rb", "a\0b", ""] {
        let s = spec(LogonSetting::Automatic, Some(pw), false);
        assert_eq!(
            build_args(&s, Path::new("/x.rdp")),
            Err(LaunchError::BadPassword),
            "{pw:?}"
        );
    }
}

/// Run by `children_end_when_the_app_crashes` in a process of its own:
/// starts a long `sleep`, prints its pid, then crashes.
#[test]
#[ignore = "run by children_end_when_the_app_crashes"]
fn crashing_app() {
    if std::env::var("DESKFEED_TEST_CRASH").is_err() {
        return;
    }
    // A child that ignores SIGTERM, as FreeRDP once did (ADR 0020); an
    // ignored signal stays ignored across `exec`.
    let mut cmd = Command::new("sh");
    // No inherited output: the outer test reads until it closes.
    cmd.args(["-c", "trap '' TERM; exec sleep 60"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let child = spawn_tied(cmd).unwrap();
    println!("CHILD {}", child.id());
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    std::process::abort();
}

#[test]
fn children_end_when_the_app_crashes() {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "sessions::launcher::tests::crashing_app",
        ])
        .args(["--nocapture", "--test-threads=1"])
        .env("DESKFEED_TEST_CRASH", "1")
        .output()
        .unwrap();
    assert!(!out.status.success(), "it should have crashed");
    let text = String::from_utf8_lossy(&out.stdout);
    let pid: u32 = text
        .lines()
        .find_map(|l| l.split_once("CHILD ").map(|(_, pid)| pid))
        .unwrap_or_else(|| panic!("no pid in {text}"))
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .parse()
        .unwrap();
    // Gone, or a zombie waiting for its new parent to reap it.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let state = fs::read_to_string(format!("/proc/{pid}/stat")).ok();
        let alive = state
            .as_deref()
            .and_then(|s| s.rsplit(')').next())
            .is_some_and(|rest| !rest.trim_start().starts_with('Z'));
        if !alive {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "sleep {pid} outlived the app"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn handing_over_arguments_never_blocks() {
    // Nobody reads the pipe, and the arguments are bigger than it.
    let (read_end, write_end) = pipe().unwrap();
    let args = vec!["x".repeat(1024 * 1024)];
    let started = std::time::Instant::now();
    assert!(send_args(write_end, &args).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(read_end);
    // Normal arguments fit.
    let (read_end, write_end) = pipe().unwrap();
    send_args(write_end, &["/u:anna".into(), "/p:pw".into()]).unwrap();
    let mut got = String::new();
    fs::File::from(read_end).read_to_string(&mut got).unwrap();
    assert_eq!(got, "/u:anna\n/p:pw\n\n");
    // Too long a password or user name never reaches the pipe.
    let mut s = spec(LogonSetting::Automatic, Some(&"p".repeat(2000)), false);
    assert_eq!(
        build_args(&s, Path::new("/x.rdp")).unwrap_err(),
        LaunchError::BadPassword
    );
    s.password = None;
    s.username = Some("u".repeat(2000));
    assert!(build_args(&s, Path::new("/x.rdp")).is_err());
}

#[test]
fn exit_codes() {
    assert_eq!(classify_exit(Some(0), false, false), SessionEnd::Closed);
    // A lost connection, whatever FreeRDP's exit code says.
    assert_eq!(
        classify_exit(Some(147), false, true),
        SessionEnd::ConnectionLost
    );
    assert_eq!(
        classify_exit(Some(0), false, true),
        SessionEnd::ConnectionLost
    );
    assert_eq!(classify_exit(None, false, true), SessionEnd::Crashed);
    assert_eq!(classify_exit(Some(11), false, false), SessionEnd::Closed);
    assert_eq!(
        classify_exit(Some(134), false, false),
        SessionEnd::LogonFailed
    );
    assert_eq!(
        classify_exit(Some(131), true, false),
        SessionEnd::LogonFailed
    );
    assert_eq!(
        classify_exit(Some(135), false, false),
        SessionEnd::AccountLockedOut
    );
    assert_eq!(
        classify_exit(Some(149), false, false),
        SessionEnd::PasswordExpired
    );
    assert_eq!(
        classify_exit(Some(140), false, false),
        SessionEnd::DnsFailed
    );
    assert_eq!(
        classify_exit(Some(131), false, false),
        SessionEnd::Failed { code: 131 }
    );
    assert_eq!(classify_exit(None, false, false), SessionEnd::Crashed);
    // Another connection took over: never a lost connection.
    assert_eq!(classify_exit(Some(5), false, true), SessionEnd::Replaced);
    assert_eq!(
        classify_exit(Some(1), false, false),
        SessionEnd::DisconnectedByServer
    );
    assert_eq!(classify_exit(Some(12), false, false), SessionEnd::Closed);
}

#[test]
fn reads_a_refused_remote_app_and_a_replaced_session() {
    // As `xf_rail.c` and `errinfo.c` in 3.32.2 print them.
    let mut log = Log::default();
    assert_eq!(log.app_refused, None);
    log.push(
        "[10:01:02:003] [1:2] [ERROR][com.freerdp.client.x11.rail] - [xf_rail_server_execute_result]: \
         RAIL exec error: execResult=RAIL_EXEC_E_NOT_IN_ALLOWLIST [0x00000003] NtError=0x0",
    );
    assert_eq!(log.app_refused, Some(AppRefusal::NotAllowed));
    for (name, why) in [
        ("RAIL_EXEC_E_FILE_NOT_FOUND", AppRefusal::NotFound),
        ("RAIL_EXEC_E_SESSION_LOCKED", AppRefusal::SessionLocked),
        ("RAIL_EXEC_E_FAIL", AppRefusal::Other),
        ("RAIL_EXEC_E_UNKNOWN", AppRefusal::Other),
    ] {
        assert_eq!(
            AppRefusal::from_log(&format!("RAIL exec error: execResult={name} [0x1]")),
            Some(why),
            "{name}"
        );
    }
    assert!(!log.replaced);
    log.push(
        "[10:01:02:003] [1:2] [INFO][com.freerdp.core] - [rdp_print_errinfo]: \
         ERRINFO_DISCONNECTED_BY_OTHER_CONNECTION (0x00000005):Another user connected to the server, \
         forcing the disconnection of the current connection.",
    );
    assert!(log.replaced);
    assert!(!log.proxy_auth);
    // As 3.32.2 printed it against an authenticating test proxy.
    log.push(
        "[12:16:11:369] [1:2] [INFO][com.freerdp.core.proxy] - [http_proxy_connect]: \
         HTTP Proxy: HTTP/1.1 407 Proxy Authentication Required",
    );
    assert!(log.proxy_auth);
    let mut other = Log::default();
    other.push("[INFO][x] - [http_proxy_connect]: HTTP Proxy: HTTP/1.1 200 Connection established");
    assert!(!other.proxy_auth);
}

/// As xfreerdp3 3.32.2 printed it for the local test server's
/// throwaway certificate (research/rdp-test-server).
const REFUSAL: &str = "Certificate details for /tmp/tfreerdp-server.33900:3389 (RDP-Server):
\tCommon Name: (null)
\tSubject:     (null)
\tIssuer:      (null)
\tValid from:  Oct 18 08:01:10 2026 GMT
\tValid to:    Oct 18 08:01:10 2027 GMT
\tThumbprint:  c8:3c:d4:90:e0:4e:ca:86:9d:9e:71:aa:89:09:90:0c:94:da:3a:bf:cf:b8:dc:9a:32:59:09:c7:2b:d9:e4:af
The above X.509 certificate could not be verified, possibly because you do not have
Do you trust the above certificate? (Y/T/N) [15:13:10:299] [675264:000a4dc2] [ERROR][com.freerdp.crypto] - [freerdp_tls_handshake]: certificate not trusted, aborting.";

#[test]
fn reads_a_refused_certificate() {
    let mut log = Log::default();
    for line in REFUSAL.lines() {
        log.push(line);
    }
    // FreeRDP's own log line, read on the other stream: it may come
    // before or after the description.
    log.push("[ERROR][com.freerdp.crypto] - [tls_print_new_certificate_warn]: The fingerprint for the host key sent by the remote host is c8:3c:d4:90:e0:4e:ca:86:9d:9e:71:aa:89:09:90:0c:94:da:3a:bf:cf:b8:dc:9a:32:59:09:c7:2b:d9:e4:af");
    let c = log.refused_certificate().expect("refused");
    assert_eq!(c.host, "/tmp/tfreerdp-server.33900:3389");
    assert_eq!(c.valid_to, "Oct 18 08:01:10 2027 GMT");
    assert!(c.fingerprint.starts_with("c8:3c:d4") && is_sha256_fingerprint(&c.fingerprint));
    assert!(log.stop_now);
    // The refusal first, the description later: still found.
    let mut log = Log::default();
    let mut lines: Vec<&str> = REFUSAL.lines().collect();
    lines.rotate_right(1);
    for line in lines {
        log.push(line);
    }
    assert_eq!(
        log.refused_certificate().unwrap().valid_to,
        "Oct 18 08:01:10 2027 GMT"
    );
    // A malformed thumbprint isn't something to approve.
    let mut log = Log::default();
    for line in REFUSAL.replace("c8:3c", "zz:3c").lines() {
        log.push(line);
    }
    assert!(log.refused_certificate().is_none() && !log.stop_now);
    // Only FreeRDP's log line: the fingerprint, without details.
    let mut log = Log::default();
    log.push("[ERROR][com.freerdp.crypto] - [tls_print_new_certificate_warn]: The fingerprint for the host key sent by the remote host is C8:3C:D4:90:E0:4E:CA:86:9D:9E:71:AA:89:09:90:0C:94:DA:3A:BF:CF:B8:DC:9A:32:59:09:C7:2B:D9:E4:AF");
    log.push(
        "[ERROR][com.freerdp.crypto] - [freerdp_tls_handshake]: certificate not trusted, aborting.",
    );
    let c = log.refused_certificate().unwrap();
    assert!(c.fingerprint.starts_with("c8:3c") && c.issuer.is_empty());
    // Control characters from the network are dropped.
    assert_eq!(clean_field(" a\u{1b}[31mb\t "), "a[31mb");
}

#[test]
fn pinned_certificate_goes_to_freerdp() {
    let fp = "c8:3c:d4:90:e0:4e:ca:86:9d:9e:71:aa:89:09:90:0c:94:da:3a:bf:cf:b8:dc:9a:32:59:09:c7:2b:d9:e4:af";
    let mut s = spec(LogonSetting::EntraSignIn, None, true);
    assert!(!args(&s).iter().any(|a| a.starts_with("/cert")));
    s.pinned_certificate = Some(fp.into());
    assert!(args(&s).contains(&format!("/cert:fingerprint:sha256:{fp}")));
    // Never anything else through this field.
    s.pinned_certificate = Some(format!("{fp},ignore"));
    assert!(!args(&s).iter().any(|a| a.starts_with("/cert")));
}

#[test]
fn log_keeps_errors_and_detects_connection() {
    let mut log = Log::default();
    let step = |from: &str, to: &str| {
        format!(
            "[DEBUG][com.freerdp.core.rdp] - [rdp_client_transition_to_state][0x1]: CONNECTION_STATE_{from} --> CONNECTION_STATE_{to}"
        )
    };
    assert_eq!(log.push("[INFO][com.freerdp.core] - something"), None);
    // Channels load before the session is active: not connected yet.
    assert_eq!(
        log.push(
            "[INFO][com.freerdp.channels.drdynvc.client] - Loading Dynamic Virtual Channel rdpgfx"
        ),
        None
    );
    assert_eq!(
        log.push(&step("NEGO", "AAD")),
        Some(SessionEvent::Stage(Stage::Authenticating))
    );
    assert_eq!(
        log.push(&step("AAD", "MCS_CREATE_REQUEST")),
        Some(SessionEvent::Stage(Stage::Connecting))
    );
    assert_eq!(
        log.push(&step("LICENSING", "MULTITRANSPORT_BOOTSTRAPPING_REQUEST")),
        None
    );
    assert_eq!(
        log.push(&step("FINALIZATION_CLIENT_FONT_MAP", "ACTIVE")),
        Some(SessionEvent::Connected)
    );
    // Only once, and nothing more after it.
    assert_eq!(log.push(&step("NEGO", "NLA")), None);
    assert_eq!(
        log.push(&step("FINALIZATION_CLIENT_FONT_MAP", "ACTIVE")),
        None
    );
    log.lines.clear();
    assert!(!log.lost);
    // As xfreerdp3 3.32.2 printed it when the local test server went away.
    log.push("[14:13:52:196] [664309:000a22f0] [INFO][com.freerdp.client.common] - [client_auto_reconnect_ex]: Network disconnect!");
    assert!(log.lost);
    log.push("[ERROR][com.freerdp.core.rdstls] - RDSTLS_RESULT_LOGON_FAILURE [0x0000052E]");
    assert!(log.logon_failure);
    assert_eq!(log.lines.len(), 1);
    log.push(&step("NEGO", "AAD"));
    log.push("[DEBUG][com.freerdp.core.rdp] - [rdp_recv_pdu]: recv Data PDU, length: 12");
    assert_eq!(log.lines.len(), 2);
    for i in 0..100 {
        log.push(&format!("[WARN] line {i}"));
    }
    assert_eq!(log.lines.len(), MAX_LOG_LINES);
}

/// Runs a fake FreeRDP that records what it received, to check the
/// pipe hand-off, the private connection file and cleanup.
#[test]
fn launch_hands_arguments_over_privately() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("avd-launch-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.txt");
    let fake = dir.join("xfreerdp3");
    fs::write(
        &fake,
        format!(
            "#!/bin/sh\n\
             out='{out}'\n\
             echo \"ARGV:$*\" > \"$out\"\n\
             first=''\n\
             while IFS= read -r line <&3; do [ -z \"$line\" ] && break; [ -z \"$first\" ] && first=\"$line\"; echo \"ARG:$line\" >> \"$out\"; done\n\
             echo \"MODE:$(stat -c %a \"$first\")\" >> \"$out\"\n\
             echo \"DIRMODE:$(stat -c %a \"$(dirname \"$first\")\")\" >> \"$out\"\n\
             grep -q drivestoredirect \"$first\" && echo DRIVES >> \"$out\"\n\
             env | grep -q AVD_TEST_SECRET && echo LEAKED_ENV >> \"$out\"\n\
             env | grep -qi 'pr0xy-pw' && echo PROXY_IN_ENV >> \"$out\"\n\
             env | grep -q '^no_proxy=.corp.example$' && echo NO_PROXY_OK >> \"$out\"\n\
             pf=\"$COSMIC_EXT_DESKFEED_SIGNIN_PROXY_FILE\"\n\
             [ -n \"$pf\" ] && echo \"PROXYFILE:$(stat -c %a \"$pf\"):$(cat \"$pf\")\" >> \"$out\"\n\
             env | grep -q '^COSMIC_EXT_DESKFEED_SIGNIN_PROFILE=/tmp/p$' && echo PROFILE_OK >> \"$out\"\n\
             env | grep -q '^WLOG_FILTER=com.freerdp.core.rdp:DEBUG$' && echo FILTER_OK >> \"$out\"\n\
             env | grep -q '^COSMIC_EXT_DESKFEED_SIGNIN_ACCOUNT=0a1b-2c.3d4e-5f$' && echo ACCOUNT_OK >> \"$out\"\n\
             env | grep -q '^COSMIC_EXT_DESKFEED_SIGNIN_LOGIN_HINT=anna@example.com$' && echo HINT_OK >> \"$out\"\n\
             echo '[DEBUG][com.freerdp.core.rdp] - [rdp_client_transition_to_state][0x1]: CONNECTION_STATE_FINALIZATION_CLIENT_FONT_MAP --> CONNECTION_STATE_ACTIVE'\n\
             exit 0\n",
            out = out.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = dir.join("run");
    fs::create_dir_all(&runtime).unwrap();
    // SAFETY: no other test reads this variable.
    unsafe { std::env::set_var("AVD_TEST_SECRET", "x") };

    let (tx, rx) = std::sync::mpsc::channel();
    let s = spec(LogonSetting::Automatic, Some("hunter2"), false);
    // A proxy with a (synthetic) password: only through the pipe and
    // the private file, never the environment (review F03).
    let proxy = FreeRdpProxy {
        https_proxy: Some("http://anna:pr0xy-pw@proxy.example:3128".into()),
        no_proxy: Some(".corp.example".into()),
    };
    let _session = launch_with(&fake, &runtime, &s, &proxy, move |e| {
        tx.send(e).unwrap();
    })
    .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        SessionEvent::Connected
    );
    let SessionEvent::Ended { end, .. } = rx.recv_timeout(Duration::from_secs(10)).unwrap() else {
        panic!("expected end")
    };
    assert_eq!(end, SessionEnd::Closed);

    let got = fs::read_to_string(&out).unwrap();
    assert!(got.contains("ARGV:/args-from:fd:3\n"), "{got}");
    assert!(
        !got.lines().next().unwrap().contains("hunter2"),
        "password on command line"
    );
    assert!(got.contains("ARG:/p:hunter2"), "{got}");
    assert!(got.contains("ARG:/gateway:type:arm"));
    assert!(got.contains("MODE:600"), "{got}");
    assert!(got.contains("DIRMODE:700"), "{got}");
    assert!(got.contains("PROFILE_OK"), "{got}");
    assert!(got.contains("FILTER_OK"), "{got}");
    assert!(got.contains("ACCOUNT_OK"), "{got}");
    assert!(got.contains("HINT_OK"), "{got}");
    assert!(!got.contains("DRIVES"));
    assert!(!got.contains("LEAKED_ENV"));
    assert!(
        got.contains("ARG:/proxy:http://anna:pr0xy-pw@proxy.example:3128"),
        "{got}"
    );
    assert!(!got.contains("PROXY_IN_ENV"), "{got}");
    assert!(got.contains("NO_PROXY_OK"), "{got}");
    assert!(
        got.contains(
            r#"PROXYFILE:600:{"uri":"http://anna:pr0xy-pw@proxy.example:3128","no_proxy":".corp.example"}"#
        ),
        "{got}"
    );
    // The connection file and its folder are gone.
    assert_eq!(
        fs::read_dir(runtime.join("cosmic-ext-deskfeed"))
            .unwrap()
            .count(),
        0
    );
    fs::remove_dir_all(dir).unwrap();
}

/// FreeRDP's log lines arrive while it runs. `sed` writes through the C
/// library like FreeRDP: through a pipe the line would wait in its
/// buffer until `sleep` ends.
#[test]
fn log_lines_arrive_while_freerdp_runs() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("avd-launch-live-{}", std::process::id()));
    fs::create_dir_all(dir.join("run")).unwrap();
    let fake = dir.join("fake");
    fs::write(
        &fake,
        "#!/bin/sh\ncat <&3 >/dev/null\n\
         { echo '[DEBUG][com.freerdp.core.rdp] - [rdp_client_transition_to_state][0x1]: CONNECTION_STATE_FINALIZATION_CLIENT_FONT_MAP --> CONNECTION_STATE_ACTIVE'; sleep 5; } | sed -n p\n",
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let s = spec(LogonSetting::Automatic, Some("pw"), false);
    let session = launch_with(
        &fake,
        &dir.join("run"),
        &s,
        &FreeRdpProxy::default(),
        move |e| tx.send(e).unwrap(),
    )
    .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        SessionEvent::Connected
    );
    stop_all(&[session], STOP_GRACE);
    fs::remove_dir_all(dir).unwrap();
}

/// The clipboard setting reaches FreeRDP through the connection file
/// (`to_freerdp_text_with`): a `/clipboard:direction-to:` argument alone
/// leaves it off (found 2026-10-20).
#[test]
fn the_clipboard_setting_reaches_the_connection_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("avd-launch-clip-{}", std::process::id()));
    fs::create_dir_all(dir.join("run")).unwrap();
    for (mode, want) in [
        (crate::settings::clipboard::BOTH, "redirectclipboard:i:1"),
        (
            crate::settings::clipboard::TO_DESKTOP,
            "redirectclipboard:i:1",
        ),
        (crate::settings::clipboard::OFF, "redirectclipboard:i:0"),
    ] {
        let out = dir.join("out.txt");
        let _ = fs::remove_file(&out);
        let fake = dir.join("fake");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh\nfirst=''\n\
                 while IFS= read -r line <&3; do [ -z \"$line\" ] && break; [ -z \"$first\" ] && first=\"$line\"; echo \"ARG:$line\" >> '{out}'; done\n\
                 grep redirectclipboard \"$first\" >> '{out}'\nexit 0\n",
                out = out.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let mut s = spec(LogonSetting::EntraSignIn, None, true);
        s.settings.clipboard = mode;
        let (tx, rx) = std::sync::mpsc::channel();
        let _session = launch_with(
            &fake,
            &dir.join("run"),
            &s,
            &FreeRdpProxy::default(),
            move |e| tx.send(e).unwrap(),
        )
        .unwrap();
        loop {
            if let SessionEvent::Ended { .. } = rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                break;
            }
        }
        let got = fs::read_to_string(&out).unwrap();
        assert!(got.lines().any(|l| l.trim_end() == want), "{mode}: {got}");
        assert_eq!(got.matches("redirectclipboard:").count(), 1, "{got}");
        // The direction still comes from the argument.
        if mode == crate::settings::clipboard::TO_DESKTOP {
            assert!(got.contains("ARG:/clipboard:direction-to:remote"), "{got}");
        }
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reports_crash_and_failure_codes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("avd-launch-fail-{}", std::process::id()));
    fs::create_dir_all(dir.join("run")).unwrap();
    for (script, want) in [
        (
            "echo '[ERROR][x] - ERRCONNECT_LOGON_FAILURE [0x00020014]'; exit 131",
            SessionEnd::LogonFailed,
        ),
        ("kill -9 $$", SessionEnd::Crashed),
        // The sign-in helper refused another account; FreeRDP then fails
        // its Entra logon. The refusal is the reason shown (review U1).
        (
            "echo 'cosmic-ext-deskfeed-signin: signed in as a different account; refused' >&2; \
             echo '[ERROR][x] - ERRCONNECT_LOGON_FAILURE [0x00020014]'; exit 131",
            SessionEnd::WrongAccount,
        ),
        // The host didn't finish the logon in time (as on 2026-10-20).
        (
            "echo '[ERROR][com.freerdp.core] - [rdp_client_wait_for_activation]: ERRCONNECT_ACTIVATION_TIMEOUT [0x0002001C]'; exit 131",
            SessionEnd::HostTimedOut,
        ),
        ("exit 140", SessionEnd::DnsFailed),
    ] {
        let fake = dir.join("fake");
        fs::write(&fake, format!("#!/bin/sh\ncat <&3 >/dev/null\n{script}\n")).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let s = spec(LogonSetting::Automatic, Some("pw"), false);
        let _session = launch_with(
            &fake,
            &dir.join("run"),
            &s,
            &FreeRdpProxy::default(),
            move |e| tx.send(e).unwrap(),
        )
        .unwrap();
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            SessionEvent::Ended { end, log } => {
                assert_eq!(end, want, "{script}");
                if want == SessionEnd::LogonFailed {
                    assert_eq!(log.len(), 1);
                }
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    fs::remove_dir_all(dir).unwrap();
}

/// A FreeRDP that ignores SIGTERM and leaves a child behind (like the
/// sign-in helper) is still ended completely, child included.
#[test]
fn stopping_ends_the_whole_process_group() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("avd-launch-stop-{}", std::process::id()));
    fs::create_dir_all(dir.join("run")).unwrap();
    let out = dir.join("child.txt");
    let fake = dir.join("fake");
    fs::write(
        &fake,
        format!(
            "#!/bin/sh
cat <&3 >/dev/null
trap '' TERM
sleep 300 &
echo $! > '{}'
sleep 300
",
            out.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let s = spec(LogonSetting::Automatic, Some("pw"), false);
    let session = launch_with(
        &fake,
        &dir.join("run"),
        &s,
        &FreeRdpProxy::default(),
        move |e| tx.send(e).unwrap(),
    )
    .unwrap();
    let child: i32 = (0..100)
        .find_map(|_| {
            std::thread::sleep(Duration::from_millis(50));
            fs::read_to_string(&out).ok()?.trim().parse().ok()
        })
        .expect("fake started its child");

    stop_all(&[session], Duration::from_millis(300));

    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        SessionEvent::Ended { .. }
    ));
    // The child is gone (or a zombie waiting for init to reap it).
    let gone = (0..40).any(|_| {
        let state = fs::read_to_string(format!("/proc/{child}/stat")).ok();
        if state.is_none_or(|s| s.rsplit(") ").next().is_some_and(|r| r.starts_with('Z'))) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
        false
    });
    assert!(gone, "child {child} still running");
    fs::remove_dir_all(dir).unwrap();
}

/// `launch_with`'s steps for a test host without the Azure gateway:
/// Deskfeed's own arguments minus `/gateway:`, changed by `adjust`.
fn start_without_gateway(
    s: &LaunchSpec,
    dir: &Path,
    adjust: impl FnOnce(Vec<String>) -> Vec<String>,
) -> (Session, std::sync::mpsc::Receiver<SessionEvent>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let freerdp = find_freerdp().expect("xfreerdp3 installed");
    let rundir = RuntimeDir::create(&dir.join("run")).unwrap();
    let rdp_path = write_connection_file(&rundir, &s.connection, false).unwrap();
    let args = build_args(s, &rdp_path)
        .unwrap()
        .into_iter()
        .filter(|a| !a.starts_with("/gateway:"))
        .collect();
    let args = adjust(args);
    let (read_end, write_end) = pipe().unwrap();
    let (terminal, output) = output_terminal().unwrap();
    let child = spawn_freerdp(
        &freerdp,
        &s.profile,
        &read_end,
        output,
        &FreeRdpProxy::default(),
        None,
        None,
    )
    .unwrap();
    drop(read_end);
    send_args(write_end, &args).unwrap();
    let session = supervise(
        child,
        terminal,
        rundir,
        Arc::new(move |e| {
            let _ = tx.send(e);
        }),
    )
    .unwrap();
    (session, rx)
}

/// A RemoteApp from a Windows host with RemoteApp allowed (ADR 0020):
/// `DESKFEED_TEST_RAIL_HOST`, `_USER`, `_PASSWORD`, `_PROGRAM` (a path
/// or `||alias`), optional `_SECS` (default 15). `_EXPECT=missing`
/// expects Windows to refuse a program that isn't there;
/// `_EXPECT=taken` expects another connection (a second run, started
/// within `_SECS`) to take the session over. Checks the launcher's
/// events; the window shows on the running session.
#[test]
#[ignore = "needs a Windows host with RemoteApp allowed"]
fn windows_host_remote_app() {
    let var = |name: &str| std::env::var(format!("DESKFEED_TEST_RAIL_{name}"));
    let host = var("HOST").expect("DESKFEED_TEST_RAIL_HOST");
    let secs: u64 = var("SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(15);
    let dir = std::env::temp_dir().join(format!("avd-rail-{}", std::process::id()));
    fs::create_dir_all(dir.join("run")).unwrap();
    let s = LaunchSpec {
        connection: ConnectionFile::local_test_app(
            &host,
            &var("PROGRAM").expect("DESKFEED_TEST_RAIL_PROGRAM"),
        ),
        username: Some(var("USER").expect("DESKFEED_TEST_RAIL_USER")),
        password: Some(Secret::new(
            var("PASSWORD").expect("DESKFEED_TEST_RAIL_PASSWORD"),
        )),
        logon: LogonSetting::Automatic,
        helper: PathBuf::from("/nonexistent/helper"),
        profile: dir.join("profile"),
        gateway_token: None,
        pinned_certificate: None,
        settings: crate::settings::Effective {
            // Set for a desktop; an app must not get them.
            fullscreen: true,
            system_shortcuts: true,
            scale: crate::display::resolve_percent(0),
            ..crate::settings::DEFAULTS
        },
        window_title: "Deskfeed RemoteApp test".into(),
        app_id: "io.github.rabol.DeskfeedTest".into(),
        account: None,
    };
    // A test host's self-signed certificate.
    let (session, rx) = start_without_gateway(&s, &dir, |mut args| {
        assert!(args.contains(&"/multimon".to_owned()), "{args:?}");
        assert!(!args.contains(&"+f".to_owned()), "{args:?}");
        args.push("/cert:ignore".to_owned());
        args
    });
    let mut events = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    let ended = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(SessionEvent::Ended { end, log }) => break Some((end, log)),
            Ok(e) => {
                println!("event: {e:?}");
                let connected = e == SessionEvent::Connected;
                events.push(e);
                if connected && var("EXPECT").is_err() {
                    break None;
                }
                if connected && var("EXPECT").is_ok_and(|v| v == "taken") {
                    let left = Duration::from_secs(secs);
                    match rx.recv_timeout(left) {
                        Ok(SessionEvent::Ended { end, log }) => break Some((end, log)),
                        other => panic!("not taken over: {other:?}"),
                    }
                }
            }
            Err(e) => panic!("nothing happened in time ({e}): {events:?}"),
        }
    };
    if var("EXPECT").is_ok_and(|v| v == "missing") {
        let (end, log) = ended.expect("an end");
        println!("ended: {end:?}");
        for line in &log {
            println!("  {line}");
        }
        assert_eq!(end, SessionEnd::AppRefused(AppRefusal::NotFound));
    } else if var("EXPECT").is_ok_and(|v| v == "taken") {
        let (end, log) = ended.expect("an end");
        println!("ended: {end:?}");
        for line in log.iter().filter(|l| l.contains("ERRINFO")) {
            println!("  {line}");
        }
        assert!(
            matches!(end, SessionEnd::DisconnectedByServer | SessionEnd::Replaced),
            "{end:?}"
        );
    } else {
        assert!(ended.is_none(), "{ended:?}");
        std::thread::sleep(Duration::from_secs(secs));
        stop_all(&[session], STOP_GRACE);
        let rest: Vec<_> = rx.try_iter().collect();
        println!("after stopping: {rest:?}");
    }
    fs::remove_dir_all(dir).unwrap();
}

/// Real `xfreerdp3` through Deskfeed's own launch path, against the
/// local test server (`research/rdp-test-server/README.md`):
/// `DESKFEED_TEST_SERVER=<its socket> cargo test --release
/// local_test_server_session -- --ignored`. Runs `DESKFEED_TEST_SECS`
/// (default 20) seconds, with the camera when `DESKFEED_TEST_CAMERA=1`.
/// With `DESKFEED_TEST_EXPECT_LOST=1` it instead expects the session to
/// end as a lost connection within that time (stop the server);
/// the window's tiling and the server's resize log
/// are checked from outside.
#[test]
#[ignore = "needs xfreerdp3 and the local test server"]
fn local_test_server_session() {
    let socket = std::env::var("DESKFEED_TEST_SERVER").expect("DESKFEED_TEST_SERVER");
    let secs: u64 = std::env::var("DESKFEED_TEST_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let dir = std::env::temp_dir().join(format!("avd-local-{}", std::process::id()));
    fs::create_dir_all(dir.join("run")).unwrap();
    let s = LaunchSpec {
        connection: ConnectionFile::local_test(&socket),
        username: Some("test".into()),
        password: Some(Secret::new("test".into())),
        logon: LogonSetting::EmptyDomain,
        helper: PathBuf::from("/nonexistent/helper"),
        profile: dir.join("profile"),
        gateway_token: None,
        // `DESKFEED_TEST_PIN=<sha256>`: connect with that approved
        // certificate instead of `/cert:ignore`.
        pinned_certificate: std::env::var("DESKFEED_TEST_PIN").ok(),
        settings: crate::settings::Effective {
            // `DESKFEED_TEST_CAMERA=1`: with the camera channel
            // (research/rdp-test-server logs what the client offers).
            camera: std::env::var("DESKFEED_TEST_CAMERA").is_ok_and(|v| v == "1"),
            // `DESKFEED_TEST_FULLSCREEN=1`: full screen.
            fullscreen: std::env::var("DESKFEED_TEST_FULLSCREEN").is_ok_and(|v| v == "1"),
            // "Automatic", resolved as `app/connect.rs` does.
            scale: crate::display::resolve_percent(0),
            ..crate::settings::DEFAULTS
        },
        // `DESKFEED_TEST_TITLE`: another title, for two sessions at once.
        window_title: std::env::var("DESKFEED_TEST_TITLE")
            .unwrap_or_else(|_| "Deskfeed local test \u{2013} t\u{e9}st".into()),
        app_id: "io.github.rabol.DeskfeedTest".into(),
        account: None,
    };
    // The two arguments the test server can't take: TLS instead of
    // NLA; and without a pin its self-signed certificate is ignored,
    // unless `DESKFEED_TEST_CHECK_CERT=1`.
    let ignore_cert =
        s.pinned_certificate.is_none() && std::env::var("DESKFEED_TEST_CHECK_CERT").is_err();
    let (session, rx) = start_without_gateway(&s, &dir, |args| {
        args.into_iter()
            .map(|a| {
                if a == "/sec:nla" {
                    "/sec:tls".to_owned()
                } else {
                    a
                }
            })
            .chain(ignore_cert.then(|| "/cert:ignore".to_owned()))
            .collect()
    });
    if std::env::var("DESKFEED_TEST_CHECK_CERT").is_ok() {
        // Expect the refusal, ended by the launcher at once.
        let SessionEvent::Ended { end, log } = rx.recv_timeout(Duration::from_secs(15)).unwrap()
        else {
            panic!("expected an end");
        };
        println!("ended: {end:?}");
        for line in &log {
            println!("  {line}");
        }
        assert!(matches!(end, SessionEnd::CertificateNotTrusted(_)));
        stop_all(&[session], STOP_GRACE);
        fs::remove_dir_all(dir).unwrap();
        return;
    }
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(15)).unwrap(),
        SessionEvent::Connected
    );
    if std::env::var("DESKFEED_TEST_EXPECT_LOST").is_ok_and(|v| v == "1") {
        // The server is stopped from outside meanwhile.
        let ended = rx.recv_timeout(Duration::from_secs(secs)).unwrap();
        assert!(
            matches!(
                ended,
                SessionEvent::Ended {
                    end: SessionEnd::ConnectionLost,
                    ..
                }
            ),
            "{ended:?}"
        );
    } else {
        std::thread::sleep(Duration::from_secs(secs));
    }
    stop_all(&[session], STOP_GRACE);
    fs::remove_dir_all(dir).unwrap();
}

/// Real `xfreerdp3`: every property we keep, plus our arguments, must
/// pass FreeRDP's argument parsing. Without `loadbalanceinfo` FreeRDP stops
/// right after parsing, before any network access.
#[test]
#[ignore = "needs xfreerdp3"]
fn real_freerdp_accepts_all_kept_properties() {
    let ints = [
        "gatewayusagemethod:i:1",
        "gatewayprofileusagemethod:i:1",
        "gatewaycredentialssource:i:0",
        "gatewaybrokeringtype:i:1",
        "remoteapplicationmode:i:0",
        "enablerdsaadauth:i:0",
        "enablecredsspsupport:i:1",
        "promptcredentialonce:i:1",
        "targetisaadjoined:i:0",
        "use redirection server name:i:0",
        "audiomode:i:0",
        "videoplaybackmode:i:1",
        "use multimon:i:0",
        "screen mode id:i:2",
        "smart sizing:i:1",
        "dynamic resolution:i:1",
        "autoreconnection enabled:i:1",
    ];
    let strs = [
        "full address:s:127.0.0.1",
        "alternate full address:s:127.0.0.1",
        // Never contacted: FreeRDP stops before the network (see above).
        "gatewayhostname:s:rdgateway.wvd.microsoft.com:443",
        "armpath:s:x",
        "aadtenantid:s:x",
        "geo:s:x",
        "hubdiscoverygeourl:s:https://127.0.0.1/",
        "wvd endpoint pool:s:x",
        "workspace id:s:x",
        "resourceprovider:s:x",
        "remotedesktopname:s:x",
        "kdcproxyname:s:x",
        "remoteapplicationprogram:s:||x",
        "remoteapplicationname:s:x",
        "remoteapplicationcmdline:s:/x \"y z\"",
    ];
    let mut text = ints
        .iter()
        .chain(&strs)
        .copied()
        .collect::<Vec<_>>()
        .join("\r\n");
    text.push_str("\r\n");
    let connection = ConnectionFile::parse(text.as_bytes()).unwrap();
    let dir = std::env::temp_dir().join(format!("avd-real-freerdp-{}", std::process::id()));
    fs::create_dir_all(dir.join("run")).unwrap();
    let s = LaunchSpec {
        connection,
        username: Some("anna@example.com".into()),
        password: Some(Secret::new("pw".into())),
        logon: LogonSetting::EmptyDomain,
        helper: PathBuf::from("/nonexistent/helper"),
        profile: dir.join("profile"),
        gateway_token: None,
        pinned_certificate: None,
        settings: crate::settings::DEFAULTS,
        window_title: String::new(),
        app_id: String::new(),
        account: None,
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let freerdp = find_freerdp().expect("xfreerdp3 installed");
    let _session = launch_with(
        &freerdp,
        &dir.join("run"),
        &s,
        &FreeRdpProxy::default(),
        move |e| tx.send(e).unwrap(),
    )
    .unwrap();
    let SessionEvent::Ended { log, .. } = rx.recv_timeout(Duration::from_secs(30)).unwrap() else {
        panic!("expected end")
    };
    let parse_errors: Vec<_> = log.iter().filter(|l| l.contains("cmdline")).collect();
    assert!(parse_errors.is_empty(), "{parse_errors:#?}");
    assert!(
        log.iter().any(|l| l.contains("loadBalanceInfo")),
        "{log:#?}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn random_freerdp_output_never_breaks_the_reader() {
    let fp = "aa:".repeat(31) + "aa";
    let output = format!(
        "[1:2] [DEBUG][{STATE_LOGGER}] - [x]: CONNECTION_STATE_NEGO --> CONNECTION_STATE_AAD\n\
         Certificate details for host:3389 (RDP-Server):\n\
         \tCommon Name: host\n\tSubject: CN = host\n\tIssuer: CN = host\n\
         \tValid from: Oct 1\n\tValid to: Oct 2\n\tThumbprint: {fp}\n\
         [WARN][x] - The fingerprint for the host key sent by the remote host is {fp}\n\
         [ERROR][x] - certificate not trusted, aborting.\n\
         [ERROR][x] - RAIL exec error: execResult=RAIL_EXEC_E_NOT_IN_ALLOWLIST [0x3]\n\
         [INFO][x] - ERRINFO_DISCONNECTED_BY_OTHER_CONNECTION (0x5):Another user\n\
         [DEBUG][{STATE_LOGGER}] - [x]: X --> {ACTIVE_MARKER}\n"
    );
    crate::test_util::random_inputs("freerdp output", &[output.as_bytes()], |input| {
        let mut log = Log::default();
        for line in String::from_utf8_lossy(input).lines() {
            let _ = log.push(line);
        }
        // Bounded, and what reaches the user is one printable line.
        assert!(log.lines.len() <= MAX_LOG_LINES);
        assert!(log.lines.iter().all(|l| l.chars().count() <= 300));
        if let Some(c) = log.refused_certificate() {
            for field in [
                &c.host,
                &c.common_name,
                &c.subject,
                &c.issuer,
                &c.valid_from,
                &c.valid_to,
                &c.fingerprint,
            ] {
                assert!(field.chars().count() <= 256, "{field:?}");
                assert!(!field.chars().any(char::is_control), "{field:?}");
            }
        }
    });
}
