# 4f. Signed-out accounts stay listed

- **Status:** To do
- **Area:** Sign-in and accounts
- **Why it's manual:** Needs the real keyring and settings store.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to.

## Steps

1. Sign out of your account.
2. Restart Deskfeed.
3. Sign in.
4. Sign out again, then select Remove account.

## Expected

- After signing out: the account stays in the sidebar; its page offers Sign in and Remove account, with no desktops.
- After restarting: still listed, signed out.
- After signing in: the desktops come back.
- After Remove account: it leaves the sidebar, which then shows only "Sign in".
