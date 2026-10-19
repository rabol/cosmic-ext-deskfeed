# 0020. RemoteApps through FreeRDP's X11 RemoteApp mode, as a preview

- Status: Accepted as a preview (checked on a Windows 11 Pro host with RemoteApp allowed, 2026-10-19; not yet with a RemoteApp from an AVD feed)
- Date: 2026-10-19

## Context

Spec §3 lists RemoteApps as parity work that needs real interoperability evidence before it is advertised as supported. Until now the feed's RemoteApps were listed but couldn't connect.

FreeRDP's RemoteApp support (MS-RDPERP, "RAIL") is in the X11 client, which Deskfeed already uses (ADR 0004). In FreeRDP 3.32.2:
- `client/common/file.c` reads `remoteapplicationmode`, `remoteapplicationprogram`, `remoteapplicationname`, `remoteapplicationcmdline`, `remoteapplicationexpandcmdline` and `remoteapplicationexpandworkingdir` from the connection file. `channels/rail/client/client_rails.c` sends the program and its arguments to Windows.
- With RemoteApp mode on, the client creates no desktop window. Each Windows window becomes an X11 window with the server's title. Its `WM_CLASS` is `/wm-class` when given (`client/X11/xf_window.c`, `xf_AppWindowCreate`), so COSMIC sees Deskfeed's app ID.
- The remote screen covers one monitor unless `/multimon` is given (`client/X11/xf_monitor.c`). Then app windows can't be moved to another monitor.
- When Windows refuses to start the program, the client logs `RAIL exec error: execResult=RAIL_EXEC_E_…` and cancels the connection (`xf_rail.c`), so it exits as "cancelled" (145).
- When another connection takes over the Windows session, the server sends `ERRINFO_DISCONNECTED_BY_OTHER_CONNECTION` and the client exits with 5 (`XF_EXIT_CONN_REPLACED`, `client/X11/xfreerdp.h`).

The owner's tenant has no RemoteApps and the owner can't publish one, so there is no AVD RemoteApp to test with. A Windows 11 Pro VM with RemoteApp allowed (`fDisabledAllowList` and `fAllowUnlistedRemotePrograms`) gives the same RemoteApp protocol without the AVD gateway or broker.

## Decision

- Connect works for RemoteApps. The resource is a RemoteApp when its connection file says `remoteapplicationmode:i:1`, not only when the feed's type says so. The list says "App (preview)".
- The connection file filter (`rdpfile.rs`) also keeps `remoteapplicationcmdline`, `remoteapplicationexpandcmdline` and `remoteapplicationexpandworkingdir`, which the administrator publishes. It drops `remoteapplicationfile` and `remoteapplicationicon`, which name files on this computer.
- For a RemoteApp, FreeRDP gets `/multimon` (app windows can go to every monitor), no `+f`, `-grab-keyboard` and `-toggle-fullscreen` (`Effective::freerdp_app_args`). The app skips "Keep the screen on" and "Open on a new workspace". Its settings page hides those rows and the keyboard section, and says why.
- `/wm-class` stays, so app windows group under Deskfeed's icon in the dock.
- A connected app has no Show button, and its tray entry opens Deskfeed's window. Show finds a desktop's window by the title Deskfeed gave it, and an app's windows have the titles Windows gives them.
- Two new ways a session can end, read from FreeRDP's exit code and log (`launcher.rs`):
  - `Replaced` (exit 5, or the `ERRINFO_DISCONNECTED_BY_OTHER_CONNECTION` line): "Another connection took over this session". Never reconnected, because two connections would keep taking the session from each other. This also applies to desktops opened from another device.
  - `AppRefused` (the `RAIL exec error` line): not allowed, not found, session locked, or other, each with its own message instead of a silent cancel. Never reconnected.
- The tray count and the quit question say "connected" instead of "desktops".
- Addendum (2026-10-19): an app whose session another app's connection took over says so. Exit code 1 (`ERRINFO_RPC_INITIATED_DISCONNECT`) becomes its own end, `DisconnectedByServer`, still a normal end for desktops. When a connected RemoteApp ends that way or as `Replaced`, and a newer RemoteApp connection of the same account is starting or connected (`decide::moved_to`), its row says "Its windows are in <app>'s connection" while that connection lasts. Connect on it takes the session back, and the other app's row then says the same about it.

## Evidence (Windows 11 Pro VM, FreeRDP 3.32.2, COSMIC on Wayland)

- Character Map as a RemoteApp: the window appeared about 5 s after connecting, titled "Character Map", activated, with Deskfeed's app ID (seen through COSMIC's window list, `workspace.rs` `watch_windows`).
- Deskfeed's own launcher (`launcher.rs` `windows_host_remote_app`): signing in, connecting, then connected; with full screen and the keyboard grab set, the app still got `/multimon` and no `+f`.
- A program that doesn't exist: `RAIL exec error: execResult=RAIL_EXEC_E_FILE_NOT_FOUND`, exit 145. Deskfeed now reports `AppRefused(NotFound)`.
- A second app while the first was open: the second connection took over the Windows session. The first connection ended, and the second showed both apps' windows. Windows 11 ended the first with `ERRINFO_RPC_INITIATED_DISCONNECT` (exit 1, a normal end), not with exit 5. AVD may send either. Both are never reconnected.
- When the last app closed, Windows signed the session out by itself after about 85 s (`ERRINFO_LOGOFF_BY_USER`, exit 12, a normal end).
- Disconnecting leaves the apps running in the Windows session. The next RemoteApp connection to that session shows them again.
- Once, FreeRDP caught the stop signal and didn't exit. Deskfeed's Disconnect already force-stops it after the grace period (`stop_all`).

## Consequences

- One FreeRDP process per app. FreeRDP's client can't start a second app on a connection that is already open, so a second app from the same host pool takes over the session, and its connection shows both apps' windows. The first app's row says where its windows went (addendum above; checked with two launcher sessions on the test host: the first ended as `DisconnectedByServer`). Disconnecting the second app closes both apps' windows.
- Two apps from different host pools don't share a session, so neither takes over the other. If one of them happens to be ended by the server while the other is starting, its row would wrongly say its windows moved. This needs both at the same moment and goes away when the other app's connection ends.
- A connection whose last app closed shows "Connected" until Windows signs the session out (about a minute and a half on the test host; on AVD it depends on the host pool's policies).
- Still to check with a RemoteApp from an AVD feed: the gateway and Entra sign-in in RemoteApp mode, `||alias` programs, which reason AVD sends when a second app takes over, and the sign-out policy. Until then RemoteApps stay marked as a preview (spec §3).
