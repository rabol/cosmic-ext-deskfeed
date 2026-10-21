# 4b. The tray on ksni

- **Status:** Passed (2026-10-20)
- **Area:** Background and tray
- **Why it's manual:** Needs COSMIC's real panel.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to.
- **Background:** [ADR 0017](../adr/0017-background-operation-and-tray.md)

## Steps

1. Click the tray icon, and select "Show Deskfeed".
2. Connect a desktop and open the tray menu.
3. Run `killall cosmic-panel`.

## Expected

- The window gets the keyboard.
- A connected desktop has a check mark and comes to the front from the menu.
- After the panel restarts, the icon returns and the window stays closed.
