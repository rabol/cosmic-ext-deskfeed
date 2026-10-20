# Manual tests still to do

Checks that only the owner can do, in the installed app with the real tenant. Each says what to do and what should happen. When one passes, move it to "Done" with the date. When one fails, note what happened and open it as a bug.

Install the current build first (`just install`, or the package).

## To do

### Sign-in and accounts

2. **Sleep for over an hour** (commit `0b957ce`). Sign in, let the computer sleep for more than an hour, wake it, select Connect. *Expected:* the desktop connects, or the account shows a Sign in button. Never a raw error. Also, within 5 minutes of waking, the list's "Updated …" time changes to the time after waking, even after a short sleep (the log says "woke from sleep: refreshing desktop lists").

3a. **Sign out with a desktop open.** Connect a desktop, then select Sign out on its account. *Expected:* a question "Sign out of …?" with "Disconnect and sign out" and Cancel. Cancel keeps everything; the other button disconnects and signs out. Without anything connected, Sign out happens at once as before.

3b. **Sign-out when the keyring refuses.** Lock the keyring (for example in Passwords and Keys, if you have it), select Sign out, and cancel the unlock prompt. *Expected:* a warning "Sign-out couldn't remove all of the account's sign-in data" with a Try again button. Select Try again and unlock. *Expected:* the warning goes away, and the account doesn't come back after restarting Deskfeed.

3c. **Sign in while the keyring is locked.** Lock the keyring, sign in, and cancel the unlock prompt. *Expected:* the desktop list works, with a note that the sign-in couldn't be saved and a Try again button. Select Try again and unlock. *Expected:* the note goes away, and after restarting Deskfeed you're still signed in.

3d. **Cancel before a desktop starts.** Select Connect, and while the row says "Preparing" or "Waiting for your password", select Cancel. *Expected:* the row goes back to Connect, any password dialog for it closes, and nothing starts afterwards. With two desktops that both ask for a password, the second dialog appears after you answer or cancel the first.

3e. **The desktop sign-in has your account filled in** (ADR 0026). On a desktop that uses Entra sign-in, select Connect. *Expected:* when Microsoft's page appears, your account is already filled in or selected. (The check that would refuse a different account doesn't work against your tenant; see the ADR's addendum.)

### Background and tray (ADR 0017)

3. **Connect from the tray with the window closed.** Turn on Settings → Background, close the window, use the tray menu's "Connect …". *Expected:* Microsoft's sign-in window appears (your organization asks for a sign-in on every connection); sign in there. Deskfeed's own window stays closed, and the desktop connects. The tray menu then shows a check mark on that desktop and "1 connected" at the top. If the connection fails, Deskfeed's window opens and says why.
4a. **Quit from the window.** With Settings → Background on, open the Account menu. *Expected:* Quit at the bottom, showing Ctrl+Q. With nothing connected, Quit (and Ctrl+Q) closes Deskfeed completely: no tray icon left. With a desktop connected, it asks first ("Disconnect and quit", "Keep running in the background", Cancel). Closing the window still only hides it.
4. **Start when you log in.** Turn on Settings → Background → Start when you log in. Log out and back in. *Expected:* only the tray icon appears, no window.

### Desktop sessions

5. **System shortcuts** (ADR 0018). In a connected desktop, press Super. *Expected:* Windows' Start menu opens. Press Right Ctrl, then Super. *Expected:* COSMIC's launcher opens.
6. **Keep the screen on** (ADR 0019). Turn on Settings → Display → Keep the screen on in full screen. Use a desktop full screen and wait past your screen-off time. *Expected:* the screen stays on. With another window in front, it blanks as usual.
7. **Reconnect after a lost connection** (ADR 0014). With a connected desktop, turn Wi-Fi off for about 30 seconds, then on. *Expected:* the row shows "Reconnecting (1 of 5)". Microsoft's sign-in window appears (your organization asks for a sign-in on every connection); sign in within 3 minutes (FreeRDP's limit per sign-in page). The desktop then comes back to the same Windows session. Selecting Stop instead ends the attempts.
8. **Display scaling** (ADR 0015). On a monitor with scaling other than 100 %, connect a desktop. *Expected:* text in Windows is the same size as in COSMIC's apps.
9. **Camera** (ADR 0013). Turn on the Camera setting for a desktop and connect. If the row says "Camera not shared: your organization doesn't allow cameras on this desktop" (or only specific cameras), this test doesn't apply to you: tell me, and I'll remove it. Otherwise open Windows' Camera app. *Expected:* the camera works. Unplug it and plug it back in, and note what happens (FreeRDP 3.32.2 doesn't notice a camera plugged in later, so it may be gone until you reconnect).

### Administrator policy (ADR 0021)

15. **Clipboard directions** (the both-ways part passed 2026-10-20). Set Clipboard to "Only to the desktop", reconnect: copying into the desktop works, copying out doesn't. Optionally turn on "Copy files" and copy a file in.

### Settings

16. **"Changes apply the next time you connect".** Connect a desktop, then open that desktop's settings. *Expected:* below the page's explanation, a separate line says it's connected and changes apply the next time you connect it. Its account's settings and View → Settings say connected desktops keep their settings until you connect again. With nothing connected, neither note shows.

### Lists

10. **Search, favorites, last update.** Type part of a desktop's name in the search field, mark a desktop as a favorite (the outline star), and look at the "Updated …" line. *Expected:* the list narrows, the favorite shows at the top under Favorites with a filled star, and the time is right. Select the filled star: it goes back to an outline and leaves Favorites.
11. **Offline list.** Turn networking off and start Deskfeed. *Expected:* the last list shows, marked "Offline list from …", with Connect turned off.

### Longer runs

14. **Stability** (spec §8, §13): a few hours of use with a desktop open, including idle time, changing audio devices, plugging in a monitor, and sleep.

## Done

- 2026-10-20: clipboard works after the fix (`9cd5e5c`): text copied both ways.
- 2026-10-20: sign in again after reinstalling: one Microsoft sign-in page, then the desktop list.
- 2026-10-20: every desktop still connects after the stricter connection-file checks (ADR 0024).
- 2026-10-19: tray menu lists the open desktop and switches to it.
- 2026-10-19: "Keep running in the background" in the quit dialog closes the window and keeps the desktop connected (fix `7fc05ab`).
