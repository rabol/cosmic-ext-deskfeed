# Settings inventory

Every value the app fixes in code, and where it belongs. The owner's rule (2026-10-18): anything that could matter to a user is a setting at **App → Account → Connection** (the desktop), each level overriding the one above. Internal values may stay fixed, but are listed here with the reason.

Keep this list current when adding or changing a constant.

Levels: **A** = App (View → Settings), **Ac** = Account, **C** = Connection (desktop). A saved plain RDP connection has no account: its settings are **A → C** (ADR 0029).

## Settings

| Setting | Built-in default | Levels | Where | Status |
|---|---|---|---|---|
| Time to wait for the desktop | 120 s (10–600, steps of 10) | A, Ac, C | `settings.rs` `DEFAULTS` | Done |
| Full screen | off | A, Ac, C | `settings.rs` | Done |
| Use all monitors | off | A, Ac, C | `settings.rs` | Done. RemoteApps always use every monitor (`/multimon`), so their windows can go anywhere (ADR 0020) |
| Open on a new workspace | off | A, Ac, C | `settings.rs` | Done |
| Sound | on | A, Ac, C | `settings.rs` | Done |
| Microphone | off | A, Ac, C | `settings.rs` | Done |
| Camera | off | A, Ac, C | `settings.rs` | Done |
| Clipboard: off, both directions, only to the desktop, only to this computer | off | A, Ac, C | `settings.rs` `Choice::Clipboard` | Done |
| Copy files through the clipboard (in the clipboard's direction) | off | A, Ac, C | `settings.rs` `Toggle::ClipboardFiles` | Done |
| Window size when it opens (share of the monitor) | FreeRDP's 1024 × 768 | A, Ac, C | `settings.rs` `Choice::WindowSize` | Done (ADR 0029 follow-up); not for RemoteApps or full screen |
| Color depth | 32-bit | A, Ac, C | `settings.rs` `Choice::ColorDepth` | Done (ADR 0029 follow-up) |
| Network type | Automatic | A, Ac, C | `settings.rs` `Choice::Network` | Done (ADR 0029 follow-up) |
| Span as one screen (with all monitors in full screen) | off | A, Ac, C | `settings.rs` `Toggle::SpanMonitors` | Done (ADR 0029 follow-up); not tested with two monitors |
| Shared folder | none | C | `config.shared_folders`, `shared_folder.rs` | Done (ADR 0029, shared folder); per desktop or connection only, by choice: sharing a folder is a decision for one machine |
| Keep the screen on in full screen | off | A, Ac, C | `settings.rs` `Toggle::KeepAwake`, `awake.rs` | Done (ADR 0019) |
| Send shortcuts like Super and Alt+Tab to the desktop | on (`+grab-keyboard`) | A, Ac, C | `settings.rs` `Toggle::SystemShortcuts` | Done (ADR 0018) |
| Ctrl+Alt+Enter switches full screen | on (`+toggle-fullscreen`) | A, Ac, C | `settings.rs` `Toggle::FullscreenKey` | Done (ADR 0018 addendum) |
| Ctrl+Alt+M (minimize), Ctrl+Alt+D (disconnect), Right Ctrl (release) | FreeRDP's own | — | — | No switch in FreeRDP 3.32.2; said in the setting's description (ADR 0018 addendum) |
| Reconnect after a lost connection | on | A, Ac, C | `settings.rs` `Toggle::Reconnect` | Done |
| Reconnect attempts | 5 (1–10) | A, Ac, C | `settings.rs` `Number::ReconnectAttempts` | Done |
| Resize the desktop with the window | on (`/dynamic-resolution`) | A, Ac, C | `sessions/launcher.rs` `build_args` | With display scaling (ADR 0015): the alternative, scaling the picture (`/smart-sizing`), gives Windows a fixed 1 024×768 desktop unless a size is set too (`client/X11/xf_client.c`), so it needs a desktop-size setting beside it |
| Refresh desktop lists every | 60 min (15–1 440, steps of 15) | A, Ac | `settings.rs` `Number::Refresh` | Done |
| Time allowed for signing in | 5 min (2–15) | A, Ac | `settings.rs` `Number::SignIn` | Done |
| Keep the offline desktop list for | 30 days (1–90) | A, Ac | `settings.rs` `Number::OfflineDays` | Done |
| Log file size before it rotates | 1 MB (1–100) | A | `log.rs` `MAX_SIZE_MIB`, config `log_max_mib` | Done |
| Printers, smart cards, serial ports, location, WebAuthn | off (forced in the `.rdp`) | A, Ac, C | `sessions/rdpfile.rs` `DEVICE_DEFAULTS` | Later: each needs FreeRDP research and a real-session test before it can be offered (spec §9) |
| Drives, USB devices | off (feed's lists dropped) | A, Ac, C | `sessions/rdpfile.rs` | Later, as above |
| Microsoft cloud (public, government, China) | public Azure | Ac | `service/endpoint.rs`, `service/http.rs` | Later: needs each cloud's endpoints and client registration |
| Display scaling | Automatic (the X11 scale from XSETTINGS); 100–300 % | A, Ac, C | `settings.rs` `Choice::Scale`, `display.rs` | Done (ADR 0015) |
| Theme: match desktop, dark, light | match desktop | A | config `app_theme`, `app/settings_ui.rs` `apply_theme` | Done |
| Keep running in the background (tray icon) | off | A | config `background`, `app/background.rs`, `tray.rs` | Done (ADR 0017) |
| Start when you log in (in the background) | off | A | the autostart entry itself, `autostart.rs` | Done (ADR 0017 addendum) |

## Fixed on purpose

| Value | Where | Why it stays fixed |
|---|---|---|
| Plain RDP connection: port 3389 unless the server field gives one | `connections.rs` `DEFAULT_PORT` | Windows' own default; a different one is typed as `host:port` (ADR 0029). |
| Plain RDP connection: NLA only (`/sec:nla`) | `sessions/launcher/args.rs` | Owner's decision (ADR 0029): no logon screen on the machine without the password check first. |
| Plain RDP connection: host up to 253 characters, name 80, user name 1024 | `connections.rs` `MAX_HOST`, `MAX_NAME`, `MAX_USER_NAME` | Input bounds: a DNS name's limit, a readable title, and far above any real Windows user name; the values go to FreeRDP one per line. |
| Body size limits (token 64 KiB, `.rdp` and keys 256 KiB, icons 256 KiB, discovery 1 MiB, feed 4 MiB) | `service/endpoint.rs` `max_body` | Security bounds (spec §7); a setting could only weaken them |
| HTTP connect 15 s, total 60 s, no redirects | `service/http.rs` | Security bounds; the connection wait is a setting |
| Feed limits: 200 000 XML nodes, depth 64, 64 tenant feeds, 2 000 resources, text 256 chars, IDs 128 chars | `service/feed.rs` | Security bounds |
| `.rdp` limits: 1 000 lines, 4 096-char values; allowed properties | `sessions/rdpfile.rs` | Security bounds and the reviewed property list |
| Icons: PNG, at most 256×256; at most 200 fetched per refresh | `service/icons.rs`, `app/account_flow.rs` | Security bounds |
| Sign-in protocol limits (request, reply and callback 16 KiB, helper line 64 KiB, 16 queued requests, 15 min longest wait) | `auth/`, `signin/src/` | Security bounds |
| Offline list: 1 MB file, 2 000 desktops, 64 workspaces, 256-char names; never URLs or connection files | `cache.rs` | Security and privacy bounds (spec §7, §10) |
| Cookie jar: 512 KiB, 300 cookies | `signin/src/session_jar.rs` | Security bounds |
| Sign-in window ended 30 s after its time if it hangs | `auth/signin.rs` `HELPER_GRACE` | Safety margin after the setting |
| Keyring payload 64 KiB; keyring prompt wait 2 min | `secrets.rs` | Bound; the prompt is the keyring's own |
| ID token clock skew 5 min; token renewal 5 min before expiry; sign-in attempt lifetime 10 min | `auth/` | Protocol safety margins |
| Silent sign-in attempt 20 s; sign-in page wait 180 s (FreeRDP's own); sign-in window shown after 1.5 s | `signin/src/` | Follow FreeRDP and Microsoft behaviour; the overall sign-in time is a setting |
| Waits between reconnect attempts: 5 s, doubling, at most 60 s | `app/decide.rs` `reconnect_delay` | Bounded backoff (spec §8); the number of attempts is the setting |
| FreeRDP's own reconnect off (`-auto-reconnect`) | `sessions/launcher.rs` `build_args` | ADR 0014 |
| Connection deadline = wait + 2 × 180 s + 60 s | `app/connect.rs` | Derived from the wait setting and FreeRDP's sign-in limit |
| FreeRDP ≥ 3.32.2; binaries `xfreerdp3`, `xfreerdp` | `deps.rs`, `sessions/launcher.rs` | The tested backend (ADR 0004) |
| FreeRDP stop grace 5 s; 60 kept log lines; 4 096-char output lines | `sessions/launcher.rs` | Internal process handling |
| Dependency check: 5 s per command, 256 KiB output | `deps.rs` | Internal |
| Tray menu: at most 200 lines | `tray.rs` `MAX_ITEMS` | Bound on what is served; the window lists all |
| No FreeRDP action script (`/action-script:` to a missing file) | `sessions/launcher.rs` `build_args` | Spec §5: the app sets the session's keys, not `~/.config/freerdp/action.sh` |
| Keep-screen-on check every 1 s | `workspace.rs` `WATCH_POLL` | Internal timing; a blank or lock comes after minutes |
| Workspace move: poll 200 ms, wait 3 s for a new workspace | `workspace.rs` | Internal timing |
| Refresh check every 5 min (and after resume) | `app/mod.rs` `REFRESH_CHECK` | Internal; the refresh interval is a setting |
| Log line 2 000 chars; redaction lists | `log.rs` | Internal; privacy |
| FreeRDP log level `info` | `sessions/launcher.rs` `build_args` | Security: at `debug` FreeRDP logs every gateway HTTP header, including `Authorization: Bearer` (`libfreerdp/core/gateway/http.c`), and hex dumps of sign-in tickets (`nla.c`, `tsg.c`), which the log's redaction can't reliably catch (spec §12). View → Diagnostics is the place for more detail |
| Remembered passwords are an opt-in per desktop in the password dialog, not a setting | `secrets.rs`, `app/connect.rs` | Spec §8 asks for a per-desktop choice |
| Never `/cert:ignore`; approved certificates are per desktop, not a setting | `sessions/launcher.rs`, `app/connect.rs` | Security (ADR 0016) |
| Endpoints, client ID, `X-MS-User-Agent`, D-Bus names, app ID | `service/`, `auth/`, `secrets.rs` | Protocol facts (the cloud choice above is the setting) |
