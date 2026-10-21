# 3c. Sign in while the keyring is locked

- **Status:** Completed but not tested
- **Area:** Sign-in and accounts
- **Why it's manual:** Needs a real keyring that is locked, and its unlock prompt.
- **Needs:** Your Microsoft work account, signed out of Deskfeed (or not added yet). A keyring that can be locked and shows an unlock prompt, such as GNOME Keyring with Seahorse.

Signing in to Deskfeed saves your Microsoft sign-in in the keyring, so Deskfeed can sign in again without asking after a restart. When the keyring is locked and you refuse to unlock it, the sign-in still works for this run, and Deskfeed should say it couldn't be saved.

## Steps

1. Lock the keyring (Seahorse: right-click "Login" → Lock).
2. In Deskfeed, select Sign in on the account's page (or Account menu → Add account) and sign in on Microsoft's page.
3. When the system asks for the keyring password to unlock it, select Cancel.
4. Select Try again next to the note, and this time unlock the keyring.
5. Quit Deskfeed (Ctrl+Q) and start it again.

## Expected

- After step 3: the desktop list works, with a note that the sign-in couldn't be saved, and Try again.
- After step 4: the note goes.
- After step 5: you're still signed in, and the desktops load without a Microsoft page.

## Notes

Not runnable on the owner's setup, for the same reason as [3b](03b-sign-out-keyring-refuses.md): `oo7-daemon` 0.6.0 there has no unlock prompter installed. `cargo test` covers the wording of the note (`app::errors::tests::unsaved_sign_ins_are_explained`); that a refused save leaves the sign-in working for this run is only checked here.
