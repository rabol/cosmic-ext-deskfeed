# 34. After the app ID change

- **Status:** To do
- **Area:** Background and tray
- **Why it's manual:** The launcher, dock, panel tray and login autostart are COSMIC's; the sign-in is Microsoft's.
- **Needs:** Your AVD account (as a normal user), and win-100 for the saved connection.

The app ID changed from `io.github.rabol.cosmic-ext-deskfeed` to `dev.rabol.deskfeed` (ADR 0012 addendum). Nothing is carried over, so you start empty.

## Steps

1. Quit Deskfeed (Account menu → Quit). Remove the old install: `sudo just appid=io.github.rabol.cosmic-ext-deskfeed uninstall`. Then build and install the new one: `just build-release` and `sudo just install`.
2. Open the launcher and search for "Deskfeed".
3. Start Deskfeed, sign in, and connect to an Azure desktop.
4. Add the win-100 connection again and connect to it.
5. With the window open, start Deskfeed again from the launcher.
6. Turn on App settings → Background → "Start when you log in", log out of COSMIC and log back in.
7. Optional cleanup: delete `~/.config/cosmic/io.github.rabol.cosmic-ext-deskfeed/` and `~/.config/autostart/io.github.rabol.cosmic-ext-deskfeed.desktop` if it exists, and in Seahorse (Passwords and Keys) delete the old Deskfeed items.

## Expected

- Step 2: one Deskfeed entry, with its icon (not two).
- Step 3: the window, Microsoft's sign-in window and the desktop's window all show with Deskfeed's icon and group together in the dock.
- Step 5: no second copy; the open window comes to the front.
- Step 6: Deskfeed starts in the tray at login.
