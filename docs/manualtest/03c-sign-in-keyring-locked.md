# 3c. Sign in while the keyring is locked

- **Status:** To do
- **Area:** Sign-in and accounts
- **Why it's manual:** Needs the real keyring and its unlock prompt.
- **Needs:** An account you can sign in with; a keyring manager such as Seahorse.

## Steps

1. Lock the keyring.
2. Sign in.
3. Cancel the unlock prompt.
4. Select Try again and unlock the keyring.
5. Restart Deskfeed.

## Expected

- After cancelling: the desktop list works, with a note that the sign-in couldn't be saved, and Try again.
- After Try again and unlocking: the note goes.
- After restarting: you're still signed in.
