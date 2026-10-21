# 3b. Sign-out when the keyring refuses

- **Status:** Completed but not tested
- **Area:** Sign-in and accounts
- **Why it's manual:** Needs a real keyring that is locked, and its unlock prompt.
- **Needs:** Your Microsoft work account signed in to Deskfeed (the account in the sidebar, whose desktops are listed). A keyring that can be locked and shows an unlock prompt, such as GNOME Keyring with Seahorse.

"Sign out" here means signing your Microsoft account out of Deskfeed: Deskfeed then deletes that account's saved sign-in from the keyring. It doesn't sign you out of COSMIC or Windows. When the keyring is locked and you refuse to unlock it, Deskfeed can't delete that data, and should say so.

## Steps

1. In Deskfeed, select your account in the sidebar.
2. Lock the keyring (Seahorse: right-click "Login" → Lock).
3. In Deskfeed, open the Account menu and select Sign out (or open the account's settings and select Sign out at the bottom). If a desktop is connected, confirm with "Disconnect and sign out".
4. When the system asks for the keyring password to unlock it, select Cancel.
5. Select Try again next to the warning at the top of the window, and this time unlock the keyring.
6. Quit Deskfeed (Ctrl+Q) and start it again.

## Expected

- After step 4: the account is signed out, and a warning at the top of the window says "Sign-out couldn't remove all of the account's sign-in data from this computer", with a Try again button.
- After step 5: the warning goes.
- After step 6: the account is still listed, but signed out: its page offers Sign in and Remove account, and no desktops load until you sign in again.

## Notes

Not runnable on the owner's setup (checked 2026-10-21): COSMIC's keyring there is `oo7-daemon` 0.6.0, which shows its unlock prompt through GNOME's or KDE's prompter, and neither is installed. A locked keyring would then refuse without a prompt, and could likely only be unlocked again by logging out and back in. The logic (a failed delete is reported, Try again repeats it, and sign-out still blocks later saves) is covered by `cargo test` (`accounts::tests`, among them `sign_out_blocks_late_saves_even_if_the_keyring_fails`).
