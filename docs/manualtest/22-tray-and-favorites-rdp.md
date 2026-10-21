# 22. Tray and favorites (plain RDP)

- **Status:** To do
- **Area:** Plain RDP connections
- **Why it's manual:** Needs COSMIC's real panel.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed.
- **Background:** [ADR 0017](../adr/0017-background-operation-and-tray.md)

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
