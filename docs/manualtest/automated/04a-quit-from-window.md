# 4a. Quit from the window

- **Status:** Automated (2026-10-27)
- **Area:** Background and tray
- **Why it's manual:** Needs the real menu and the app really exiting.
- **Needs:** Nothing connected.
- **Background:** [ADR 0017](../../adr/0017-background-operation-and-tray.md)

## Steps

1. Turn on Settings → Background.
2. Open the Account menu.
3. Select Quit (or press Ctrl+Q).

## Expected

- Quit is at the bottom of the menu, showing Ctrl+Q.
- Deskfeed closes completely: no tray icon left.

## Automated

App tests (`cargo test`): `app::tests::quit_from_the_menu_asks_while_a_desktop_is_open` and `quit_from_the_menu_with_nothing_open_doesnt_ask` (Quit from the Account menu quits even with background operation on, asking first while a desktop is open), and `app::decide::tests::closing_and_quitting`.

Not checked: how the menu looks (Quit at the bottom, Ctrl+Q shown), the key press itself, and that no tray icon is left. In the release check.
