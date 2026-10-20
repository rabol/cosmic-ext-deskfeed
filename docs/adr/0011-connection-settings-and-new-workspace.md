# 0011. Connection settings per account and desktop; open on a new workspace

- Status: Accepted (new workspace: pending the owner's test)
- Date: 2026-10-17

## Context

The owner asked for several accounts, each with its own connection defaults, and for desktops to open on a new COSMIC workspace. Spec §Settings asks for defaults with per-resource overrides.

## Decision

- **Levels:** built-in defaults → app settings (View → Settings, added 2026-10-18 at the owner's request) → account overrides → desktop overrides. Every value that could matter to a user belongs at these levels; [settings-inventory.md](../settings-inventory.md) lists them and the values that stay fixed. Unset values inherit (`src/settings.rs`). Stored in the COSMIC config (`account_settings`, `desktop_settings`), keyed by opaque account IDs and resource keys; no credentials.
- **First settings:** time to wait for the desktop (10–600 s, default 120 s, measured need up to 49.8 s), full screen, all monitors (full screen only), open on a new workspace, sound (default on), microphone and clipboard (default off, spec §11).
- **FreeRDP arguments** are always explicit, because the feed's `.rdp` may ask for full screen or several monitors: `/timeout:`, `+f`/`-f`, `/multimon`/`-multimon`, `/audio-mode:redirect /sound` or `/audio-mode:none`, `/microphone`, `/clipboard`/`-clipboard`. Each was checked to parse with `sdl-freerdp3` 3.32.2.
- **UI (COSMIC design):** account settings in the context drawer from a header button, as settings sections with togglers and a spin button, Sign out at the bottom. Desktop settings from a button on each desktop row, with a dropdown per setting (account default / on / off) and a reset for the timeout.
- **Open on a new workspace:** FreeRDP gets `/wm-class:<app ID>` (the dock groups the window with Deskfeed) and `/title:<desktop> – <account>`. A background task (`src/workspace.rs`) waits for that window with `ext-foreign-toplevel-list` + `cosmic-toplevel-info`, takes the first empty workspace in the window's group (or asks COSMIC to create one), and moves the window with `cosmic-toplevel-management` `move_to_ext_workspace` (v4). It uses cosmic-client-toolkit as re-exported by libcosmic: no new crate. It stops when the session ends; failure is logged, not shown (the desktop still opened).
- The dependency check gained an optional "COSMIC window management" item that looks for those protocols.

- **Show** (added 2026-10-18): a connected desktop's row has a Show button that finds the session's window the same way and asks COSMIC to activate it (`activate` with the first seat; `workspace::focus`). It succeeds only when COSMIC then reports the window activated. Checked with two local test sessions: with the later one focused, Show on the earlier one made it the active window, and back.

## Consequences

- cosmic-comp offers window management only to unsandboxed clients; another reason for [ADR 0010](0010-native-only-and-dependency-check.md).
- If two desktops with the same title and account open at the same moment, either window may be moved first; both end up on new workspaces when both ask for it.
