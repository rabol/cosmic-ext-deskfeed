# 0019. Keep the screen on in full screen through org.freedesktop.ScreenSaver

- Status: Accepted (checked on the live COSMIC session with the local test server, 2026-10-19)
- Date: 2026-10-19

## Context

Spec §9: prevent screen lock only while a full-screen session is focused, and only when the user turns it on.

FreeRDP 3.32.2's X11 client doesn't ask the desktop to stay awake (no screensaver or inhibit calls in `client/X11`). Under XWayland, X11 screensaver calls wouldn't reach COSMIC anyway.

COSMIC's idle daemon, cosmic-idle (`src/freedesktop_screensaver.rs`), serves `org.freedesktop.ScreenSaver` on the session bus:
- While any `Inhibit` is held, it drops its screen-off and suspend timers (`src/main.rs`, `recreate_notifications`), so the screen doesn't blank, lock or suspend.
- `UnInhibit(cookie)` releases a hold, and so does the caller's bus connection closing. A crashed app can't leave the screen on.

The Wayland `idle-inhibit` protocol would need a surface of Deskfeed's own in front, but the session's window belongs to FreeRDP.

## Decision

- New setting "Keep the screen on in full screen" (Display section, App, Account and Desktop levels), off by default.
- For each session started with it on, Deskfeed watches that session's window (by app ID and title, through COSMIC's window list, as for Show and ADR 0011). It holds an `Inhibit` while one of those windows is both activated and full screen, and releases it as soon as none is.
- The watcher runs only while such a session is open (an iced subscription keyed by the window titles), on its own thread. It checks once a second, because a blank or lock comes after minutes.
- The call goes over Deskfeed's own bus connection (`single.rs`), so the hold ends if Deskfeed exits.
- The log notes each hold and release.

## Consequences

- Live check with the local test server: a full-screen session held the block while in front (`Inhibit`, cookie 1). Switching to another window released it (`UnInhibit`), switching back held it again, and closing released it. A windowed session held nothing.
- Windows' own screen saver and lock inside the session are Windows' business and aren't affected.
