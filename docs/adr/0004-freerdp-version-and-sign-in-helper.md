# 0004. FreeRDP 3.32.2 and the sign-in helper protocol

- Status: Proposed (needs the milestone 1 account tests below)
- Date: 2026-10-17

## Context

Spec §4 step 6 and §8: inspect the selected FreeRDP version's AVD gateway and Entra mechanisms before proposing a custom helper or patch.

### Installed build (developer machine, Arch Linux, COSMIC Wayland)

`freerdp 2:3.32.2-1`, `sdl-freerdp3 /buildconfig` reports version 3.32.2 (`97eff6ddfb`), the same release the reference app bundles. Relevant options:

| Option | Value | Meaning for us |
|---|---|---|
| `WITH_AAD` | ON | Entra gateway and session-host (RDSAAD) authentication available |
| `WITH_SSO_MIB` | OFF | No Microsoft Identity Broker support in the distro build |
| `WITH_WEBVIEW_AAD_AUTH_HELPER`, `WITH_QT_AAD_AUTH_HELPER`, `WITH_XDG_AAD_AUTH_HELPER` | OFF | No bundled sign-in helper; FreeRDP falls back to the terminal `Browse to:` prompt |
| `WITH_WAYLAND`, `WITH_X11`, `WITH_CLIENT_SDL3` | ON | `sdl-freerdp3`, `wlfreerdp3`, `xfreerdp3` all installed |
| `WITH_KRB5` | ON | Kerberos available for NLA |

### Findings from the 3.32.2 source

- **Helper protocol** (`client/common/aad-helper-protocol.md`, `client/common/aad_helper.c`): `/azure:auth-helper:<path>` starts the helper with `--cmdInFd=P<hex fd>` and `--cmdOutFd=P<hex fd>` (`P` = anonymous pipe, fd in lowercase hex; `winpr/libwinpr/handle/handle.c`). Newline-delimited JSON-RPC 2.0. FreeRDP sends `hello`, then `navigate {title, url, redirect_uri, timeout_ms}`; the helper answers with `{status:"ok", redirect_url}` or an error (`timeout`, `user_cancelled`, IdP error). Then `shutdown` and an `exit` notification; FreeRDP kills the helper after 3 seconds.
- **The helper has no OAuth logic.** FreeRDP builds the authorize URL, extracts the code, and does the token exchange itself, including the session host's proof-of-possession `req_cnf` (`client/common/client.c`). Our helper only has to show the URL and return the final redirect URL.
- **A new helper process per token request.** `client_helper_get_access_token_va` starts and stops the helper around each request, although the protocol doc says the instance is reused. Cookies therefore cannot live in the helper process; they must live in the app (or a long-lived sign-in process the app owns), which matches spec §8.
- **Fallback order** (`client_failsafe_get_access_token`): the SDL3 and X11 clients try the helper first and fall back to the terminal prompt. With `/azure:auth-helper:<path>` set, our helper is used.
- **The terminal fallback is unusable for people** (observed 2026-10-17): Microsoft's `nativeclient` page warns about phishing and redirects to `/common/wrongplace` after 3 seconds, so the code can't be copied from a normal browser. A helper must catch the redirect itself (see `docs/milestone-1.md`).
- **Gateway token request** uses `FreeRDP_GatewayAvdClientID` (default: Microsoft's public client ID) and redirect `…/oauth2/nativeclient`. `/azure:avd-scope:` changes the scope; `/azure:tenantid:` and `use-tenantid` change the authority.
- **`/args-from:fd:<n>` cannot be combined with other arguments.** All arguments, not just secrets, must come through the fd. Fine for us: the backend builds the full argument list and writes it to a private pipe.
- **`/from-stdin`** makes the SDL3 client read a missing password at a no-echo terminal prompt instead of its own dialog (`client/SDL/SDL3/dialogs/sdl_dialogs.cpp`). It also honours `FREERDP_ASKPASS`. The app must not use either path for passwords (spec §8: password via fd), but the research script uses `/from-stdin`.

### Reference app evidence (owner's machine, redacted, 2026-10-17)

The reference session log shows: helper detection failed (`aad_auth_helper_detect_helper: could not determine expected helper binary location`), the terminal `Browse to:` flow then got a gateway token, and RDSTLS later failed with `RDSTLS_RESULT_LOGON_FAILURE [0x0000052E]` / `ERRCONNECT_LOGON_FAILURE`. The reference app also writes the full `nativeclient?code=…` redirect URL to its session log, and stores feed XML and `.rdp` files with mode `0644`. We must not repeat either.

### Client comparison on COSMIC (2026-10-18)

Measured against a local test server (`research/rdp-test-server`: FreeRDP 3.32.2's sample server plus the display control channel, with the channel's capabilities delayed by 15 s like a real logon) on COSMIC (cosmic-comp `41497b4`, auto-tiling, output scale 1.25), with Deskfeed's arguments:

| | `sdl-freerdp3` (native Wayland) | `sdl-freerdp3` (XWayland) | `xfreerdp3` (XWayland) | `wlfreerdp3` (native Wayland) |
|---|---|---|---|---|
| Opens tiled | 1 of 5 (else floating) | yes | 4 of 4 | 4 of 4 |
| Sends the tile size once the display channel is ready | no | no | yes, every run | yes, every run |
| Size sent for a 1524×836 tile | window's own | — | 1904×1000 (physical pixels) | 3048×1600 (integer scale 2) |
| Non-ASCII title | correct | mangled (Latin-1) | correct | correct |

Causes, from the 3.32.2 source: the SDL client sends a layout only on a window event or from its retry timer, at most 5 × 1 s after it (`client/SDL/SDL3/sdl_disp.cpp`, `MAX_RETRIES`), so capabilities that arrive after a longer logon never get one; and it makes its window fixed-size right after creating it (`sdl_context.cpp`, `postConnect` → `setResizeable(false)`), which COSMIC treats as a dialog and floats when it maps the window then (cosmic-comp `src/shell/layout/mod.rs`, `is_dialog`). `wlfreerdp3` knows only the integer output scale, so Windows would draw a desktop 1.6× too large for the window. `xfreerdp3` gets physical pixels because COSMIC's default XWayland scaling is fractional (`cosmic-comp-config`, `XwaylandDescaling::Fractional`).

All three clients use `client_failsafe_get_access_token`, so the sign-in helper works with each (`client/X11/xf_client.c`, `client/Wayland/wlfreerdp.c`), and the X11 and SDL clients share their exit codes (`client/X11/xfreerdp.h`, `client/SDL/SDL3/sdl_utils.hpp`). Unlike the SDL client, `xfreerdp3` has no dialogs: an unverifiable certificate is asked about on the terminal (`client/common/client.c`, `client_cli_accept_certificate`), which reads `/dev/null` in the app and so refuses.

## Decision (proposed)

1. Target FreeRDP **3.32.2** as the first supported version, using the distro's **`xfreerdp3`** (X11 client, on COSMIC's XWayland). It was `sdl-freerdp3` until the client comparison above.
2. Integrate Entra token requests through **FreeRDP's own helper protocol** (`/azure:auth-helper:<path>`), with a small Deskfeed helper binary that forwards each `navigate` to the running app over a private, authenticated channel. No FreeRDP patch and no terminal scraping.
3. Pass the complete FreeRDP argument list through `/args-from:fd:<n>`.
4. Keep `sso-mib` as a later optional capability. The distro build doesn't include it, so it needs our own FreeRDP build (likely only in Flatpak).

## Tests needed before acceptance

- [ ] A research helper that speaks the protocol completes the gateway token request (`/sec:nla`) on the owner's tenant.
- [ ] The same helper completes the session-host token request (`/sec:aad`, proof-of-possession) on a host pool with Entra single sign-on, if one is available.
- [ ] (`research/loopback-redirect-test.sh`; unauthenticated checks are inconclusive, see `docs/milestone-1.md`) Whether Microsoft's public client ID accepts a loopback redirect (`http://localhost:<port>`). If it does, the system browser can replace the web view and the helper needs no web engine ([ADR 0003](0003-minimal-dependencies.md)). If it is rejected (`AADSTS50011`), record that.
- [x] Backend comparison on COSMIC (spec §4 step 7): `sdl-freerdp3` native Wayland, `sdl-freerdp3` on XWayland, `xfreerdp3` on XWayland, plus `wlfreerdp3` (2026-10-18, above). Still open: multi-hour stability, and a real Azure connection with `xfreerdp3`.

## Consequences

- If the helper protocol works, Deskfeed needs no FreeRDP patch for Entra.
- Because FreeRDP restarts the helper for each token, the app must hold the sign-in state, and the helper must find and authenticate to the app each time.
- The helper protocol is new in FreeRDP 3.x and may change; pin the version and recheck on upgrades.
