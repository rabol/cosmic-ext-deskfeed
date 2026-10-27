# 26. Open on a new workspace follows the window

- **Status:** Passed (2026-10-21)
- **Area:** Desktop sessions
- **Why it's manual:** Needs COSMIC's real workspaces.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to. And a saved connection.
- **Background:** [ADR 0011](../../adr/0011-connection-settings-and-new-workspace.md)

## Steps

1. Turn on Full screen and Open on a new workspace.
2. Connect.

## Expected

- COSMIC switches to the new workspace with the desktop on it, and typing goes to the desktop.

## Notes

Passed with a saved connection and an AVD desktop (fix `dbd5918`). With AVD the window still showed on the old workspace first: see test 27.

Broke with COSMIC 1.11 (installed 2026-10-27): the desktop opened on the workspace in use, while the log said "opened there". COSMIC now sends its window list a moment later, so every workspace looked empty. After the fix (Deskfeed waits for the list), passed again on 2026-10-27 with an AVD desktop and 5 pinned workspaces.

Also covered by `just session-checks` since 2026-10-27 (`nested_new_workspace_without_flash`, `nested_full_screen_follows_the_setting`), with the local test server instead of Microsoft.
