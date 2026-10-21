# 3e. Your account is filled in on Microsoft's page

- **Status:** To do
- **Area:** Sign-in and accounts
- **Why it's manual:** Only Microsoft's real page shows whether it uses the account Deskfeed passes along.
- **Needs:** Your Microsoft work account signed in to Deskfeed, and the AVD desktop you normally connect to.
- **Background:** [ADR 0027](../adr/0027-desktop-sign-in-pre-selects-the-apps-account.md)

Connecting to an AVD desktop opens a second Microsoft sign-in window (your organization asks for it on every connection). Deskfeed tells that window which account you are signed in with, so you shouldn't have to type or pick it again.

## Steps

1. In Deskfeed, select your work account in the sidebar.
2. Select Connect on the desktop you normally use.
3. If a password dialog appears instead of Microsoft's window: set **Logon setting** to "Entra sign-in, no password (recommended)" and select Connect.
4. Microsoft's sign-in window opens. Before typing or clicking anything, look at it.
5. Finish signing in, so the desktop connects.

## Expected

- At step 4 your work e-mail address is already there: either the password page for your address, or "Pick an account" with your account listed. Not an empty "Sign in" page asking for an e-mail address.
- After step 5 the desktop connects as usual.

## Notes

If no Microsoft window appears at all, the sign-in went through by itself: the test doesn't apply that time, so try again later.
