# 0017. Background operation with a StatusNotifierItem tray over zbus

- Status: Accepted (checked on the live COSMIC session on 2026-10-19: tray item registered, menu, close to background, reopen from the tray, second start reopens the first, tray Quit, setting off; with a connected desktop still to check)
- Date: 2026-10-18

## Context

Spec §5 allows optional background operation: closing the window keeps Deskfeed and its desktops running, with a way back. Off by default, and never without a way back.

COSMIC's panel shows tray icons through its status area applet (`cosmic-applet-status-area`), which is a StatusNotifierHost: apps register a `org.kde.StatusNotifierItem` with `org.kde.StatusNotifierWatcher` and expose a `com.canonical.dbusmenu` menu. On a click the applet calls `ProvideXdgActivationToken` and then `Activate`. Otherwise it opens the menu.

Tray crates (`ksni`, `tray-icon`, `libappindicator`) would add a dependency, and some would add GTK. ADR 0003 says to keep dependencies minimal. zbus is already in the tree for the keyring (ADR 0009).

libcosmic closes the app when its main window closes, unless the `multi-window` feature is on. With that feature, the app can close the window (`set_main_window_id(None)` + `window::close`) and open it again later with the same settings.

## Decision

- App settings → Background → "Keep running in the background", config `background`, off by default.
- The tray is served directly with zbus (`src/tray.rs`): one item and a small menu with "Show Deskfeed", one "Show <desktop>" per connected desktop (at most 20), and Quit. No new crate. The libcosmic `multi-window` feature is turned on.
- The tray runs as an iced subscription only while the setting is on. If no watcher or host answers, the app gets `Unavailable`: the window stays open (or is reopened), closing quits as before, and the setting shows why.
- Closing with the setting on and a tray present:
  - with no desktops open, the window closes and the app keeps running;
  - with desktops open, the quit dialog adds a third choice, "Keep running in the background".
- The tray's Quit really quits. With desktops open, it shows the window and asks first, as usual.
- A click on the tray icon, or starting Deskfeed again (single instance, D-Bus activation), opens the window again or focuses it. COSMIC's activation token is passed on so the window takes focus.
- Single instance is done by Deskfeed (`src/single.rs`), not by libcosmic's `single-instance` feature. libcosmic serves at the app ID with only `.` replaced (`/io/github/rabol/cosmic-ext-deskfeed`), but `-` isn't allowed in a D-Bus object path, so the app exited at start, and a second start ran a second copy. Deskfeed claims its app ID as a bus name and serves `org.freedesktop.Application` at `/io/github/rabol/cosmic_ext_deskfeed` (that interface's rule: `-` becomes `_`). A second start calls `Activate` with its activation token and ends.
- The item registers again when the panel restarts (`NameOwnerChanged` for the watcher).
- Desktop titles in the menu stay on the local session bus, as window titles do.

## Consequences

- With no status area in the panel, background operation is unavailable, not silently broken.
- `start minimized` (roadmap) can build on this later.
- The dbusmenu implementation is minimal: it always returns the whole menu, and it has no icons or submenus.

## Addendum (2026-10-19): start when you log in

- App settings → Background → "Start when you log in" writes an XDG autostart entry, `~/.config/autostart/io.github.rabol.cosmic-ext-deskfeed.desktop`, with `Exec=cosmic-ext-deskfeed --background`. Removing it turns the setting off; the file is the setting. COSMIC runs these entries at login.
- `--background` starts without a window (libcosmic `no_main_window`). The window still opens when background operation is off, when no tray answers, or when a dependency is missing, so Deskfeed is never hidden without a way back.
- Checked on the live session: with background on, `--background` showed only the tray icon, and a tray click opened the window. With it off, the window opened.

## Addendum (2026-10-19): desktops and sessions in the tray menu

- COSMIC's status area (cosmic-applets `cosmic-applet-status-area`, `components/status_menu.rs`) draws menu labels, separators, disabled items, a check mark for `toggle-state` 1 and one level of submenus. It sends a fresh activation token before each menu click. It shows no tooltip, title or attention state, so sessions show in the menu.
- The app builds the menu; `tray.rs` only serves it. From the top:
  - "N desktops connected" (only when there are any);
  - Show Deskfeed;
  - every desktop of each signed-in account (favorites first, then by title; in a submenu per account when there are several). A connected desktop has a check mark and brings it to the front; one that isn't offers "Connect …"; a busy one shows its state, disabled;
  - Quit.
- The menu is rebuilt after every message and sent only when it changed.
- Connect from the tray works with the window closed. The window opens when the user is needed: a password or certificate question, the quit question, a sign-in, or a failed connection.
- Show from the menu now passes COSMIC's activation token, like a click on the icon.


## Addendum (2026-10-19): the panel's tray going away (review U3)

Until now the item registered again when the panel's tray (the StatusNotifierWatcher) came back, but the app wasn't told when the tray disappeared. With the window closed, Deskfeed could then be left with no visible way back except starting it again.

- `tray::follow_host` watches the watcher's name. If it is gone and still gone after 5 s (a panel restart takes a moment), the app gets `Unavailable` and shows its window, as when there is no tray at startup. When the watcher returns, the item registers again and the app gets `Ready`, so the icon and menu come back. A refused registration also counts as `Unavailable`.
- Tested on a private bus (`dbus-daemon`, with a stand-in watcher), so the real panel isn't touched (`follows_the_panels_tray`). A quick restart re-registers without telling the app. A tray gone for good tells it after the grace time. A tray coming back later brings the icon back. Without the new step, the test fails. CI installs `dbus` for it.
