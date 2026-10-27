# 6. Keep the screen on

- **Status:** Automated (2026-10-27)
- **Area:** Desktop sessions
- **Why it's manual:** Needs real screen blanking.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to. Or win-100.
- **Background:** [ADR 0019](../../adr/0019-keep-screen-on-in-full-screen.md)

## Steps

1. Turn on Settings → Display → Keep the screen on in full screen.
2. Connect a desktop in full screen and leave it past your screen-off time.
3. Then put another window in front and wait again.

## Expected

- In full screen: the screen stays on.
- With another window in front: it blanks as usual.

## Automated

`just session-checks` (`nested_keep_awake`) runs a desktop in a window and then one in full screen, in a nested COSMIC session against the local test server, with a stand-in for COSMIC's `org.freedesktop.ScreenSaver`. Deskfeed asks to keep the screen on only for the full-screen desktop, once, as "Deskfeed", and lets go when it ends.

Not checked: that COSMIC then really keeps the screen on. On the owner's session cosmic-idle serves `org.freedesktop.ScreenSaver` with these `Inhibit`/`UnInhibit` methods (checked 2026-10-27); honouring them is COSMIC's part.
