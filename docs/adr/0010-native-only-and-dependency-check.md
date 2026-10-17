# 0010. Native packages only, with a startup dependency check

- Status: Accepted
- Date: 2026-10-17

## Context

The spec planned a Flatpak after the core worked. The owner decided against it: the app is **never** a Flatpak. Evidence that agrees: COSMIC's compositor offers window management (needed for "open on a new workspace") only to clients without a sandbox security context (`cosmic-comp` `src/state.rs`, `ToplevelManagementState` with `client_not_sandboxed`), and a Flatpak would need its own pinned FreeRDP and WebKitGTK.

Without a sandbox bundle, the app relies on system software, which may be missing, too old or built without a needed feature. Users must be told clearly.

## Decision

- Native packages only. Spec §11, step 5 and the distribution phase are changed accordingly; milestone step I is dropped.
- The README lists every runtime dependency with its minimum version and the Arch Linux package.
- The app checks them at startup and from **View → Dependencies** (with **Check again**), in the background (`src/deps.rs`). Each check tests the capability used, not only that a file exists:
  - FreeRDP: `sdl-freerdp3` (or `sdl-freerdp`) in `PATH`, `/version` at least 3.32.2 ([ADR 0004](0004-freerdp-version-and-sign-in-helper.md)).
  - FreeRDP Entra support: `/buildconfig` contains `WITH_AAD=ON`.
  - Sign-in helper: found next to the app and `--check` answers `ok`, which proves GTK 4 and WebKitGTK 6 load.
  - Secret Service: `org.freedesktop.secrets` opens a session and has a default keyring (optional: without it, sign-in isn't remembered).
  - `XDG_RUNTIME_DIR` exists.
  - COSMIC window management: added with the "open on new workspace" feature.
- Missing required items show a COSMIC warning bar; details are in the context drawer as settings sections. Commands run with a 5 s limit and bounded output.

## Consequences

- Minimum versions are a documented contract; raising one means updating the README, `deps::MIN_FREERDP` and this ADR.
