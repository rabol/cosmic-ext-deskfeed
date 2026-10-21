# 3. Connect from the tray with the window closed

- **Status:** Passed (2026-10-21)
- **Area:** Background and tray
- **Why it's manual:** Needs COSMIC's real panel tray and Microsoft's sign-in page.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to.
- **Background:** [ADR 0017](../adr/0017-background-operation-and-tray.md)

## Steps

1. Turn on Settings → Background.
2. Close Deskfeed's window.
3. In the tray menu, select "Connect …" for a desktop.
4. Sign in on Microsoft's page when it appears.

## Expected

- Microsoft's sign-in window appears (your organization asks for a sign-in on every connection).
- Deskfeed's own window stays closed.
- The desktop connects, and the tray menu shows it as connected.
