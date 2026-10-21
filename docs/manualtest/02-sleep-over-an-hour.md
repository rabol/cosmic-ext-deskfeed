# 2. Sleep for over an hour

- **Status:** To do
- **Area:** Sign-in and accounts
- **Why it's manual:** Real sleep and Microsoft's token renewal can't be faked in a test.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to.

## Steps

1. Sign in.
2. Let the computer sleep for more than an hour.
3. Wake it.
4. Within 5 minutes, look at the list's "Updated …" time.
5. Select Connect on a desktop.

## Expected

- The "Updated …" time changes to after waking, within 5 minutes.
- The desktop connects, or the account shows a Sign in button. Never a raw error.
