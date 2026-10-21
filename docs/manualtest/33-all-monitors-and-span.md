# 33. Use all monitors, and Span as one screen

- **Status:** To do
- **Area:** Desktop sessions
- **Why it's manual:** Needs two real monitors; the local test server can't show how Windows lays them out.
- **Needs:** Your AVD account (as a normal user) and a desktop to connect to. Or win-100. Both monitors connected.
- **Background:** [ADR 0011](../adr/0011-connection-settings-and-new-workspace.md)

## Steps

1. Connect both monitors.
2. In the desktop's settings, turn on Full screen and Use all monitors, with Span as one screen off. Connect.
3. In Windows, open Settings → System → Display.
4. Disconnect, turn on Span as one screen, and connect again.
5. Look at Settings → System → Display again, and maximize a window in Windows.

## Expected

- Use all monitors: the desktop covers both monitors, and Windows lists two displays; a maximized window fills one monitor.
- Span: the desktop covers both monitors, Windows lists one wide display, and a maximized window stretches over both.

## Notes

Span is not tested anywhere yet (compatibility.md): FreeRDP accepts `+span` and connects with one monitor. Note the monitors' resolutions and scaling if something looks wrong.
