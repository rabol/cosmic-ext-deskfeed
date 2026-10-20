# Manual tests still to do

Only what automated tests can't check:
- Microsoft's real sign-in and service;
- the real COSMIC desktop (panel, autostart, keyboard, screen blanking);
- hardware, the network, sleep;
- how something looks.

The logic behind each test is covered by `cargo test`. Each test says why it is manual.

Each test must be able to pass on the owner's setup (see [compatibility.md](compatibility.md)). A check that can't run there goes in the compatibility matrix as "not tested", not here. When a test passes, move it to "Done" with the date. When one fails, note what happened: it's a bug.

Install the current build first (`just install`, or the package).

## To do

### Sign-in and accounts

2. **Sleep for over an hour** (real sleep, Microsoft's token renewal). Sign in, let the computer sleep for more than an hour, wake it, select Connect.
   - *Expected:* the desktop connects, or the account shows a Sign in button. Never a raw error.
   - Within 5 minutes of waking, the list's "Updated …" time changes to after waking.

3b. **Sign-out when the keyring refuses** (the real keyring and its unlock prompt). Lock the keyring, select Sign out, and cancel the unlock prompt.
   - *Expected:* "Sign-out couldn't remove all of the account's sign-in data" with Try again.
   - Select Try again and unlock. *Expected:* the warning goes, and the account doesn't come back after restarting Deskfeed.

3c. **Sign in while the keyring is locked** (same). Lock the keyring, sign in, and cancel the unlock prompt.
   - *Expected:* the desktop list works, with a note that the sign-in couldn't be saved and Try again.
   - Select Try again and unlock. *Expected:* the note goes, and you're still signed in after restarting Deskfeed.

3e. **Your account is filled in on Microsoft's page** (Microsoft's page). On a desktop that uses Entra sign-in, select Connect. *Expected:* when Microsoft's page appears, your account is already filled in or selected.

### Background and tray (ADR 0017)

3. **Connect from the tray with the window closed** (the panel's tray, Microsoft's page). Turn on Settings → Background, close the window, use the tray menu's "Connect …".
   - *Expected:* Microsoft's sign-in window appears (your organization asks for a sign-in on every connection); sign in there.
   - Deskfeed's own window stays closed, the desktop connects, and the tray menu shows it as connected.

4a. **Quit from the window** (the menu and the real app exiting). With Settings → Background on and nothing connected, open the Account menu.
   - *Expected:* Quit at the bottom, showing Ctrl+Q.
   - Quit, or Ctrl+Q, closes Deskfeed completely: no tray icon left.

4c. **Settings pages after the settings rework** (the pages' controls and the real settings store; cleanup-todo §2–3). Before installing the new build, note a few settings you changed earlier.
   - *Expected after installing:* those settings still show as before (the saved format is the same).
   - App settings: switch Full screen, change Time to connect, and pick a Clipboard direction. Account settings: set Sound to off, then back to "Use app setting". Desktop settings: set Camera on.
   - Restart Deskfeed. *Expected:* every change is still there, and Sound on the account follows the app again.

4d. **The sign-in helper after it stopped polling** (the real web view and FreeRDP; cleanup-todo §3).
   - Connect a desktop that uses Entra sign-in. *Expected:* it connects as before (silently, or after Microsoft's page).
   - Add an account and close Microsoft's sign-in window without signing in. *Expected:* the Add page says nothing went wrong and Sign in works again.
   - Start adding an account, and while Microsoft's page is open, quit Deskfeed (Ctrl+Q). *Expected:* the sign-in window closes with it.

4e. **The offline list while a sign-in has ended** (how it looks; the owner's tenant ends sign-ins often, AADSTS70043). Start Deskfeed when it asks you to sign in again.
   - *Expected:* "Sign in", then "Remove account" (no "Sign out"); under them "The desktops below are from …. Sign in to connect to them."; the desktops below say "Offline" and have no Connect button.
   - Sign in. *Expected:* the live list replaces it, with Connect buttons.

4. **Start when you log in** (COSMIC's autostart). Turn on Settings → Background → Start when you log in. Log out and back in. *Expected:* only the tray icon appears, no window.

### Desktop sessions

5. **System shortcuts** (the real keyboard, COSMIC and Windows; ADR 0018). In a connected desktop, press Super.
   - *Expected:* Windows' Start menu opens.
   - Press Right Ctrl, then Super. *Expected:* COSMIC's launcher opens.

6. **Keep the screen on** (real screen blanking; ADR 0019). Turn on Settings → Display → Keep the screen on in full screen. Use a desktop full screen and wait past your screen-off time.
   - *Expected:* the screen stays on.
   - With another window in front, it blanks as usual.

7. **Reconnect after a lost connection** (the real network, Microsoft's page; ADR 0014). With a connected desktop, turn Wi-Fi off for about 30 seconds, then on.
   - *Expected:* the row shows "Reconnecting (1 of 5)", and Microsoft's sign-in window appears. Sign in within 3 minutes, FreeRDP's limit per page.
   - The desktop comes back to the same Windows session. Selecting Stop instead ends the attempts.

8. **Display scaling** (how it looks; ADR 0015). On a monitor with scaling other than 100 %, connect a desktop. *Expected:* text in Windows is the same size as in COSMIC's apps.

9. **Camera** (a real camera; ADR 0013). Turn on the Camera setting for a desktop and connect.
   - If the row says "Camera not shared: your organization doesn't allow cameras on this desktop" (or only specific cameras), this test doesn't apply: tell me, and I'll remove it.
   - Otherwise open Windows' Camera app. *Expected:* the camera works.
   - Unplug it and plug it back in, and note what happens. FreeRDP 3.32.2 doesn't notice a camera plugged in later, so it may be gone until you reconnect.

15. **Clipboard directions** (a real desktop; both ways passed 2026-10-20). Set Clipboard to "Only to the desktop" and reconnect.
   - *Expected:* copying into the desktop works, copying out doesn't.
   - Optionally turn on "Copy files" and copy a file in.

### How things look

16. **"Changes apply the next time you connect"**. Connect a desktop and open its settings.
   - *Expected:* below the page's explanation, a separate line says it's connected and changes apply the next time you connect it.
   - Its account's settings and View → Settings have a similar line.

10. **Favorites and "Updated"**. Mark a desktop as a favorite.
   - *Expected:* an outline star turns filled, and the desktop shows at the top under Favorites.
   - The "Updated …" time is right.

11. **Offline list** (no network at start). Turn networking off and start Deskfeed. *Expected:* the last list shows, marked "Offline list from …", with Connect turned off.

### Longer runs

14. **Stability** (spec §8, §13). A few hours of use with a desktop open, including idle time, changing audio devices, plugging in a monitor, and sleep.

## Done

- 2026-10-20: the tray on ksni (4b): a click on the icon and "Show Deskfeed" give the window the keyboard; a connected desktop has a check mark and comes to the front; after `killall cosmic-panel` the icon returns and the window stays closed.
- 2026-10-20: clipboard works after the fix (`9cd5e5c`): text copied both ways.
- 2026-10-20: sign in again after reinstalling: one Microsoft sign-in page, then the desktop list.
- 2026-10-20: every desktop still connects after the stricter connection-file checks (ADR 0024).
- 2026-10-19: tray menu lists the open desktop and switches to it.
- 2026-10-19: "Keep running in the background" in the quit dialog closes the window and keeps the desktop connected (fix `7fc05ab`).
