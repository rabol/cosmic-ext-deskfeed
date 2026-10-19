# Changelog

All notable changes are listed here. The project uses [semantic versioning](https://semver.org/); while it is 0.x, any release may change behaviour.

## [Unreleased]

### Added

- RemoteApps (preview): apps from the feed connect and open as ordinary windows, grouped under Deskfeed in the dock, on every monitor. Settings that only fit a desktop (full screen, keyboard, workspace, keep the screen on) are left out for apps. When Windows won't start an app, the row says why. Checked against a Windows host, not yet with an AVD RemoteApp (ADR 0020).
- Two desktops asking for a password at once each get their dialog, one after the other; before, one could stay stuck "waiting for your password". A desktop that is preparing or waiting for its password can be cancelled from its row.
- A proxy's user name and password no longer go into the environment of FreeRDP or the sign-in window (ADR 0008 addendum). A proxy that needs a password can't be used for desktop connections because of a FreeRDP bug; the row now says so.
- If Deskfeed crashes or is killed, its desktops and sign-in windows now close with it instead of running on (ADR 0022).
- Error messages no longer contain raw technical text; the details are in the log (View → Diagnostics). A sign-in refused because the computer isn't a managed device now says so.
- Desktop lists refresh after the computer wakes from sleep, and hourly refreshes are spread by a few minutes either way.
- When the sign-in couldn't be saved to the keyring, a Try again button saves it again; until then it lasts only while Deskfeed runs.
- When sign-out can't remove everything (for example, the keyring stays locked), a Try again button removes what's left.
- Signing out of an account with desktops or apps connected asks first, offering "Disconnect and sign out" or Cancel.
- The administrator's clipboard, microphone and sound policy from the connection file now wins over your settings, as the camera policy already did. The desktop's row says what was turned off and why (ADR 0021).
- When a second app from the same place takes over the first app's session, the first app's row says its windows are now in the second app's connection.
- When another connection takes over a session (the desktop opened somewhere else, or a second app from the same host pool), the row says so, and Deskfeed doesn't reconnect and take it back.
- App settings (View → Settings): defaults for every account. New settings: time allowed for signing in and how often desktop lists refresh (app and account), and the log file size (app). Account settings now follow the app's setting or override it, as desktop settings do for the account.
- View → Diagnostics: a report for support that you read in full before saving. It has no passwords, tokens or cookies, and account, workspace and desktop names are replaced.
- Appearance: Deskfeed can match the desktop or stay dark or light (View → Settings), with the desktop's own colours.
- Clipboard directions: both ways, only to the desktop, or only to this computer. Copying files is a separate setting, off by default; before, turning the clipboard on also let files through both ways.
- Remember Windows passwords (opt-in): the password dialog can keep the password in the keyring for that desktop, so it isn't asked again and the desktop can reconnect by itself. A rejected password is removed; signing out removes all of them.
- Unverified desktop certificates: instead of failing silently, Deskfeed shows the certificate (name, issuer, validity, fingerprint) and lets you trust it for that desktop. A certificate that changes after you trusted it is blocked; you can remove the trusted one in the desktop's settings.
- Desktop rows say what a connection is doing: preparing, waiting for your password, connecting, signing in to Windows, connected, reconnecting, disconnecting. "Connected" now means the Windows session is active.
- Display scaling follows the monitor: Windows draws text and apps at COSMIC's scale (for example 125 %), so the desktop matches the apps around it. A "Display scaling" setting (App, account, desktop) can set it instead.
- Show: a connected desktop's row brings its window to the front, switching to its workspace.
- Reconnecting after a lost connection (network drop, sleep): the app connects again by itself, with growing waits and a Stop button, up to 5 attempts (App, account and desktop settings). Desktops that use a Windows password say the connection was lost instead.
- Offline desktop list: while the live list can't be loaded (offline, or signing in), the last one is shown, marked as offline and without Connect. It holds only names, is kept 30 days by default (App and account setting) and is deleted on sign-out.
- Search the desktop list by desktop or workspace name.
- Favorites: the heart button on a desktop adds it to a Favorites section at the top.
- The list shows when it was last updated, and after a failed refresh how old the shown list is.
- Desktop icons from the feed, with a stock icon until they load. Icons must be PNGs of at most 256×256 pixels.
- Camera setting (off by default), per account and desktop. It shares this computer's cameras when the desktop's administrator allows all cameras; otherwise the desktop row says why no camera is shared (ADR 0013).

## [0.1.0] - 2026-10-18

First version.

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
- Desktops connect with FreeRDP's X11 client (`xfreerdp3`) instead of the SDL client. Under COSMIC auto-tiling the SDL client kept the remote desktop at its first size (it sends the window's size only within 5 seconds of a window change, and a real logon takes longer) and sometimes opened as a floating window. The X11 client opens tiled and sends the tile's size as soon as the remote side is ready, also after the window moves or is re-tiled ([ADR 0004](./docs/adr/0004-freerdp-version-and-sign-in-helper.md)).
- FreeRDP's log lines reach the app as they're written (its output is a terminal, not a pipe). Through a pipe they came in blocks, so the connected state could wait until FreeRDP ended.
- The sign-in window opens as a floating, fixed-size dialog in the middle of the screen, so COSMIC's auto-tiling no longer resizes the other windows while it's open. It belongs to Deskfeed in the dock (it had no app ID of its own).
- "Open on a new workspace" works on current COSMIC: it advertises the move under an older capability name, which the app now accepts.
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
