# 32. "Keep running in the background" from the quit dialog

- **Status:** Passed (2026-10-19)
- **Area:** Background and tray
- **Why it's manual:** Needs the real app window closing.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to.
- **Background:** [ADR 0017](../adr/0017-background-operation-and-tray.md)

## Steps

1. Connect a desktop.
2. Close Deskfeed's window and choose "Keep running in the background" in the dialog.

## Expected

- The window closes and the desktop stays connected (fix `7fc05ab`).
