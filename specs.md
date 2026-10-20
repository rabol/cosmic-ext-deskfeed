# Deskfeed (cosmic-ext-deskfeed) specification

Status: proposed implementation specification. Created 2026-10-17.

## 1. Project identity and purpose

- Project, executable, Cargo package, and GitHub repository: `cosmic-ext-deskfeed` (the `cosmic-ext-` namespace the [COSMIC trademark policy](https://github.com/pop-os/cosmic-epoch/blob/master/TRADEMARK.md) asks third-party apps to use).
- Display name: Deskfeed, described as "for the COSMIC™ desktop". Do not call it "Deskfeed" or otherwise imply official COSMIC status. In this document, "Deskfeed" means this application.
- App ID: `io.github.rabol.cosmic-ext-deskfeed`. GitHub owner: `rabol`. Repository: `https://github.com/rabol/cosmic-ext-deskfeed`. Copyright holder: Steen Rabol.
- Project directory: `/home/srabol/code/cosmic-ext-deskfeed`, its own Git repository.
- License for original project code: MIT, with `SPDX-License-Identifier: MIT` where appropriate.
- Platform: Linux, designed first for the COSMIC desktop on Wayland.
- Purpose: sign in to a work or school account, discover entitled Azure Virtual Desktop and Windows 365 resources, and launch and manage desktop and RemoteApp sessions.
- Primary goal: a client that works reliably on COSMIC. The reference application (§3) signs in and lists resources, but connecting fails for the owner on COSMIC (§3, issue #38). Reliable connection and session stability are core requirements, not polish.
- Independent community application; do not imply Microsoft or System76 endorsement.
- Future hosting: the public GitHub repository above; creating and publishing it is a later, explicit step.

## 2. Foundation and implementation constraints

Generate the application from the official [pop-os/cosmic-app-template](https://github.com/pop-os/cosmic-app-template), using its documented `cargo generate` workflow when implementation begins. The project was generated with `cargo generate --init` from template revision `97ff759e658922d7f02e708b10cfda421352b100` (2026-10-17). The template hardcoded `APP_ID = "dev.mmurphy.Test"` in `src/app.rs`; this was corrected to the app ID above.

Use Rust and [libcosmic](https://github.com/pop-os/libcosmic) for the native application interface. Retain the template's application lifecycle, localization structure, resources, desktop integration, and `just` build conventions. Follow current libcosmic patterns for navigation, dialogs, themes, configuration, and asynchronous messages. Pin Git dependencies to reviewed revisions and commit `Cargo.lock` for reproducible application builds.

The main interface must not be GTK, Electron, a web application, or an embedded browser shell. The only exception is the separate, sign-in-only web view described in §6. FreeRDP is the protocol implementation; do not write an RDP stack. Deskfeed handles account and resource discovery, configuration, and session supervision. FreeRDP handles RDP transport, AVD gateway connections, and gateway/session-host authentication supported by the selected version and build. Reuse FreeRDP's existing authentication mechanisms; add custom integration only where a demonstrated gap requires it. Prefer maintained Rust libraries for HTTP, OAuth/OIDC, XML parsing, Secret Service integration, and asynchronous work. Select exact crates after the initial compatibility investigation rather than treating an untested library choice as settled.

Use the reference project as behavioral and interoperability research, not as a line-for-line port. Preserve MIT attribution for any reused source or assets. Original application code remains MIT; third-party libraries and bundled binaries retain their respective licenses and notices.

### Security assumptions

Protect against network attackers, malicious or malformed feed content, other local user accounts, and untargeted local processes of the same user (for example, a process that can connect to a loopback port or read world-readable files). Out of scope: malware running as the same user with debugging or display access, a compromised Microsoft service, and a compromised session host. These assumptions are the reason for the loopback, IPC, and file-permission rules below.

## 3. Outcomes and scope

The release target is the useful workflow of [avd_feed_connect](https://github.com/shakeelosmani/avd_feed_connect), working reliably on COSMIC Wayland, with native COSMIC presentation and explicit security and lifecycle guarantees.

### Reference application findings

Inspected at the commit listed in §15:

- **Sign-in:** uses Microsoft's public Remote Desktop client ID `a85cf173-4192-42f8-81fa-777a763e6e2c`, tenant `organizations`, scope `https://www.wvd.microsoft.com/.default offline_access openid profile`, and redirect `https://login.microsoftonline.com/common/oauth2/nativeclient`. That redirect can't be returned to an app by a normal browser, so the reference app catches it in an embedded WebKitGTK view with a persistent cookie store.
- **Discovery:** `https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery` returns per-tenant feed URLs; each feed lists resources with `.rdp` file URLs and icons. The feed service rejects requests without an approved `X-MS-User-Agent` (the reference sends `com.microsoft.rdc.html/2.0.79.2 rdhtml-sdk/2.0.4`).
- **Connection:** bundled `sdl-freerdp` (FreeRDP 3.32.2, SDL3, `WITH_WEBVIEW=OFF`) with `/gateway:type:arm /sec:aad`. FreeRDP's session-host sign-in prompt (`Browse to:`) is read over a pseudo-terminal and answered by a hidden web view that shares the sign-in cookies. FreeRDP itself defaults to the same Microsoft client ID and redirect (`libfreerdp/core/settings.c`).
- **Session risks:** the owner's documented failure is the connection failure below (issue #38); no session crash has been captured yet. Reference choices likely to cause session problems on COSMIC: everything is forced onto X11/XWayland (`--socket=x11`) because its SDL client was unstable on native Wayland; a fixed `/scale-desktop:200`; GNOME-runtime WebKit rendering workarounds; and FreeRDP hotkeys changed by editing the user's `sdl-freerdp.json`. Any session failures must be captured and reproduced in milestone 1 before the backend configuration is chosen.
- **Password prompt on connect ([issue #38](https://github.com/shakeelosmani/avd_feed_connect/issues/38), the owner's report on Arch Linux + COSMIC):** the owner's host pool `.rdp` has no `enablerdsaadauth:i:1` (it has `enablecredsspsupport:i:1`, `promptcredentialonce:i:1`), so the session host needs the user's Windows password; Entra single sign-on is not enabled for it. The reference then switches to `/sec:nla` with an empty domain, FreeRDP's ARM path uses RDSTLS, and the SDL client shows its own credential dialog, which does not accept paste. The log shows `RDSTLS_RESULT_LOGON_FAILURE [0x0000052E]` (wrong user name or password) after entry. The user-visible result was "nothing happens" instead of an error. On later attempts, the gateway token request first shows the Microsoft account picker, then the password dialog. The macOS and Windows clients likely use credentials they have saved; this needs confirming.
- **FreeRDP sign-in helper not used:** the same log shows FreeRDP 3.32.2 looking for an out-of-process sign-in helper (`aad_auth_helper_detect_helper: could not determine expected helper binary location`) before falling back to the terminal prompt.
- **Security gaps not to copy:** plaintext `0600` token fallback, ID tokens decoded without validation, broad `*.microsoft.com` token trust, an optional `/cert:ignore`, raw feed XML and `.rdp` files saved to disk, and `--device=all` in the Flatpak.

### Initial usable release

- Several signed-in organizational accounts side by side (owner decision 2026-10-17), each including resources from multiple entitled tenant feeds, each with its own connection defaults.
- Entra sign-in in both identity modes (§4, §6), MFA interaction, secure remembered sessions, and silent renewal where policy permits.
- Discovery and refresh of workspaces, desktops, Windows 365 Cloud PCs, and RemoteApps exposed through the validated feed integration.
- Resource icons, grouping, search, favorites, last successful refresh time, and clearly marked cached resources.
- Connect, cancel connection, focus an existing session where supported, disconnect, and reconnect.
- Windowed and fullscreen desktop sessions, dynamic resolution, display scaling that follows the monitor's scale, and a choice between one monitor and all monitors.
- Stable sessions: no freezes, crashes, or lost input in normal use, including audio device changes, display changes, network changes, and suspend/resume (§8, §9).
- Audio playback and opt-in microphone and clipboard redirection.
- Per-resource settings, safe diagnostics, and actionable failures.

### Subsequent parity work

Tray resource menus and session indicators, optional background operation, start minimized, multiple saved accounts, and richer device controls follow the validated core. Camera redirection, RemoteApp, and choosing specific monitors (beyond one or all) require real interoperability evidence before being advertised as supported. Track outstanding parity features openly; the first usable release need not claim full parity.

### Outside initial scope

Generic arbitrary-host RDP connection management, RD Gateway administration, Cloud PC provisioning, host-pool management, Microsoft Graph administration, sovereign clouds, mobile clients, Teams media optimization, printer and smartcard redirection, and implementing device compliance or an identity broker ourselves are excluded. Using an installed Microsoft Identity Broker through FreeRDP is allowed (§6). A cloud profile architecture may accommodate sovereign clouds later without weakening endpoint validation.

## 4. Compatibility investigation: first implementation milestone

Before substantial UI work, prove the complete sign-in → discovery → connection path in an authorized test environment. Authentication to the feed, gateway, and session host are separate concerns; success at one does not prove the others.

1. Identify the supported service endpoints, token audiences, delegated scopes, tenant handling, redirect registrations, and protocol requirements for each stage.
2. Support two sign-in identities, chosen per account:
   - **Default: Microsoft's public Remote Desktop client ID** (`a85cf173-…`), the identity FreeRDP and the reference app use. Its only usable redirect is `…/oauth2/nativeclient`, so sign-in uses the restricted sign-in web view (§6). This works in most tenants without administrator action. State plainly in the app and README that Deskfeed is unofficial, uses this public client identity for interoperability, and is not a Microsoft product; never use Microsoft branding.
   - **Optional: administrator-provided registration.** *Not in v1 (owner's decision, 2026-10-19; review F10); may come later.* A public-client registration that a tenant administrator creates and consents to (AVD delegated permission plus session-host sign-in), with a loopback redirect and the system browser. Native applications must not contain client secrets. Deskfeed does not ship its own multi-tenant registration unless milestone 1 shows the feed and gateway accept it without per-tenant administrator work.
3. Test feed, gateway, and session-host access with both identities. Record which stages accept a non-Microsoft client ID, which require the approved `X-MS-User-Agent`, and how the service reacts to client version strings. Plan for that header to need occasional updates.
4. If Microsoft blocks the default identity or the header, document the evidence and rely on the administrator-provided mode; do not work around new restrictions. These interfaces are not documented public APIs, so record the observed behavior and its date.
5. Validate both sign-in routes in the native build, including gateway/session-host authentication. There is no Flatpak ([ADR 0010](docs/adr/0010-native-only-and-dependency-check.md)). Do not promise a single sign-in or silent connection authentication until tested.
6. Inspect and test the selected FreeRDP version's AVD gateway and Entra mechanisms before proposing a custom helper or patch: the out-of-process sign-in helper protocol (`/azure:auth-helper:<path>`, documented in `client/common/aad-helper-protocol.md`), the `GetAccessToken` callback, `/args-from`, and optional Microsoft Identity Broker support (`sso-mib`). Also test host pools without Entra single sign-on (password/RDSTLS login), including which user name and domain format they accept. Distinguish upstream functionality from gaps in how Deskfeed invokes and supervises it. Record the selected version, build options, capabilities, integration approach, and any demonstrated need for patches in an architecture decision record.
7. Reproduce the reference app's session failures on COSMIC and record the symptoms and logs. Compare `sdl-freerdp` on native Wayland, `sdl-freerdp` on XWayland, and `xfreerdp` on XWayland for stability, input, display changes, scaling, audio, clipboard, and RemoteApp. Choose the default backend from this evidence.
8. Produce a compatibility matrix describing the configurations actually tested and the unsupported cases. Include Conditional Access failures and distinguish missing assignments from unsupported authentication.

Note (2026-10-17, [ADR 0006](docs/adr/0006-build-app-alongside-compatibility-work.md)): the owner decided to build the app alongside this investigation. Remaining questions are answered with the app itself rather than research scripts; the completion criterion below still applies before release.

Completion criterion: end-to-end tests with both identity modes (or documented evidence that one is unavailable), a stable multi-hour session on COSMIC with the chosen backend, documented limitations, and a reproducible backend configuration. Unresolved service access, authentication, or session stability is a release blocker. Do not replace discovery with a local connection-file importer and call it feature parity.

## 5. User experience

### Main window

Use a COSMIC header bar, account controls, a workspace navigation area, search, and a responsive resource grid or list. Resource entries show title, type, workspace, icon, favorite state, and session status. Status must use text or accessible labels as well as color. Provide obvious refresh and connect actions, keyboard navigation, meaningful focus order, and empty states for first launch, no assignments, filtered results, offline operation, and partial feed failure.

Follow the desktop theme by default and allow light/dark overrides. Use libcosmic spacing, typography, icons, context drawers, and dialogs rather than imitating the reference project's GTK interface. All user-facing strings must be localizable from the start. Provide accessible names, visible focus, and readable scaling on mixed-DPI displays.

### Sign-in

With the default identity, open the sign-in web view window; with an administrator-provided registration, open the system browser. Either way, show a cancellable waiting state. Return focus to the application after a valid callback where the desktop permits it. Explain policy denial, administrator consent requirements, expired sessions, unavailable keyrings, and user cancellation distinctly. Never ask users to paste authorization URLs or tokens into a field for normal operation.

### Settings

Support global defaults and resource overrides for display mode, supported monitors, scaling, audio, microphone, camera, clipboard direction, and keyboard shortcut behavior (including the backend's local hotkeys, set per session and never by editing the user's FreeRDP configuration). Show unavailable backend capabilities as unavailable with a reason. Do not expose arbitrary FreeRDP command-line text as a normal setting. Respect server policy and show effective behavior when policy overrides a preference.

### Background operation

Closing the main window normally exits. When sessions are active, explain that quitting disconnects them and provide explicit choices to cancel, disconnect and quit, or continue in the background if enabled. Background operation and autostart are opt-in. Tray availability must be detected; do not leave an invisible application with no supported way to reopen it. Disconnect ends the local connection, not the user's remote Windows logon session.

## 6. Authentication and credential handling

- Use authorization code flow with PKCE S256 in both identity modes. Device code may be an explicit, separately validated option, never an automatic fallback after policy denial.
- Generate fresh unpredictable state and verifier values for every attempt. Require state to be present and match exactly; reject missing state, replay, wrong callback paths, malformed responses, and callbacks after cancellation or expiry.
- **Sign-in web view (default identity):** a separate, minimal helper process (WebKitGTK or another maintained engine), used only for Microsoft sign-in. It starts at `https://login.microsoftonline.com`, allows only HTTPS, shows the current host to the user, blocks pop-up windows, downloads, and file access, exposes no script bridge to the app, and stops at the `nativeclient` redirect. Only `code`, `state`, and error fields go back to the app over a private pipe. Federated identity-provider pages (ADFS, Okta, and similar) reached by sign-in redirects must work. Its cookie store belongs to one account, lives in the app's private data directory, and is deleted on sign-out. The same helper answers FreeRDP's session-host token requests (§8), showing a window only when the user must interact.
- **System browser (administrator registration):** bind callback listeners only to loopback, before opening the browser. Use a registration-compatible redirect URI, a bounded listener lifetime, and single-use completion. Do not bind to all network interfaces. Validate the Flatpak browser/callback route explicitly.
- Use vetted OAuth/OIDC handling. Validate ID tokens used for identity, including signature, issuer, audience, expiration, and nonce where applicable; decoding a JWT payload is not validation. Do not infer authorization from display names or unverified claims.
- Keep access tokens in memory. Store refresh tokens only in the desktop Secret Service with account, tenant, authority, and client identity separation.
- If the keyring is unavailable, offer an explicit memory-only session or retry. Never fall back to plaintext credential files, including files with `0600` permissions.
- Serialize refresh per account. Rotate persisted refresh tokens safely and distinguish transient network errors from invalid grants or interactive policy requirements. Use bounded retry with backoff and cancellation; do not open spontaneous sign-in windows.
- Sign-out invalidates outstanding operations, stops refresh, removes local credentials and account caches, and prevents late callbacks from restoring the account. If keyring deletion fails, report that removal is incomplete and offer retry. Sign-out deletes the sign-in web view's cookies. In system-browser mode, explain that the browser's Microsoft session is independent and is not cleared by local sign-out.
- Sign-out with active sessions offers disconnect-and-sign-out or cancellation. No child session or authentication helper should become orphaned.
- On Intune-enrolled Linux devices with the Microsoft Identity Broker installed, FreeRDP's optional `sso-mib` support can satisfy Conditional Access device-compliance policies. Offer it as an optional, detected capability; elsewhere, explain device-compliance denials clearly.
- Do not put passwords, access/refresh tokens, authorization codes, sensitive redirect URLs, or reusable session credentials in logs, process arguments, environment variables, crash reports, or persistent temporary files.

## 7. HTTP, feed, and resource security

Treat network metadata, XML, icons, connection files, resource names, and backend output as untrusted inputs.

- Use one shared HTTP policy for discovery, feeds, connection files, and icons. Validate HTTPS scheme, normalized hostname, port, and intended endpoint role before attaching credentials; reject URL userinfo and malformed authorities.
- Disable automatic authenticated redirects. Either reject them or follow a bounded chain through explicit validation on every hop. Never forward bearer credentials to a different origin by default, even if both hosts are Microsoft-owned. Reject HTTPS downgrades.
- Maintain reviewed endpoint-role allowlists and token-audience bindings derived from the validated service integration. Broad `*.microsoft.com` matching alone is insufficient. User-supplied feed URLs must not receive an existing token merely because the user entered them.
- Preserve TLS chain and hostname validation. Support normal system trust and documented enterprise CA installation; provide no global ignore-TLS switch.
- Honor system proxy settings (environment variables) and pass an equivalent proxy to FreeRDP. Proxy credentials follow the same rules as tokens. Endpoint and credential rules apply to the destination, not the proxy.
- Keep the `X-MS-User-Agent` client string as one reviewed value owned by the service integration. When the service reports an incompatible client version, show an error that says so instead of a generic failure.
- Refresh feeds at startup, after resume, on request, and on a bounded interval (default one hour, with jitter). Never refresh in a tight loop after errors.
- Bound request time, redirects, response/decompressed body sizes, XML depth and resource counts, and icon dimensions. Disable external entity resolution and network/file access from XML parsing. Reject unsupported formats and malformed connection properties safely.
- Parse connection files into a typed, reviewed property set. Do not execute feed-supplied commands or silently enable local drives, arbitrary paths, USB devices, or broad device access through file properties. Validate gateway and session-host destinations separately from feed HTTP endpoints, using the established AVD routing and identity model.
- Never use resource titles as storage identity. Key resources by account, tenant, workspace, and stable resource identifier; use opaque filesystem names. Prevent traversal, title collisions, cross-account cache reuse, and concurrent session-file overwrites.
- Cache only the non-secret metadata needed for the interface, with schema versioning, atomic writes, restrictive permissions, size/age limits, and account isolation. Do not persist raw authenticated feed responses or connection files by default.
- Offline cache entries are informational. A successful live authorization/connection flow is required to connect; an old entitlement cache never grants access.

## 8. RDP backend and session lifecycle

Start with a supervised FreeRDP process behind a Rust backend abstraction. Use the selected FreeRDP version's existing gateway/session-host authentication and helper interfaces wherever supported; Deskfeed should orchestrate those mechanisms rather than duplicate them.

FreeRDP 3.32.2 supports an out-of-process sign-in helper: it starts the binary named by `/azure:auth-helper:<path>` and talks newline-delimited JSON-RPC to it over two private pipes (`hello`, then `navigate` with a URL and the redirect prefix to wait for). Deskfeed ships its own small helper that speaks this protocol and forwards each request to the running app, which completes it with the sign-in web view and the account's cookies (silently when possible) or the system browser, depending on the identity mode. This replaces terminal scraping without patching FreeRDP. Only if the helper protocol proves insufficient, fall back to an upstreamable patch around the `GetAccessToken` callback or a minimal client built on libfreerdp. Session-host tokens are proof-of-possession tokens bound to a key FreeRDP generates (`req_cnf`) and requested per host. Obtain them with the same identity as the feed token, and do not assume a general OAuth crate supports this. Keep any patch isolated, reviewed, pinned, and suitable for upstream submission. A private IPC channel must use bounded messages, restrictive access, and authenticated peer/session association.

PTY scraping of human-readable `Browse to:` prompts is not the production authentication architecture; if used during research, keep it out of the release path. The only web view allowed to reuse sign-in cookies is the restricted, per-account sign-in helper from §6. Validate how the chosen backend supports external authentication before committing to it.

- Detect backend version and capabilities before launch and explain missing dependencies.
- Launch without a shell using typed arguments and an explicit environment allowlist. Close unrelated file descriptors. Pass sensitive or feed-derived values through FreeRDP's `/args-from:fd:<n>` rather than the visible command line.
- Derive scaling from the target monitor's scale factor; never hardcode values like `/scale-desktop:200`.
- Prefer memory or protected IPC for sensitive connection data. Where a file is unavoidable, use a unique private runtime directory (`0700`) and exclusive files (`0600`), minimize contents and lifetime, and clean up on every exit path and recoverably after crashes.
- Use validated certificate-chain verification by default. If private/self-signed session hosts require trust-on-first-use, present identity and fingerprint for explicit approval and pin to the correct host identity. Certificate changes block connection until reviewed. No silent `/cert:ignore` fallback.
- Entra and NLA modes must be chosen from validated capabilities and host requirements; never silently downgrade security after an authentication failure. Passwords required for an explicitly supported mode remain in memory and travel only over protected IPC.
- Host pools without Entra single sign-on (no `enablerdsaadauth:i:1`) are supported, not an edge case. Deskfeed asks for the Windows password itself in a native COSMIC dialog that allows paste and shows the user name it will send (pre-filled from the account, editable, with the domain format validated in milestone 1). It passes the password to FreeRDP without the command line (for example `/args-from:fd:<n>`), so FreeRDP's own SDL dialog never appears. Remembering the password is opt-in, per account and resource, stored only in the Secret Service, and cleared on sign-out. A logon failure (for example RDSTLS `0x52E`) shows "wrong user name or password" and lets the user retry; it is never silent. The dialog explains that an administrator can enable Entra single sign-on on the host pool to remove the prompt.
- Represent lifecycle states explicitly: preparing, authenticating, awaiting user (certificate approval or sign-in interaction), connecting, connected, reconnecting, disconnecting, disconnected, cancelled, and failed. Every attempt reaches a terminal state or a bounded timeout; waiting for the user has its own, longer limit.
- Associate events with attempt/session IDs. Ignore stale events after cancellation, sign-out, resource refresh, or a newer attempt.
- Prevent accidental duplicate launches; allow simultaneous sessions to different resources. Associate every child with its owning account and resource.
- Cancellation stops pending network/authentication work and terminates the associated process group. Disconnect is graceful first, with bounded escalation and child reaping. Never kill unrelated FreeRDP processes.
- Retry transient connection loss with bounded backoff, visible status, and a stop action. Do not repeatedly retry policy denial, rejected certificates, or invalid credentials.
- Backend exit must clear busy states and show a useful sanitized failure reason. Connected status comes from backend session evidence, not merely successful process creation.
- Session stability: a session must survive several hours of active and idle use, audio device changes, monitor hotplug, network changes, and suspend/resume, or recover through reconnect. Keep sanitized backend logs for every abnormal exit, and add a regression test or manual check for each reproduced reference-app failure.

## 9. COSMIC, Wayland, and device integration

The application UI must run natively on Wayland. Native Wayland remote-session windows are the preferred target and require tests for keyboard focus, modifiers, clipboard, fullscreen, monitor hotplug, fractional scaling, suspend/resume, and reconnection.

If a required FreeRDP capability needs XWayland, make that a documented backend-specific compatibility mode. Do not force the COSMIC interface onto X11 or claim native Wayland session support without evidence. Focus requests must respect compositor activation policy rather than requiring global compositor configuration changes. FreeRDP's RemoteApp support lives in the X11 client (`xfreerdp`), so expect RemoteApp to need the XWayland mode unless testing shows otherwise.

While a session has keyboard focus, capture system shortcuts (Super, Alt+Tab) only through the Wayland keyboard-shortcuts-inhibit protocol, with a visible way to release them; confirm COSMIC's support in milestone 1. Prevent screen lock only while a fullscreen session is focused, and only when the user enables it.

Audio playback may default on. Microphone, camera, drive, and USB redirection default off. Clipboard redirection requires an explicit setting and clear direction controls where the backend supports them. Use portals and PipeWire where supported and prefer narrow device access. Camera support that needs broad sandbox device access must have a documented tradeoff and an explicit packaging decision before release.

## 10. Architecture and persistence

Keep UI state separate from protocol and process work. Suggested modules, adapted to the official template's structure:

- `app`: libcosmic application, messages, navigation, views, and user actions.
- `auth`: account identity, authorization attempts, renewal, and sign-out.
- `secrets`: Secret Service adapter and memory-only mode.
- `service`: endpoint policies, HTTP transport, discovery, typed feed models, icons, and connection-file validation.
- `sessions`: backend abstraction, supervised child processes, IPC, and lifecycle state machine.
- `config`: versioned global and per-resource settings using COSMIC conventions.
- `cache`: account-isolated, bounded non-secret metadata storage.
- `diagnostics`: structured redaction, bounded logs, and explicit support export.

Run as a single instance: launching the app again focuses the existing window (the template's single-instance support).

No networking, token refresh, keyring blocking calls, image decoding, or child waits on the UI thread. Use bounded asynchronous work, cancellation, and typed results instead of process exits or panics for expected failures. Partial feed failure preserves successful results and identifies the failed workspace.

Persist configuration and caches according to XDG/COSMIC conventions; use the runtime directory for ephemeral session artifacts. Atomic writes and a schema migration policy are required. Do not modify the user's general FreeRDP configuration to implement per-session preferences. Configuration exports contain no credentials.

## 11. Packaging, licensing, and GitHub readiness

- Native packages only, built with the official template's `just` workflow; never a Flatpak (owner decision, [ADR 0010](docs/adr/0010-native-only-and-dependency-check.md)).
- Use the system's installed FreeRDP at a documented minimum version. Do not download executables at application runtime.
- Document every runtime dependency and its minimum version in the README: FreeRDP, WebKitGTK, a Secret Service, COSMIC. The app checks them at startup and on request ("Check again") and explains what to install. Also document Rust MSRV and the libcosmic revision. Build `sso-mib` support as an optional feature.
- Include desktop entry, original icons, AppStream metadata, screenshots without real tenant data, README, MIT `LICENSE`, third-party notices, contribution instructions, changelog, and `SECURITY.md` before public release.
- Use a single release version across Cargo, UI, AppStream, tags, and artifacts. Record source and backend revisions and produce dependency/license inventories.
- GitHub Actions must run formatting, linting, tests, build, metadata validation, and relevant dependency/security checks. Pin third-party actions to reviewed commit SHAs, minimize job permissions, and isolate signing credentials from untrusted pull requests.
- Publish versioned checksummed artifacts and release notes with compatibility limitations. Do not claim Flathub availability until accepted and published there. Public hosting and release publication are later actions, outside this specification task.

## 12. Diagnostics and privacy

Use bounded, rotating structured logs with secrets redacted before formatting or storage. Diagnostic exports are user-initiated, previewable, and exclude tokens, codes, cookies, complete connection files, and raw feed responses. Treat account names, hostnames, resource titles, and tenant identifiers as sensitive metadata and redact them by default in exports. Telemetry and automatic upload are absent by default. This includes AVD client diagnostics sent to a connection file's `diagnosticserviceurl`; if added later, it is opt-in, documented, and limited to what the official clients send.

Show actionable errors for offline/DNS failure, TLS rejection, expired authentication, wrong Windows user name or password, Conditional Access denial, missing assignment, unsupported host authentication, malformed feed, missing backend, backend crash, and denied device access. A support reference or correlation ID may be shown if it is safe to share. Technical details belong in an optional details area.

## 13. Verification and release acceptance

Automated tests must exercise behavior and failure boundaries, not merely mirror implementation. Use mock endpoints and synthetic credentials; CI must not require real Microsoft accounts.

Required automated coverage:

1. Missing/mismatched/replayed OAuth state, callback spoofing, expired attempts, sign-out races, and cancellation.
2. Keyring failure produces memory-only operation without credential files; concurrent renewal is serialized; rotated credentials do not cross account boundaries.
3. Authenticated redirects to untrusted and different origins, downgrade attempts, malformed URLs, and icon redirects cannot leak Authorization headers. Use mock transport or local servers to verify actual requests, not only URL helper return values.
4. Malformed/oversized XML, images, connection files, partial tenant failure, missing assignments, malicious property injection, and duplicate resource titles.
5. Backend missing/crash/hang, timeout, cancellation during authentication, stale events, concurrent sessions, safe argument construction, cleanup, and child reaping.
6. Certificate failure and changed pins never lead to automatic verification bypass.
7. Diagnostic redaction and sign-out cleanup, including unavailable keyring deletion.
8. The sign-in web view refuses disallowed navigations, pop-ups, and downloads, and returns only code, state, and error fields. The FreeRDP sign-in helper handles the protocol correctly, including cancellation, timeout, and a closed app.
9. Password-login hosts: the password never appears in arguments, environment, or logs; a logon failure produces a visible retry; the remembered password is removed on sign-out.
10. Test the feed XML, connection-file, and callback parsers with random and mangled inputs, as part of `cargo test` on the stable compiler (no nightly-only tools; changed 2026-10-19, ADR 0028).

Manual release matrix on COSMIC Wayland:

- First sign-in in both identity modes, MFA, federated identity provider, cancellation, silent refresh, expired session, and policy denial.
- Multi-hour session soak on COSMIC with the default backend, plus each reproduced reference-app session failure.
- Corporate HTTP proxy.
- Authorized AVD desktop and Windows 365 connection, on a host pool with Entra single sign-on and on one without it (password login, the issue #38 case); RemoteApp before declaring it supported.
- Native installation with missing or too-old dependencies (the startup check); locked/unavailable keyring and memory-only sessions.
- Windowed/fullscreen, selected monitors, mixed DPI, display hotplug, keyboard shortcuts, and supported clipboard directions.
- Audio device changes during a session; microphone/camera opt-in and unplug/replug when those features are enabled.
- Network interruption, suspend/resume, backend crash, app quit, and sign-out with active sessions.
- Offline cached workspaces, missing assignments, partial feed errors, and concurrent resources with identical titles.

Release acceptance: the documented core flow works with both identity modes (or the documented available one) and the chosen backend, including on a host pool without Entra single sign-on; no known credential leakage, plaintext credential persistence, silent TLS bypass, orphaned child process, or permanently busy UI remains. Unsupported configurations are documented explicitly. Each advertised optional capability has test evidence. Real tenant tests require an authorized account and environment; do not claim them from mock test success.

## 14. Implementation sequence

1. **Feasibility:** resolve service identity/access for both identity modes and backend token delivery; reproduce the reference app's session failures and choose the backend mode; record decisions and test evidence.
2. **Foundation:** generate the official template, apply MIT identity, establish modules, state models, localization, and CI.
3. **Account and discovery:** secure credentials, browser callbacks, refresh, typed feeds, resource UI, and offline metadata.
4. **Connection:** supervised backend, FreeRDP sign-in helper, native password dialog for hosts without single sign-on, certificate handling, protected IPC, cancellation, cleanup, and stable core sessions.
5. **Parity:** RemoteApp, advanced displays, redirection, tray/background behavior, and additional account support according to validated capabilities.
6. **Distribution:** native packaging, the dependency check, documentation, license inventory, and GitHub release preparation.

Resolve implementation decisions through evidence and record them; revise this specification when feasibility changes the design. Do not weaken security guarantees merely to reproduce a workaround from the reference app.

## 15. Sources and research baseline

Consulted 2026-10-17; implementations must recheck relevant upstream interfaces and versions.

- [Reference application](https://github.com/shakeelosmani/avd_feed_connect), inspected at commit `83e525e5083d8dab95973ee3ce2d28cb7aca0d7d`. Reference tests: 63 passed, 1 skipped; no live account/session test was performed during that review.
- [Official COSMIC application template](https://github.com/pop-os/cosmic-app-template) and [libcosmic documentation](https://pop-os.github.io/libcosmic/cosmic/).
- [Microsoft authorization code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow).
- [Microsoft desktop application configuration](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-registration).
- [Microsoft application registration](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app).
- [FreeRDP upstream](https://github.com/FreeRDP/FreeRDP), tag `3.32.2` inspected: default AVD client ID and `nativeclient` redirect in `libfreerdp/core/settings.c`, session-host proof-of-possession tokens in `libfreerdp/core/aad.c`, the `GetAccessToken` callback in `include/freerdp/freerdp.h`, and broker support in `client/common/sso_mib_tokens.c`.
- [AVD supported RDP properties](https://learn.microsoft.com/en-us/azure/virtual-desktop/rdp-properties), [AVD single sign-on with Entra](https://learn.microsoft.com/en-us/azure/virtual-desktop/configure-single-sign-on), and [AVD required URLs](https://learn.microsoft.com/en-us/azure/virtual-desktop/safe-url-list).
- [MS-RDPBCGR](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/) (RDS AAD authentication).

These sources establish the implementation foundation and general authentication model; they do not establish that an administrator-provided registration can use every AVD feed or gateway interface, or that Microsoft will keep accepting the default identity. Both remain part of milestone 1.
