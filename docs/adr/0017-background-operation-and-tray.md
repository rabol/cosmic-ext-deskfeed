# 0017. Background operation with a StatusNotifierItem tray over zbus

- Status: Accepted (pending the owner's check in the real app)
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
- The item registers again when the panel restarts (`NameOwnerChanged` for the watcher).
- Desktop titles in the menu stay on the local session bus, as window titles do.

## Consequences

- With no status area in the panel, background operation is unavailable, not silently broken.
- `start minimized` (roadmap) can build on this later.
- The dbusmenu implementation is minimal: it always returns the whole menu, and it has no icons or submenus.
