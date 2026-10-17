# Milestone 1: compatibility investigation

Tracks spec §4. Since [ADR 0006](adr/0006-build-app-alongside-compatibility-work.md), the remaining items are tested with the app itself, not with the scripts in `research/`. Decisions go into [ADRs](adr/README.md). This file holds the plan, progress, and the compatibility matrix. Evidence from real tenants is recorded only in redacted form.

## Plan

| # | Step (spec §4) | Needs owner's account | Status |
|---|---|---|---|
| A | Pin libcosmic, set up ADRs, confirm the template builds | No | Done ([0002](adr/0002-pin-libcosmic-revision.md)) |
| B | Inspect FreeRDP 3.32.2: helper protocol, gateway token, RDSTLS, `/args-from` (step 6) | No | Done ([0004](adr/0004-freerdp-version-and-sign-in-helper.md)) |
| C | Password login on the owner's host pool: find the accepted user name format (step 6, issue #38) | **Yes** | Script ready: `research/password-login-test.sh` ([0005](adr/0005-password-login-host-pools.md)) |
| C2 | User name handling: follow FreeRDP upstream (no `/u:` `/d:` by default; service supplies the user name, domain `AzureAD`); validate optional overrides | No | Done: `src/sessions/username.rs`, 8 tests ([0005](adr/0005-password-login-host-pools.md)) |
| D | Loopback redirect with the public client ID (steps 1–2; decides whether a web view is needed) | **Yes** | Script ready: `research/loopback-redirect-test.sh` |
| E | Research sign-in helper speaking FreeRDP's protocol; gateway token without terminal prompts (step 6) | Build: no. Test: **yes** | Not started |
| F | Feed discovery probe: endpoints, `X-MS-User-Agent`, client ID acceptance per stage (steps 1, 3, 4) | Build: no. Test: **yes** | Not started |
| G | Backend comparison and session stability on COSMIC (step 7) | **Yes** | Not started. Step C gives the first `sdl-freerdp3` native Wayland data point |
| H | Administrator-provided registration mode (steps 2–3, 5) | Needs a tenant admin | Not started; may be documented as unavailable |
| I | Flatpak validation of both sign-in routes (step 5) | **Yes** | After E/F |
| J | Compatibility matrix and limitations (step 8) | — | Ongoing, below |

## Observations

Dated because these are undocumented Microsoft interfaces.

- 2026-10-17: unauthenticated `GET /organizations/oauth2/v2.0/authorize` with the public client ID returns the normal sign-in page (`sErrorCode` 50058, "not signed in") for every redirect tried, including obviously invalid ones. Redirect validation happens after account selection, so step D needs a real sign-in. An unknown client ID returns 50059 instead.
- 2026-10-17: opening `https://login.microsoftonline.com/common/oauth2/nativeclient?code=…` in a normal browser shows an anti-phishing warning ("This page is not normally shown and could be a sign of a phishing attempt … do not copy or share the URL") and after 3 seconds redirects to `/common/wrongplace`. FreeRDP's terminal `Browse to:` / `Paste redirect URL here:` flow is therefore not usable by people any more. Copying the code with browser developer tools would work around this protection, so we don't use or document it (spec §4 step 4). Consequence: with the public client ID, sign-in needs a web view that catches the redirect before the page loads (as the reference app does), unless the loopback redirect test (step D) succeeds.
- 2026-10-17: a normal browser that is already signed in to another Microsoft account (here a personal Microsoft 365 account) picks that account automatically for the FreeRDP authorize URL, which has no `login_hint`. The app's per-account sign-in cookies (spec §6) avoid this; research tests use a private browser window.
- 2026-10-17 (reference app log, owner's tenant): the gateway token via `Browse to:` worked (answered by the reference app's hidden web view, not by copying); RDSTLS then failed with `0x0000052E`. See [0005](adr/0005-password-login-host-pools.md).

- 2026-10-17 11:54 (reference app, owner's tenant, Advanced flags `/d:AzureAD`, password typed by hand into FreeRDP's SDL dialog): again `RDSTLS_RESULT_LOGON_FAILURE [0x0000052E]`. Both failures share a hand-typed 32-character password. Next: the same with the password pasted via `research/reference-app-paste-password.sh` (`FREERDP_ASKPASS`), first with `/d:AzureAD`, then with no Advanced flags.
- 2026-10-17 12:13 (reference app, `/d:AzureAD`, password pasted via `FREERDP_ASKPASS`, no dialog): `0x52E` again, 10 s after the gateway step. A typing error is now unlikely.
- FreeRDP history: commit `967793134` (2025-09-26, "[core,arm] fix domain setting"): "While AzureAD seems to be required for many setups, some need an empty domain (or maybe a different all together?)". FreeRDP issue #11887 (fixed in 3.18): an AVD setup that needed an empty domain; the same reporter also connected by forcing Entra authentication to the session host (`enablerdsaadauth:i:1`) although the host pool didn't set it. Issue #10298: `RDSTLS_RESULT_LOGON_FAILURE` with several formats; the maintainer calls the right format "hard to determine". No later FreeRDP change to `arm.c`/`rdstls.c` addresses this (checked `3.32.2..master`).
- Next tests: (1) empty domain (no Advanced flags) with a pasted password; (2) Advanced flags `/sec:aad` (Entra authentication to the session host, no password).

## Compatibility matrix

| Configuration | Feed discovery | Gateway | Session host | Notes |
|---|---|---|---|---|
| Public client ID, host pool without SSO, `sdl-freerdp3` 3.32.2 native Wayland, native build | not yet tested | works (reference app log) | `0x52E` with UPN + empty domain (reference app) | Step C |
| Public client ID, host pool with SSO (`/sec:aad`) | not yet tested | not yet tested | not yet tested | Needs a host pool with SSO |
| Admin-provided registration | not yet tested | not yet tested | not yet tested | Step H |
