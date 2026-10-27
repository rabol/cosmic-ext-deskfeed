# 21. Reconnect (plain RDP)

- **Status:** Automated (2026-10-27)
- **Area:** Plain RDP connections
- **Why it's manual:** Needs the real network.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed. Its password remembered.
- **Background:** [ADR 0014](../../adr/0014-reconnect-by-the-app.md)

## Steps

1. Connect with a remembered password.
2. Turn the Windows machine's network off for about 30 seconds, then on.

## Expected

- The row shows "Reconnecting (1 of 5)", then the desktop comes back to the same Windows session.

## Automated

`just session-checks` (`nested_lost_connection_is_recognised`): a real FreeRDP session whose server goes away ends as a lost connection. App tests: `app::decide::tests::reconnects_only_lost_entra_sessions` (a plain RDP connection reconnects only with a remembered password, never after Windows refused it) and `reconnect_waits_grow_to_a_minute`.

Not checked: that Windows hands back the same session. In the release check, with a remembered password; without one there is no reconnect, by design.
