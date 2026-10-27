# Release check

One run through the real app before each release, about 10 minutes. It covers what the automated tests can't reach for tests 4a, 18, 19, 20, 21 and 22 (each file in [automated/](automated/) says what that is): your real keyring, COSMIC's panel, Windows itself, and the Account menu as you see it.

- **Needs:** win-100 with Remote Desktop on, its user name and password, and its network cable or Wi-Fi within reach. Seahorse (Passwords and Keys).
- **First:** install the release build (`just build-release`, then `sudo just install`), restart Deskfeed, and run `just session-checks`; all must pass.

## Steps and what to see

1. Turn on Settings → Background ("Keep running in the background").
2. **Add connection:** win-100, your user name, the password, "Remember this password" on. Connect.
   - It connects without asking for the password.
3. Disconnect (close the desktop's window), then connect again.
   - No password dialog.
   - Seahorse lists "Deskfeed RDP password (win-100)".
4. Star the connection.
   - It is under Favorites at the top of the Connections page.
5. Close Deskfeed's window, open the tray menu and connect win-100 from there.
   - The tray lists it (under "Connections" if an account is signed in too) and it connects.
   - Opened again once connected, the tray shows a check mark next to it; selecting it brings the desktop's window to the front.
6. While connected, take win-100 off the network for about 30 seconds, then back.
   - The row shows "Reconnecting (1 of 5)", and the desktop comes back to the same Windows session (your open windows are still there).
7. Edit the connection and select Remove connection, then "Disconnect and remove".
   - A question comes first.
   - The desktop's window closes within a few seconds, the connection leaves the list, and Seahorse no longer lists its password.
8. Add win-100 again with a wrong password and "Remember this password" on, and connect.
   - The row says the user name or password was rejected.
   - Seahorse lists no password for it.
9. Open the Account menu.
   - Quit is at the bottom and shows Ctrl+Q.
10. Press Ctrl+Q.
    - Deskfeed closes completely: no window and no tray icon.

## When something differs

Note the step and what you saw here, under a date, and tell Claude: it's a bug. Then fix the step's test in [automated/](automated/) so the automated part catches it next time, if it can.

## Runs

(none yet)
