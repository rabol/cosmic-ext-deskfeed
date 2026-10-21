# 21. Reconnect (plain RDP)

- **Status:** To do
- **Area:** Plain RDP connections
- **Why it's manual:** Needs the real network.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed. Its password remembered.
- **Background:** [ADR 0014](../adr/0014-reconnect-by-the-app.md)

## Steps

1. Connect.
2. Turn the Windows machine's network off for about 30 seconds, then on.

## Expected

- The row shows "Reconnecting (1 of 5)", then the desktop comes back to the same Windows session.
