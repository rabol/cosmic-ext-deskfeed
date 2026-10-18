# Roadmap: what's still missing

Status on 2026-10-18, after 0.1.0 (commit `cb552a5`). Checked against [specs.md](../specs.md), the review's coverage table (CODE_REVIEW.md, written before this week's fixes) and the current code. The spec asks for outstanding work to be tracked openly (§3).

Working rules for this list: research the docs, source and APIs first; test locally where possible (`research/rdp-test-server`), and ask the owner for a real-tenant test only as the final check.

## Required for the first usable release (spec §3)

1. ~~**Resource icons.**~~ Done (2026-10-18): the feed's PNG, fetched in the background through the shared HTTP policy (256 KiB, no redirects), header-checked to at most 256×256 before display (`service/icons.rs`), cached per account and URL, cleared on sign-out. Stock icon until it loads. Checked with the owner's feed: 3 of 3 loaded. The `.ico` variant isn't used.
2. ~~**Search, favorites, last successful refresh time**~~ Done (2026-10-18): search by desktop or workspace name, favorites (heart button, "Favorites" section, stored per desktop key), "Updated 14:05" / "Showing the list from …" after a failed refresh. Waiting for the owner's look in the real app.
3. ~~**Offline metadata cache**~~ Done (2026-10-18, `cache.rs`): per account, names, titles, kinds and keys only (no URLs, no connection files); 0700/0600, atomic, versioned, bounded; kept 30 days by default (App/Account setting); deleted on sign-out and for accounts no longer saved. Shown read-only, "Offline list from 14:05", until the live list loads; Connect stays off. Waiting for the owner's test.
4. **Reconnect** after connection loss with bounded backoff, visible status and a stop action; **focus an existing session** when Connect is pressed for a running desktop (§3, §8).
5. **Display scaling that follows the monitor's scale** (§3, §8: derive it, never hardcode). With `xfreerdp3` Windows runs at 100 % on a 1.25-scaled screen, so its text is smaller than local apps.
6. **Explicit session states**: preparing, authenticating, awaiting user, connecting, connected, reconnecting, disconnecting, disconnected, cancelled, failed (§8). Today: preparing, connecting, connected, failed. "Connected" comes from `Loading Dynamic Virtual Channel` (`sessions/launcher.rs`, `CONNECTED_MARKER`); the review notes this is before session activation. Use the `CONNECTION_STATE_ACTIVE` state line instead (it now arrives live).
7. **Certificate approval** with identity and fingerprint, pinned, changes blocking (§8). Since the switch to `xfreerdp3` an unverifiable certificate is refused on its terminal prompt (`client/common/client.c`, `client_cli_accept_certificate`), so the connection fails instead.
8. **Opt-in password remembering** for password-logon host pools: per account and resource, Secret Service only, cleared on sign-out (§8).
9. **Clipboard direction controls** where the backend supports them (§9); today on/off only.
10. **Administrator registration / system-browser sign-in mode** (review F16; deferred).
11. **Stability acceptance** (§8, §13): several hours of active and idle use, audio device changes, monitor hotplug, network changes, suspend/resume.
12. Smaller items:
    - Light/dark theme override (§5).
    - The sign-in helper's "Sign in" strings aren't localized (`signin/src/main.rs`, `freerdp.rs`, `protocol.rs`).
    - Capturing system shortcuts (Super, Alt+Tab) in a session and a visible way to release them: unverified with `xfreerdp3` (§9).
    - Diagnostics support export, previewable and redacted (§12).

## Parity work the spec allows later (§3)

- **Camera redirection**: done apart from the real-session test ([ADR 0013](adr/0013-camera-redirection.md)). Camera setting, off by default, gated by the feed's `camerastoredirect`; checked against the local test server. Still open: a real AVD session with a camera, unplug/replug (§13), and a dependency-check item for FreeRDP builds without the camera channel. FreeRDP 3.32.2 can't pick specific cameras or notice ones plugged in later.
- **RemoteApps**: listed, not connectable (`app/views.rs`). FreeRDP's RemoteApp support is in the X11 client, which Deskfeed now uses (§9).
- Tray resource menus and session indicators, optional background operation, start minimized.
- Choosing specific monitors (beyond one or all).
- Microsoft identity broker (`sso-mib`): not in the distro build (ADR 0004).

## Release and project work

- Tag `v0.1.0`, then fill in the AUR package checksum (review F20).
- CI dependency/security checks (review F21): needs the owner's decision on adding `cargo-deny`, CI only.
- AppStream screenshots.
- Minimum GTK 4 and WebKitGTK versions as package constraints.

## Known deviation from the spec

The spec prefers native Wayland session windows (§9). Deskfeed uses `xfreerdp3` on XWayland, the documented compatibility mode §9 allows; the measurements are in [ADR 0004](adr/0004-freerdp-version-and-sign-in-helper.md) (client comparison, 2026-10-18).
