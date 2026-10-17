# Changelog

All notable changes are listed here. The project uses [semantic versioning](https://semver.org/); while it is 0.x, any release may change behaviour.

## [Unreleased]

First version (0.1.0), not yet released.

### Added

- Sign-in with Microsoft's public Remote Desktop client identity in a separate, locked-down sign-in window (WebKitGTK); PKCE, state and ID token checks.
- Discovery of workspaces and desktops from all tenant feeds.
- Connecting with FreeRDP 3.32.2 through the AVD gateway, with Entra sign-in first and a native password dialog (paste allowed) for host pools that need a Windows password.
- FreeRDP's sign-in requests answered by the app's sign-in window, with the gateway token supplied by the app and a silent first attempt.
- Several accounts side by side, each with its own sign-in, cookies and keyring entry.
- Staying signed in: refresh tokens in the desktop keyring (Secret Service over D-Bus); no file fallback.
- Settings per account with per-desktop overrides: time to wait, full screen, all monitors, open on a new COSMIC workspace, sound, microphone, clipboard.
- "Open on a new workspace" through COSMIC's window and workspace protocols.
- Dependency check at startup and under View → Dependencies.
- Log file under `~/.local/state/cosmic-ext-deskfeed/` with View → Open log folder.
- Installation with `just install`: desktop entry, icons, AppStream metadata.

### Known limitations

See [README → Known limitations](./README.md#known-limitations).
