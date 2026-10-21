# Roadmap: what's still open

Only open work is listed here. What's done is in [CHANGELOG.md](../CHANGELOG.md), why it was done that way is in the [ADRs](adr/README.md), and the checks only the owner can do are in [manual-tests.md](manual-tests.md). Code cleanup is tracked in [cleanup-todo.md](cleanup-todo.md).

## Before the first release

- Tag `v0.1.0`, then fill in the AUR package checksum (review F14, F20).
- AppStream screenshots.
- The real-tenant checks in [manual-tests.md](manual-tests.md), including the stability run (spec §8, §13): several hours of use with idle time, audio device changes, a monitor plugged in, network changes, and suspend and resume.

## Next

- **Plain RDP connections, after the first version** ([ADR 0029](adr/0029-plain-rdp-connections.md#compared-with-remmina-follow-up-steps)): waiting on the owner for smartcard logon. Restricted admin mode: a specific message once manual test 25 shows what Windows answers when it doesn't allow it. Later decisions: RD Gateway, importing `.rdp` files.

## Open questions

- **"Client version not supported"** (spec §7): a specific message when the service rejects the client version. What the service sends in that case isn't known yet. It needs evidence (a real rejection, or Microsoft's documentation) before anything is built.
- **OIDC key scope** (review U6): whether the ID token's signing key should also be checked against the tenant it was issued for.
- **Pinning the tenant for the desktop's sign-in** (ADR 0027): `/azure:tenantid:<tid>,use-tenantid:on`. It waits for a test on the real tenant.

## Later, when possible

- **Camera:** a test in a real AVD session, including unplugging and plugging back in ([ADR 0013](adr/0013-camera-redirection.md)). Also a dependency-check item for FreeRDP builds without the camera channel. FreeRDP 3.32.2 can't pick specific cameras or notice ones plugged in later.
- **RemoteApps:** a preview until a user reports an app from an AVD feed working, or until there is an AVD host pool to test with ([ADR 0020](adr/0020-remoteapps.md)).
- **Choosing specific monitors** (beyond one or all): parked until a second monitor is available to test with. The groundwork is known:
  - FreeRDP numbers monitors in RandR's order (`client/X11/xf_monitor.c`; `/list:monitor` prints them) and takes `/monitors:<ids>` with `/multimon`.
  - Under XWayland, RandR's names are COSMIC's output names (`DP-1`), so a choice can be stored by name and turned into FreeRDP's numbers at launch, with x11rb's `randr` feature, which libcosmic already compiles in.

## Owner's calls


- **Report the FreeRDP proxy bug upstream:** HTTP proxy authentication drops the last character of the password ([ADR 0008](adr/0008-http-and-crypto-crates.md) addendum). The report would be public.

## Not in v1

- **Administrator registration and sign-in in the system browser** (spec §4, optional; review F10). Closed for v1 by the owner on 2026-10-19; it may come later. It needs a tenant where an administrator can register an app.

## Known deviation from the spec

The spec prefers native Wayland session windows (§9). Deskfeed uses `xfreerdp3` on XWayland, the compatibility mode §9 allows. The measurements are in [ADR 0004](adr/0004-freerdp-version-and-sign-in-helper.md) (client comparison, 2026-10-18).
