# 4d. The sign-in helper after it stopped polling

- **Status:** To do
- **Area:** Sign-in and accounts
- **Why it's manual:** Needs the real web view and FreeRDP (cleanup-todo §3).
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to.

## Steps

1. Connect a desktop that uses Entra sign-in.
2. Add an account, and close Microsoft's sign-in window without signing in.
3. Start adding an account again, and while Microsoft's page is open, quit Deskfeed (Ctrl+Q).

## Expected

- The desktop connects as before (silently, or after Microsoft's page).
- After closing the window: the Add page says nothing went wrong, and Sign in works again.
- After quitting: the sign-in window closes with Deskfeed.
