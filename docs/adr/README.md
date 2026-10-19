# Architecture decision records

Each significant decision is recorded as a short Markdown file, `NNNN-title.md`, numbered in order. See [ADR 0001](0001-record-architecture-decisions.md) for the format.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0002](0002-pin-libcosmic-revision.md) | Pin libcosmic to a reviewed revision | Accepted |
| [0003](0003-minimal-dependencies.md) | Keep dependencies minimal | Accepted |
| [0004](0004-freerdp-version-and-sign-in-helper.md) | FreeRDP 3.32.2 and the sign-in helper protocol | Proposed |
| [0005](0005-password-login-host-pools.md) | Password login on host pools without Entra single sign-on | Proposed |
| [0006](0006-build-app-alongside-compatibility-work.md) | Build the app alongside the compatibility work | Accepted |
| [0007](0007-sign-in-web-view.md) | Sign-in web view: WebKitGTK 6 in a separate helper process | Accepted |
| [0008](0008-http-and-crypto-crates.md) | HTTP, TLS and crypto crates | Accepted |
| [0009](0009-secret-service-over-zbus.md) | Secret Service over zbus, no keyring crate | Accepted |
| [0010](0010-native-only-and-dependency-check.md) | Native packages only, with a startup dependency check | Accepted |
| [0011](0011-connection-settings-and-new-workspace.md) | Connection settings per account and desktop; open on a new workspace | Accepted |
| [0012](0012-name-deskfeed.md) | The app is called Deskfeed | Accepted |
| [0013](0013-camera-redirection.md) | Camera redirection through FreeRDP's camera channel | Accepted |
| [0014](0014-reconnect-by-the-app.md) | The app reconnects lost sessions, not FreeRDP | Accepted |
| [0015](0015-display-scaling-from-xsettings.md) | Windows' display scaling follows the X11 scale from XSETTINGS | Accepted |
| [0016](0016-session-host-certificates.md) | Session host certificates: the broker's pin, else the user's approval | Accepted |
| [0017](0017-background-operation-and-tray.md) | Background operation with a StatusNotifierItem tray over zbus | Accepted |
| [0018](0018-system-shortcuts-through-xwayland-grab.md) | System shortcuts reach the desktop through XWayland's keyboard grab | Accepted |
| [0019](0019-keep-screen-on-in-full-screen.md) | Keep the screen on in full screen through org.freedesktop.ScreenSaver | Accepted |
| [0020](0020-remoteapps.md) | RemoteApps through FreeRDP's X11 RemoteApp mode, as a preview | Accepted (preview) |

Do not put tenant data (host names, tenant or workspace IDs, user names, feed URLs) in ADRs. Describe evidence in redacted form.
