# Manual tests

Checks only a person at the real app can do:
- Microsoft's real sign-in and service;
- the real COSMIC desktop (panel, autostart, keyboard, workspaces, screen blanking);
- hardware, the network, sleep;
- how something looks.

The logic behind each test is covered by `cargo test`. Each test has its own file with why it is manual, what it needs, the steps and the expected result.

Install the current build first (`just install`, or the package).

## Statuses

- **To do:** can run on the owner's setup (an AVD account as a normal user, the Windows machine win-100, two monitors) and hasn't passed yet.
- **Passed (date):** the owner ran it and it behaved as expected.
- **Completed but not tested:** built and covered by automated tests as far as they reach, but the real check needs something the owner doesn't have: AVD administrator rights, administrator changes on Windows, or extra hardware. Not counted as open. The file says what was checked and what wasn't; [compatibility.md](../compatibility.md) lists it as "not tested".

When a test passes, set its status to "Passed (date)" in its file and here. When one fails, note what happened in its file: it's a bug. Expectations must match the owner's setup: an unknown administrator policy is a step ("if the row says …, stop"), not an expected result.

## To do (28)

| # | Test | Area | Status |
|---|---|---|---|
| 2 | [Sleep for over an hour](02-sleep-over-an-hour.md) | Sign-in and accounts | To do |
| 3b | [Sign-out when the keyring refuses](03b-sign-out-keyring-refuses.md) | Sign-in and accounts | To do |
| 3c | [Sign in while the keyring is locked](03c-sign-in-keyring-locked.md) | Sign-in and accounts | To do |
| 3e | [Your account is filled in on Microsoft's page](03e-account-filled-in.md) | Sign-in and accounts | To do |
| 4 | [Start when you log in](04-start-at-login.md) | Background and tray | To do |
| 4a | [Quit from the window](04a-quit-from-window.md) | Background and tray | To do |
| 4c | [Settings pages after the settings rework](04c-settings-after-rework.md) | Settings | To do |
| 4d | [The sign-in helper after it stopped polling](04d-sign-in-helper-no-polling.md) | Sign-in and accounts | To do |
| 4e | [The offline list while a sign-in has ended](04e-offline-list-sign-in-ended.md) | Sign-in and accounts | To do |
| 4f | [Signed-out accounts stay listed](04f-signed-out-accounts-stay.md) | Sign-in and accounts | To do |
| 5 | [System shortcuts](05-system-shortcuts.md) | Desktop sessions | To do |
| 6 | [Keep the screen on](06-keep-screen-on.md) | Desktop sessions | To do |
| 7 | [Reconnect after a lost connection (AVD)](07-reconnect-avd.md) | Desktop sessions | To do |
| 8 | [Display scaling](08-display-scaling.md) | How things look | To do |
| 10 | [Favorites and "Updated"](10-favorites-and-updated.md) | How things look | To do |
| 11 | [Offline list](11-offline-list.md) | How things look | To do |
| 14 | [Stability](14-stability.md) | Longer runs | To do |
| 15 | [Clipboard directions](15-clipboard-directions.md) | Desktop sessions | To do |
| 16 | ["Changes apply the next time you connect"](16-changes-apply-next-time.md) | How things look | To do |
| 18 | [Remembered password](18-remembered-password.md) | Plain RDP connections | To do |
| 19 | [Wrong password and domain](19-wrong-password-and-domain.md) | Plain RDP connections | To do |
| 20 | [Remove while connected](20-remove-while-connected.md) | Plain RDP connections | To do |
| 21 | [Reconnect (plain RDP)](21-reconnect-rdp.md) | Plain RDP connections | To do |
| 22 | [Tray and favorites (plain RDP)](22-tray-and-favorites-rdp.md) | Plain RDP connections | To do |
| 23 | [Window size, color depth, network type](23-window-size-depth-network.md) | Desktop sessions | To do |
| 24 | [Shared folder](24-shared-folder.md) | Desktop sessions | To do |
| 27 | [No flash on a new workspace](27-new-workspace-no-flash.md) | Desktop sessions | To do |
| 33 | [Use all monitors, and Span as one screen](33-all-monitors-and-span.md) | Desktop sessions | To do |

## Completed but not tested

| # | Test | Area | Status |
|---|---|---|---|
| 9 | [Camera](09-camera.md) | Desktop sessions | Completed but not tested |
| 25 | [Restricted admin mode](25-restricted-admin.md) | Plain RDP connections | Completed but not tested |

Test 24 (shared folder) is To do on win-100; its AVD half is completed but not tested.

## Passed

| # | Test | Area | Status |
|---|---|---|---|
| 3 | [Connect from the tray with the window closed](03-connect-from-tray.md) | Background and tray | Passed (2026-10-21) |
| 4b | [The tray on ksni](04b-tray-on-ksni.md) | Background and tray | Passed (2026-10-20) |
| 17 | [Plain RDP connection](17-plain-rdp-connection.md) | Plain RDP connections | Passed (2026-10-21) |
| 26 | [Open on a new workspace follows the window](26-new-workspace-follows.md) | Desktop sessions | Passed (2026-10-21) |
| 28 | [Clipboard both ways](28-clipboard-both-ways.md) | Desktop sessions | Passed (2026-10-20) |
| 29 | [Sign in again after reinstalling](29-sign-in-after-reinstall.md) | Sign-in and accounts | Passed (2026-10-20) |
| 30 | [Every desktop connects after the stricter connection-file checks](30-connection-file-checks.md) | Desktop sessions | Passed (2026-10-20) |
| 31 | [Tray menu lists the open desktop](31-tray-lists-open-desktop.md) | Background and tray | Passed (2026-10-19) |
| 32 | ["Keep running in the background" from the quit dialog](32-keep-running-from-quit-dialog.md) | Background and tray | Passed (2026-10-19) |
