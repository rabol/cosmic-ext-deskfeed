# 7. Reconnect after a lost connection (AVD)

- **Status:** To do
- **Area:** Desktop sessions
- **Why it's manual:** Needs the real network and Microsoft's page.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to.
- **Background:** [ADR 0014](../adr/0014-reconnect-by-the-app.md)

## Steps

1. Connect a desktop.
2. Turn Wi-Fi off for about 30 seconds, then on.
3. Sign in on Microsoft's page within 3 minutes (FreeRDP's limit per page).
4. Repeat, and this time select Stop on the row.

## Expected

- The row shows "Reconnecting (1 of 5)", and Microsoft's sign-in window appears.
- The desktop comes back to the same Windows session.
- Stop ends the attempts.
