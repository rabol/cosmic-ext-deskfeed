# 20. Remove while connected

- **Status:** Automated (2026-10-27)
- **Area:** Plain RDP connections
- **Why it's manual:** Needs a real FreeRDP process and Windows machine.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed.
- **Background:** [ADR 0029](../../adr/0029-plain-rdp-connections.md)

## Steps

1. Connect, then edit the connection and select Remove connection.
2. Select "Disconnect and remove".
3. Add it again, select Connect, and while it is still connecting, remove it.

## Expected

- A question comes first.
- The session window closes within a few seconds; the connection leaves the list (and the sidebar, if it was the last), and its keyring item is gone.
- Removing while connecting: the window doesn't stay open.

## Automated

App tests: `app::tests::saved_connections::removing_an_open_connection_asks_first` and `removing_takes_everything_about_it` (the question, then the connection, its password, certificate, settings and favorite are gone).

Not checked: that the real session window closes. In the release check.
