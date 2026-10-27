# 22. Tray and favorites (plain RDP)

- **Status:** Automated (2026-10-27)
- **Area:** Plain RDP connections
- **Why it's manual:** Needs COSMIC's real panel.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed.
- **Background:** [ADR 0017](../../adr/0017-background-operation-and-tray.md)

## Steps

1. Turn on "Keep running in the background" and close the window.
2. Open the tray menu and connect the saved connection.
3. Open the tray menu again once connected.
4. In the window, star the connection.

## Expected

- The tray menu lists the connection (under "Connections" when an account is signed in too).
- Connect asks for the password in the window, or connects with the remembered one.
- Once connected it has a check mark, and selecting it brings its window to the front.
- Starred: it appears under Favorites at the top of the Connections page, and first in its tray group.

## Automated

App tests: `app::tests::saved_connections::the_tray_lists_connections` (under "Connections" when an account is signed in too), `app::background::tests::connected_busy_and_failed_desktops` (the check mark) and `nothing_connected_shows_no_count_and_offers_connect` (a favorite first in its group); `tray::tests::the_panel_sees_the_menu_and_clicks_reach_the_app` drives the real tray menu over D-Bus with a stand-in panel.

Not checked: COSMIC's panel itself, and the Favorites section in the window. In the release check.
