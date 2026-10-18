# Settings inventory

Every value the app fixes in code, and where it belongs. The owner's rule (2026-10-18): anything that could matter to a user is a setting at **App → Account → Connection** (the desktop), each level overriding the one above. Internal values may stay fixed, but are listed here with the reason.

Keep this list current when adding or changing a constant.

Levels: **A** = App (View → Settings), **Ac** = Account, **C** = Connection (desktop).

## Settings

| Setting | Built-in default | Levels | Where | Status |
|---|---|---|---|---|
| Time to wait for the desktop | 120 s (10–600, steps of 10) | A, Ac, C | `settings.rs` `DEFAULTS` | Done |
| Full screen | off | A, Ac, C | `settings.rs` | Done |
| Use all monitors | off | A, Ac, C | `settings.rs` | Done |
| Open on a new workspace | off | A, Ac, C | `settings.rs` | Done |
| Sound | on | A, Ac, C | `settings.rs` | Done |
| Microphone | off | A, Ac, C | `settings.rs` | Done |
| Camera | off | A, Ac, C | `settings.rs` | Done |
| Clipboard | off | A, Ac, C | `settings.rs` | Done |
| Resize the desktop with the window | on (`/dynamic-resolution`) | A, Ac, C | `sessions/launcher.rs` `build_args` | To do |
| FreeRDP log detail | info (`/log-level:info`) | A, Ac, C | `sessions/launcher.rs` `build_args` | To do |
| Refresh desktop lists every | 60 min (15–1 440, steps of 15) | A, Ac | `settings.rs` `Number::Refresh` | Done |
| Time allowed for signing in | 5 min (2–15) | A, Ac | `settings.rs` `Number::SignIn` | Done |
| Log file size before it rotates | 1 MB (1–100) | A | `log.rs` `MAX_SIZE_MIB`, config `log_max_mib` | Done |
| Printers, smart cards, serial ports, location, WebAuthn | off (forced in the `.rdp`) | A, Ac, C | `sessions/rdpfile.rs` `DEVICE_DEFAULTS` | Later: each needs FreeRDP research and a real-session test before it can be offered (spec §9) |
| Drives, USB devices | off (feed's lists dropped) | A, Ac, C | `sessions/rdpfile.rs` | Later, as above |
| Microsoft cloud (public, government, China) | public Azure | Ac | `service/endpoint.rs`, `service/http.rs` | Later: needs each cloud's endpoints and client registration |
| Display scaling | not set | A, Ac, C | — | Roadmap item 5: derived from the monitor first, then an override |
| Light/dark theme | follows the desktop | A | — | Roadmap item 12 |

## Fixed on purpose

| Value | Where | Why it stays fixed |
|---|---|---|
| Body size limits (token 64 KiB, `.rdp` and keys 256 KiB, icons 256 KiB, discovery 1 MiB, feed 4 MiB) | `service/endpoint.rs` `max_body` | Security bounds (spec §7); a setting could only weaken them |
| HTTP connect 15 s, total 60 s, no redirects | `service/http.rs` | Security bounds; the connection wait is a setting |
| Feed limits: 200 000 XML nodes, depth 64, 64 tenant feeds, 2 000 resources, text 256 chars, IDs 128 chars | `service/feed.rs` | Security bounds |
| `.rdp` limits: 1 000 lines, 4 096-char values; allowed properties | `sessions/rdpfile.rs` | Security bounds and the reviewed property list |
| Icons: PNG, at most 256×256; at most 200 fetched per refresh | `service/icons.rs`, `app/account_flow.rs` | Security bounds |
| Sign-in protocol limits (request, reply and callback 16 KiB, helper line 64 KiB, 16 queued requests, 15 min longest wait) | `auth/`, `signin/src/` | Security bounds |
| Cookie jar: 512 KiB, 300 cookies | `signin/src/session_jar.rs` | Security bounds |
| Sign-in window ended 30 s after its time if it hangs | `auth/signin.rs` `HELPER_GRACE` | Safety margin after the setting |
| Keyring payload 64 KiB; keyring prompt wait 2 min | `secrets.rs` | Bound; the prompt is the keyring's own |
| ID token clock skew 5 min; token renewal 5 min before expiry; sign-in attempt lifetime 10 min | `auth/` | Protocol safety margins |
| Silent sign-in attempt 20 s; sign-in page wait 180 s (FreeRDP's own); sign-in window shown after 1.5 s | `signin/src/` | Follow FreeRDP and Microsoft behaviour; the overall sign-in time is a setting |
| Connection deadline = wait + 2 × 180 s + 60 s | `app/connect.rs` | Derived from the wait setting and FreeRDP's sign-in limit |
| FreeRDP ≥ 3.32.2; binaries `xfreerdp3`, `xfreerdp` | `deps.rs`, `sessions/launcher.rs` | The tested backend (ADR 0004) |
| FreeRDP stop grace 5 s; 60 kept log lines; 4 096-char output lines | `sessions/launcher.rs` | Internal process handling |
| Dependency check: 5 s per command, 256 KiB output | `deps.rs` | Internal |
| Workspace move: poll 200 ms, wait 3 s for a new workspace | `workspace.rs` | Internal timing |
| Refresh check every 5 min (and after resume) | `app/mod.rs` `REFRESH_CHECK` | Internal; the refresh interval is a setting |
| Log line 2 000 chars; redaction lists | `log.rs` | Internal; privacy |
| Endpoints, client ID, `X-MS-User-Agent`, D-Bus names, app ID | `service/`, `auth/`, `secrets.rs` | Protocol facts (the cloud choice above is the setting) |
