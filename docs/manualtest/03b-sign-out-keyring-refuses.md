# 3b. Sign-out when the keyring refuses

- **Status:** To do
- **Area:** Sign-in and accounts
- **Why it's manual:** Needs the real keyring and its unlock prompt.
- **Needs:** An account that is signed in; a keyring manager such as Seahorse to lock the keyring.

## Steps

1. Lock the keyring (Seahorse: right-click Login → Lock).
2. Select Sign out.
3. Cancel the unlock prompt.
4. Select Try again and unlock the keyring.
5. Restart Deskfeed.

## Expected

- After cancelling: "Sign-out couldn't remove all of the account's sign-in data" with Try again.
- After Try again and unlocking: the warning goes.
- After restarting: the account doesn't come back signed in.
