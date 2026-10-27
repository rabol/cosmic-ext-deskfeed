# 27. New workspace on the second monitor

- **Status:** To do
- **Area:** Desktop sessions
- **Why it's manual:** Needs COSMIC's real workspaces and Microsoft's page.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to. And a saved connection.
- **Background:** [ADR 0011](../adr/0011-connection-settings-and-new-workspace.md)

## Steps

1. Turn on Full screen and Open on a new workspace for a desktop.
2. Connect both monitors. Click Deskfeed's window on the second monitor and connect.

## Expected

- The switch to an empty workspace and the desktop happen on the monitor where Deskfeed's window was; the other monitor keeps its workspace.

## Notes

2026-10-27 (COSMIC 1.11, after the window-list fix `b959664`): the first steps passed by hand, with no flash for the AVD desktop and the saved win-100 connection.

Since then automated with `just session-checks`, in a nested COSMIC session with one monitor: `nested_new_workspace_without_flash` (with another window on the workspace in use, the desktop opens on an empty one, is never seen on the old one, and has the focus) and `nested_switch_back_when_nothing_opens` (no window comes: back to the workspace you started from). The nested session has one monitor, so the second monitor stays manual.
