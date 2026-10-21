# 4e. The offline list while a sign-in has ended

- **Status:** To do
- **Area:** Sign-in and accounts
- **Why it's manual:** Needs the real tenant ending a sign-in (yours does this often, AADSTS70043).
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to.

## Steps

1. Start Deskfeed at a time when it asks you to sign in again.
2. Look at the account's page.
3. Sign in.

## Expected

- Before signing in: "Sign in", then "Remove account" (no "Sign out"); under them "The desktops below are from …. Sign in to connect to them."; the desktops say "Offline" and have no Connect button.
- After signing in: the live list replaces it, with Connect buttons.
