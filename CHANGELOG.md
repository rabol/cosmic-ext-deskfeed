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
- Connection files from the feed are refused unless their gateway is an Azure Virtual Desktop service host (the access token goes there), and their certificate policy is ignored, so the session host's certificate is always checked.
- Desktop lists refresh by themselves every hour, and soon after a resume from suspend. Refresh renews the sign-in first; when it can't be renewed, the account asks for a new sign-in.
- A password logon chosen for a desktop is used even when the connection file asks for Entra sign-in.
- Renewed sign-ins are checked like restored ones: a renewed ID token must be valid and name the same account.
- The "sign-in not saved" notice after a sign-in now appears when the keyring fails.
- Quitting with desktops open asks first ("Disconnect and quit"), then ends every FreeRDP process, including anything it started, before the app exits. This also applies to closing from the dock or a keyboard shortcut.
- Signing out ends all of the account's sessions, including desktops that a refresh removed from the list.
- FreeRDP is started in the background, so the window never waits for it.
- Installation with `just install`: desktop entry, icons, AppStream metadata.

### Known limitations

See [README → Known limitations](./README.md#known-limitations).
