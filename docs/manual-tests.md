# Manual tests still to do

Checks that only the owner can do, in the installed app with the real tenant. Each says what to do and what should happen. When one passes, move it to "Done" with the date. When one fails, note what happened and open it as a bug.

Install the current build first (`just install`, or the package).

## To do

### Sign-in and accounts

1. **Sign in again after reinstalling.** The saved sign-in had expired under the tenant's sign-in time limit. Reinstall, open Deskfeed, select Sign in on the account. *Expected:* one Microsoft sign-in page, then the desktop list.
2. **Sleep for over an hour** (commit `0b957ce`). Sign in, let the computer sleep for more than an hour, wake it, select Connect. *Expected:* the desktop connects, or the account shows a Sign in button. Never a raw error. Also, within 5 minutes of waking, the list's "Updated …" time changes to the time after waking, even after a short sleep (the log says "woke from sleep: refreshing desktop lists").

3a. **Sign out with a desktop open.** Connect a desktop, then select Sign out on its account. *Expected:* a question "Sign out of …?" with "Disconnect and sign out" and Cancel. Cancel keeps everything; the other button disconnects and signs out. Without anything connected, Sign out happens at once as before.

3b. **Sign-out when the keyring refuses.** Lock the keyring (for example in Passwords and Keys, if you have it), select Sign out, and cancel the unlock prompt. *Expected:* a warning "Sign-out couldn't remove all of the account's sign-in data" with a Try again button. Select Try again and unlock. *Expected:* the warning goes away, and the account doesn't come back after restarting Deskfeed.

3c. **Sign in while the keyring is locked.** Lock the keyring, sign in, and cancel the unlock prompt. *Expected:* the desktop list works, with a note that the sign-in couldn't be saved and a Try again button. Select Try again and unlock. *Expected:* the note goes away, and after restarting Deskfeed you're still signed in.

3d. **Cancel before a desktop starts.** Select Connect, and while the row says "Preparing" or "Waiting for your password", select Cancel. *Expected:* the row goes back to Connect, any password dialog for it closes, and nothing starts afterwards. With two desktops that both ask for a password, the second dialog appears after you answer or cancel the first.

3e. **The desktop sign-in uses the account** (ADR 0026). On a desktop that uses Entra sign-in, sign out of Microsoft in the sign-in window if one is remembered, or use a fresh account, then select Connect. *Expected:* if Microsoft's page appears, your account is already filled in or selected; the desktop connects; the log (View → Diagnostics) has no "account not confirmed" and no "different account" line. Then, if you have a second work account: when the page appears, choose "Use another account" and sign in with the other one. *Expected:* the row says "You signed in to the desktop with a different account…" (if it says only that the sign-in was rejected, note it: the helper's message didn't reach Deskfeed), and nothing reconnects. Select Connect once more. *Expected:* Microsoft's page shows the list of accounts; picking your own connects. If you use a guest account in another organization, try that too: it must connect normally.

### Background and tray (ADR 0017)

3. **Connect from the tray with the window closed.** Turn on Settings → Background, close the window, use the tray menu's "Connect …". *Expected:* the desktop connects without the window opening. A password question or a failure opens the window. The connected desktop has a check mark, and the top of the menu says "1 connected".
4. **Start when you log in.** Turn on Settings → Background → Start when you log in. Log out and back in. *Expected:* only the tray icon appears, no window.

### Desktop sessions

5. **System shortcuts** (ADR 0018). In a connected desktop, press Super. *Expected:* Windows' Start menu opens. Press Right Ctrl, then Super. *Expected:* COSMIC's launcher opens.
6. **Keep the screen on** (ADR 0019). Turn on Settings → Display → Keep the screen on in full screen. Use a desktop full screen and wait past your screen-off time. *Expected:* the screen stays on. With another window in front, it blanks as usual.
7. **Reconnect after a lost connection** (ADR 0014). With a connected desktop, turn Wi-Fi off for about 30 seconds, then on. *Expected:* the row shows "Reconnecting (1 of 5)" and the desktop comes back by itself. Stop ends the attempts.
8. **Display scaling** (ADR 0015). On a monitor with scaling other than 100 %, connect a desktop. *Expected:* text in Windows is the same size as in COSMIC's apps.
9. **Camera** (ADR 0013), on a desktop whose administrator allows cameras. Turn on the Camera setting, connect, open Windows' Camera app. *Expected:* the camera works. Unplug and plug it back in: note what happens.

### Connection files (ADR 0024)

14a. **Desktops still connect after the stricter connection-file checks.** Connect each of your desktops once. *Expected:* they connect as before. If one doesn't, the log has a line "connection file: dropped for their form: …" naming the property; tell me the name (it holds no tenant data).

### Administrator policy (ADR 0021)

15. **Clipboard and microphone still work where allowed.** With the clipboard on in Settings, connect your usual desktop and copy text both ways. *Expected:* it works. If it doesn't, the desktop's row says "Clipboard off: your organization doesn't allow it", and the log has a line "Device policy from the connection file" showing what the feed allows. Tell me what it says.

### Lists

10. **Search, favorites, last update.** Type part of a desktop's name in the search field, mark a desktop as a favorite (heart), and look at the "Updated …" line. *Expected:* the list narrows, the favorite shows at the top under Favorites, and the time is right.
11. **Offline list.** Turn networking off and start Deskfeed. *Expected:* the last list shows, marked "Offline list from …", with Connect turned off.

### Only when available

11a. **Editable Windows user name** (review F09), on a desktop that asks for a Windows password (your issue #38 host pool). *Expected:* the password dialog shows "Windows user name", filled with your account's name. Change it (for example to `DOMAIN\name` if your Windows logon uses that), connect. *Expected:* it signs in as that user, and the next time the dialog shows the name you typed. A malformed name (like `name@`) gives a clear message instead of trying.
12. **Remembered Windows password** (ADR 0009 addendum), on a desktop that asks for a Windows password. Tick "Remember this password", connect, disconnect, connect again. *Expected:* no password question the second time.
14. **Stability** (spec §8, §13): a few hours of use with a desktop open, including idle time, changing audio devices, plugging in a monitor, and sleep.

## Done

- 2026-10-19: tray menu lists the open desktop and switches to it.
- 2026-10-19: "Keep running in the background" in the quit dialog closes the window and keeps the desktop connected (fix `7fc05ab`).
