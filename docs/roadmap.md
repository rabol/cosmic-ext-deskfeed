# Roadmap: what's still missing

Status on 2026-10-18, after 0.1.0 (commit `cb552a5`). Checked against [specs.md](../specs.md), the review's coverage table (CODE_REVIEW.md, written before this week's fixes) and the current code. The spec asks for outstanding work to be tracked openly (§3).

Working rules for this list: research the docs, source and APIs first; test locally where possible (`research/rdp-test-server`), and ask the owner for a real-tenant test only as the final check.

## Required for the first usable release (spec §3)

1. ~~**Resource icons.**~~ Done (2026-10-18): the feed's PNG, fetched in the background through the shared HTTP policy (256 KiB, no redirects), header-checked to at most 256×256 before display (`service/icons.rs`), cached per account and URL, cleared on sign-out. Stock icon until it loads. Checked with the owner's feed: 3 of 3 loaded. The `.ico` variant isn't used.
2. ~~**Search, favorites, last successful refresh time**~~ Done (2026-10-18): search by desktop or workspace name, favorites (heart button, "Favorites" section, stored per desktop key), "Updated 14:05" / "Showing the list from …" after a failed refresh. Waiting for the owner's look in the real app.
3. ~~**Offline metadata cache**~~ Done (2026-10-18, `cache.rs`): per account, names, titles, kinds and keys only (no URLs, no connection files); 0700/0600, atomic, versioned, bounded; kept 30 days by default (App/Account setting); deleted on sign-out and for accounts no longer saved. Shown read-only, "Offline list from 14:05", until the live list loads; Connect stays off. Waiting for the owner's test.
4. **Reconnect** after connection loss with bounded backoff, visible status and a stop action: done (2026-10-18, [ADR 0014](adr/0014-reconnect-by-the-app.md)); needs a real-session test (network drop, suspend/resume). **Focus an existing session**: done (2026-10-18), a Show button on connected desktops (ADR 0011), checked with two real local sessions.
5. ~~**Display scaling that follows the monitor's scale**~~ Done (2026-10-18, [ADR 0015](adr/0015-display-scaling-from-xsettings.md)): Automatic reads COSMIC's X11 scale (XSETTINGS `Xft/DPI`) and sends it as `/scale-desktop`; a "Display scaling" setting overrides it. The local test server received 125 %. Waiting for the owner's look at a real desktop.
6. ~~**Explicit session states**~~ Done (2026-10-18): preparing, waiting for your password, connecting, signing in to Windows (FreeRDP's `AAD`/`NLA` steps, as in 15 real AVD connections in the owner's log), connected (now at `--> CONNECTION_STATE_ACTIVE`, checked with the local test server), reconnecting, disconnecting, failed; disconnected and cancelled show as the row's Connect again. Not shown: the moment Microsoft's sign-in window waits for the user (FreeRDP runs it; the app doesn't see it).
7. ~~**Certificate approval**~~ Done (2026-10-18, [ADR 0016](adr/0016-session-host-certificates.md)): AVD's broker pins the session host certificate (FreeRDP checks it); when that doesn't apply, a dialog shows identity and fingerprint for approval, the approved fingerprint is pinned per desktop, and a changed one is blocked. Checked with the local test server: unknown, approved and changed certificates.
8. ~~**Opt-in password remembering**~~ Done (2026-10-18, ADR 0009 addendum): "Remember this password in the keyring" in the password dialog; stored after a working connection, per account and desktop, removed on rejection, sign-out or Remove. Waiting for the owner's test on a password-logon desktop.
9. ~~**Clipboard direction controls**~~ Done (2026-10-18): off / both / only to the desktop / only to this computer (`/clipboard:direction-to:`), and files separately, off by default (`files-to:`). Before, an "on" clipboard also let files through both ways (FreeRDP's default mask). Not visible: a host policy that turns the clipboard off.
10. **Administrator registration / system-browser sign-in mode** (review F16; deferred).
11. **Stability acceptance** (§8, §13): several hours of active and idle use, audio device changes, monitor hotplug, network changes, suspend/resume.
12. Smaller items:
    - ~~Light/dark theme override (§5).~~ Done (2026-10-18): View → Settings → Appearance.
    - ~~The sign-in helper's "Sign in" strings aren't localized~~ Done (2026-10-18): the app passes its translated title to the helper (`COSMIC_EXT_DESKFEED_SIGNIN_TITLE`); "Sign in" stays only as the fallback.
    - ~~Capturing system shortcuts (Super, Alt+Tab) in a session and a visible way to release them (§9).~~ Done (2026-10-19, [ADR 0018](adr/0018-system-shortcuts-through-xwayland-grab.md)): FreeRDP's keyboard grab reaches COSMIC through XWayland's grab protocol, which cosmic-comp treats like shortcuts-inhibit; Keyboard setting at every level; Right Ctrl named in the setting and on the connected row. Waiting for the owner's check in a real session.
    - ~~Keep the screen on while a full-screen session is focused, when the user turns it on (§9).~~ Done (2026-10-19, [ADR 0019](adr/0019-keep-screen-on-in-full-screen.md)): Display setting at every level; `org.freedesktop.ScreenSaver` held only while such a window is in front and full screen. Checked with the local test server.
    - ~~Diagnostics support export, previewable and redacted (§12).~~ Done (2026-10-18): View → Diagnostics shows the whole report, Save writes exactly it (0600). Versions, dependency check, settings, counts per account, the last 300 log lines redacted again, known account, workspace and desktop names replaced. A report from the owner's real log had no IDs, e-mail addresses, tokens or host names.

## Parity work the spec allows later (§3)

- **Camera redirection**: done apart from the real-session test ([ADR 0013](adr/0013-camera-redirection.md)). Camera setting, off by default, gated by the feed's `camerastoredirect`; checked against the local test server. Still open: a real AVD session with a camera, unplug/replug (§13), and a dependency-check item for FreeRDP builds without the camera channel. FreeRDP 3.32.2 can't pick specific cameras or notice ones plugged in later.
- **RemoteApps**: listed, not connectable (`app/views.rs`). FreeRDP's RemoteApp support is in the X11 client, which Deskfeed now uses (§9).
- ~~Optional background operation~~ Done (2026-10-18, ADR 0017): tray icon with Show, connected desktops and Quit. Still open: tray resource menus (all desktops), session indicators, start minimized.
- Choosing specific monitors (beyond one or all).
- Microsoft identity broker (`sso-mib`): not in the distro build (ADR 0004).

## Release and project work

- Tag `v0.1.0`, then fill in the AUR package checksum (review F20).
- CI dependency/security checks (review F21): needs the owner's decision on adding `cargo-deny`, CI only.
- AppStream screenshots.
- Minimum GTK 4 and WebKitGTK versions as package constraints.

## Known deviation from the spec

The spec prefers native Wayland session windows (§9). Deskfeed uses `xfreerdp3` on XWayland, the documented compatibility mode §9 allows; the measurements are in [ADR 0004](adr/0004-freerdp-version-and-sign-in-helper.md) (client comparison, 2026-10-18).
