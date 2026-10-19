# 0018. System shortcuts reach the desktop through XWayland's keyboard grab

- Status: Accepted (checked in the source code; waiting for the owner's check in a real session)
- Date: 2026-10-19

## Context

Spec §9 says shortcuts such as Super and Alt+Tab go to the session only while it has keyboard focus, through the Wayland keyboard-shortcuts-inhibit protocol, with a visible way to release them. Spec §5 asks for keyboard shortcut behavior as a setting at each level.

Deskfeed runs `xfreerdp3`, an X11 client under XWayland (ADR 0004). An X11 client can't use keyboard-shortcuts-inhibit. What happens instead (source: FreeRDP 3.32.2 `client/X11`, Xwayland 24.1.14 `hw/xwayland/xwayland-input.c`, cosmic-comp `41497b42d974`):

- FreeRDP calls `XGrabKeyboard` on its window when the pointer enters the focused window, or when the window gains focus with the pointer inside (`xf_event.c`, `EnterNotify` and `FocusIn`). This happens only with `GrabKeyboard` on, its default. It releases the grab when the pointer leaves (`LeaveNotify`) or focus moves away (`FocusOut`). Pressing and releasing Right Ctrl on its own also releases it (`xf_keyboard.c`, `xf_keyboard_handle_special_keys_release`).
- Xwayland turns an active X keyboard grab into `zwp_xwayland_keyboard_grab_v1` on the window's surface (`xwl_keyboard_activate_grab`), and destroys it when the grab ends.
- cosmic-comp implements that protocol (`wayland/handlers/xwayland_keyboard_grab.rs`). In its key and button handling it treats an active XWayland grab on the focused surface exactly like an active shortcuts inhibitor (`input/mod.rs`, `shortcuts_inhibited`): its own shortcuts aren't run, and keys go to the window.

The result matches what the spec asks of shortcuts-inhibit: shortcuts go to the session only while its window has focus and the pointer, and COSMIC's handling is the same code.

## Decision

- New setting "Send shortcuts like Super and Alt+Tab to the desktop" (Keyboard section, App, Account and Desktop levels). It's on by default, which is FreeRDP's default and today's behavior. The app always passes it explicitly: `+grab-keyboard` or `-grab-keyboard`, never left to the user's FreeRDP configuration.
- The way back is shown where it's needed:
  - the setting's description names Right Ctrl and moving the pointer out;
  - while a session started with the setting on is connected, its row says "Right Ctrl gives COSMIC its shortcuts back".
- No custom grab code. The deviation from the spec's wording (an XWayland grab, not shortcuts-inhibit) is recorded here, because no Wayland-native FreeRDP client is in use (ADR 0004).

## Consequences

- With the setting on and the pointer over a windowed session, Super opens Windows' Start menu instead of COSMIC's launcher. Moving the pointer out gives the shortcuts back without a key.
- In full screen the pointer can't leave, so Right Ctrl is the way back. After Right Ctrl, FreeRDP grabs again only when the pointer enters the window again.
- With the setting off, FreeRDP never grabs. COSMIC keeps all its shortcuts, and Windows still gets the other keys.
- FreeRDP's own hotkeys (Ctrl+Alt+Enter for full screen, Ctrl+Alt+M to minimize) stay as they are. Settings for those are a separate item.

## Addendum (2026-10-19): FreeRDP's own hotkeys

FreeRDP 3.32.2's X11 client (`client/X11/xf_keyboard.c`, `xf_keyboard_handle_special_keys`) handles these keys itself:

| Keys | What it does | Switch |
|---|---|---|
| Ctrl+Alt+Enter | full screen on and off | `-toggle-fullscreen` |
| Ctrl+Alt+M | minimizes the window | none |
| Ctrl+Alt+D | disconnects at once (not in `/help`) | none |
| Right Ctrl, pressed and released alone | releases the keyboard | none (it is the way back) |

Before any of these, FreeRDP runs an action script, `~/.config/freerdp/action.sh` by default, for the key combinations the script lists. The script can take keys from the session.

Decision:
- Setting "Ctrl+Alt+Enter switches full screen" (Keyboard, at every level), on by default, passed as `+toggle-fullscreen` or `-toggle-fullscreen`.
- Its description says Ctrl+Alt+M and Ctrl+Alt+D can't be turned off in this FreeRDP version (spec §5: unavailable capabilities are shown with a reason). An action script that answers `key-local` could swallow them, but FreeRDP refuses to start when such a script returns nothing (`xf_utils.c`, `run_action_script`), and it would run a shell on each such key press. That is too fragile for the gain.
- Every session gets `/action-script:` with a path that doesn't exist, in its private folder. The user's own `action.sh` doesn't change keys in Deskfeed's sessions (spec §5: never the user's FreeRDP configuration). Checked with the local test server: the session connects as before.

