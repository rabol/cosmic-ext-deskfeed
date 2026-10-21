# Compatibility

What has been tested, on what, and what isn't supported (spec §4, item 8). Evidence from real tenants is recorded in redacted form. The decisions behind these findings are in the [ADRs](adr/README.md).

## Matrix

| Configuration | Desktop list | Gateway | Signing in to the desktop | Notes |
|---|---|---|---|---|
| The owner's tenant; public client ID; host pool without the single sign-on property; `xfreerdp3` 3.32.2 | works | works | **Entra sign-in works.** Password logon is refused by the host (`0x52E`) | Since 2026-10-17. One Microsoft confirmation page per connection (below) |
| Host pool with single sign-on (`enablerdsaadauth:i:1`) | not tested | not tested | not tested | Needs such a host pool |
| RemoteApp from an AVD feed | not tested | not tested | not tested | No such app available; RemoteApps stay a preview ([ADR 0020](adr/0020-remoteapps.md)) |
| RemoteApp on a Windows 11 Pro host (home lab, no AVD) | — | — | works | App windows, takeover by a second app, refused and missing apps ([ADR 0020](adr/0020-remoteapps.md)) |
| Remembered Windows password | — | — | not tested | Needs a desktop that accepts password logon; the owner's refuses it (`0x52E`) |
| Window size, color depth, network type (local test server, `xfreerdp3` 3.32.2, 2026-10-20) | — | — | works | The client asks for the chosen size (80 % → 3072×1728 on a 3840×2160 monitor), 16 bpp and WAN (type 5); the defaults stay 1024×768, 32 bpp, autodetect. With tiling on, COSMIC resizes the window at once and the desktop follows |
| Shared folder (`/drive:`) | — | — | not tested | FreeRDP 3.32.2 takes the argument and loads its device channel; the local test server has no drive channel. Needs a Windows machine (manual test 24) |
| Span as one screen (`+span`) | — | — | not tested | Needs two monitors. FreeRDP accepts it and connects (local test server, one monitor) |
| HTTP proxy that needs a user name and password | works | **doesn't work** | — | FreeRDP 3.32.2 bug ([ADR 0008](adr/0008-http-and-crypto-crates.md) addendum) |
| Administrator-provided registration | — | — | — | Not in v1 (owner's decision) |

## Not supported

- **Password logon on host pools that block it on the host side.** The client can't fix this; Entra sign-in is used instead ([ADR 0005](adr/0005-password-login-host-pools.md)).
- **A Windows user name other than the one the AVD service supplies.** The service replaces it ([ADR 0005](adr/0005-password-login-host-pools.md) addendum).
- **Microsoft's identity broker** (`sso-mib`): not in the distribution's FreeRDP build ([ADR 0004](adr/0004-freerdp-version-and-sign-in-helper.md)).
- **Flatpak**: native packages only ([ADR 0010](adr/0010-native-only-and-dependency-check.md)).

## Findings on the owner's tenant

Dated, because these are Microsoft interfaces without documentation, and they can change.

- **Desktop list** (2026-10-17): the AVD feed service accepts the public client ID with `X-MS-User-Agent: com.microsoft.rdc.html/2.0.79.2 rdhtml-sdk/2.0.4`. After a renewed sign-in, feeds can come from another host under `wvd.microsoft.com` (seen: `rdweb-r1`). Microsoft lists `*.wvd.microsoft.com` as required for clients.
- **Password logon** (2026-10-17): RDSTLS refused it with `RDSTLS_RESULT_LOGON_FAILURE [0x0000052E]`.
  - Refused with every domain setting, with the password typed and pasted, and after the owner changed it. Also refused in the reference app.
  - Entra sign-in (`/sec:aad`) to the same host pool works, although its connection file doesn't ask for it.
  - The official Windows and macOS apps never ask for a Windows password there.
- **The confirmation page** "Are you trying to sign in to Azure Virtual Desktop Client?" appears once per connection, for the desktop's own token.
  - The app supplies the gateway token itself, which removed a second page.
  - A silent attempt (`prompt=none`) returns `interaction_required`.
  - FreeRDP 3.32.2 redeems that code itself with its per-connection key (`client/common/aad-helper-protocol.md`, `aad.c`), so the app can't avoid the page. The official apps don't show it.
- **Signing in to the desktop can take long.** FreeRDP's Entra step (`NEGO --> AAD` until `AAD --> MCS_CREATE_REQUEST`) took 0.4 to 54.9 s (2026-10-17, nine connections).
  - FreeRDP's default limit of 9 s caused `ERRCONNECT_ACTIVATION_TIMEOUT` (exit 131), so the app allows 120 s ("Time to wait for the desktop").
  - On 2026-10-20 one connection still went over 120 s, right after the previous session had closed; the next try took 54 s. The cause is on the host and unknown.
- **Sign-in frequency.** The tenant's Conditional Access asks for a full sign-in "every time" (AADSTS70043, "maximum allowed lifetime … 300" seconds, 2026-10-17).
  - A saved sign-in is then refused when renewed. The app deletes it and asks to sign in again, saying why.
  - Microsoft can also refuse a remembered web session at that point, so the sign-in page asks for the password again.
- **Entra doesn't return `client_info` on the sign-in redirect the helper sees** (2026-10-20), so the helper can't tell which account signed in; the check built on it was removed (ADR 0027).
- **Microsoft's `nativeclient` page** (2026-10-17) shows an anti-phishing warning and redirects to `/common/wrongplace`. So FreeRDP's own terminal sign-in (copy a URL, paste the answer) can't be used by people. Hence the sign-in window ([ADR 0007](adr/0007-sign-in-web-view.md)).
- **Without `login_hint`**, a browser already signed in to another Microsoft account picks that account. The app's per-account sign-in profiles, and since 2026-10-20 the hint, prevent this.
