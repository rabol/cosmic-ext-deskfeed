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
- Remote desktops in a window take the window's size under COSMIC auto-tiling: once connected, the app moves the focus away from the window and back (COSMIC sends a window its size with each focus change), which makes FreeRDP send its size; Windows changes resolution once. Alone on its workspace, the window is maximized and restored instead. FreeRDP reports the window resizable a moment before it is, so the app checks that the window took the new size and tries again (up to 3 times).
- FreeRDP's log lines reach the app as they're written (its output is a terminal, not a pipe). Through a pipe they came in blocks, so the window fit above and the connected state could wait until FreeRDP ended.
- FreeRDP's window is a native Wayland window. SDL 3 chose XWayland on COSMIC, which mangled non-ASCII characters in the window title and hid the app ID the window fit and "open on a new workspace" look for.
- Signing out waits for the account's sign-in window and FreeRDP (with its sign-in helper) to end before deleting its cookies, so nothing can write them back; the helper never recreates deleted folders. Signing the same account in again waits until that is done.
- Every connection attempt ends: the app stops a sign-in window that outlives its time, and a connection that hasn't connected within the time to wait plus the sign-in pages. Events from an earlier attempt are ignored.
- Sign-in folders of cancelled "Add account" sign-ins are removed.
- The log replaces IDs, user names, IP addresses, organization host names and long tokens with placeholders, also in FreeRDP's error lines (redacted before they are shortened).
- Output from FreeRDP and messages to the sign-in helper are read with a size limit.
- An unanswered keyring unlock prompt is dismissed after 2 minutes, with a notice; one account waiting for the keyring no longer holds up another account's renewal.
- Desktops are told apart by workspace too, so the same desktop ID in two workspaces no longer shares settings; saved settings move to the new keys by themselves.
- FreeRDP gets exactly the proxy the app uses (same choice among the variables, and the same no-proxy list); a proxy type FreeRDP can't use is reported instead of connecting directly.
- Connect is refused with a clear message while a required dependency is missing or too old, and the check runs again; a non-executable file no longer hides a usable FreeRDP or sign-in helper.
- Installation with `just install`: desktop entry, icons, AppStream metadata.

### Known limitations

See [README → Known limitations](./README.md#known-limitations).
