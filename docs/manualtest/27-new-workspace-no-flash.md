# 27. No flash on a new workspace

- **Status:** To do
- **Area:** Desktop sessions
- **Why it's manual:** Needs COSMIC's real workspaces and Microsoft's page.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to. And a saved connection.
- **Background:** [ADR 0011](../adr/0011-connection-settings-and-new-workspace.md)

## Steps

1. Turn on Full screen and Open on a new workspace.
2. Go to a workspace that has other windows on it, and connect an AVD desktop.
3. Do the same with the saved connection.
4. Connect again, and close Microsoft's sign-in page without signing in (for the saved connection: give a wrong password).
5. With both monitors connected: click Deskfeed's window on the second monitor and connect.

## Expected

- COSMIC switches to an empty workspace right away; Microsoft's sign-in page (AVD) and then the desktop open there.
- The window doesn't show on the old workspace first.
- The log (View → Open log folder) says "open on a new workspace: the window opened there".
- When nothing opens: COSMIC switches back to the workspace you started from.
- Two monitors: the switch and the desktop happen on the monitor where Deskfeed's window was focused; the other monitor keeps its workspace.

## Notes

2026-10-27 (COSMIC 1.11, after the window-list fix `b959664`): steps 2 and 3 passed: no flash with the AVD desktop or the saved win-100 connection. Still to check: steps 4 and 5 (switching back when nothing opens, and the second monitor).
