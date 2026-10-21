# Independent repository code review

*Historical: a one-time review of commit `35b6557`. Its findings were fixed or decided since. Its finding IDs (F02, U6, …) are cited in ADRs and the changelog; what is still open is in [docs/roadmap.md](../roadmap.md).*

Date: 2026-10-19. Reviewed commit: `35b6557327e17d54b6f4889b3e0567c0ca9d1f37` (initial working tree clean).

## Overall assessment and release readiness

**Not ready for the specification's initial usable release.** This is a substantial, buildable implementation, not a collection of placeholders. The native account/resource UI, secure feed transport, PKCE/OIDC checks, keyring persistence, typed connection filtering, and supervised FreeRDP integration have meaningful implementations and tests. Both workspace builds pass, and 218 enabled tests pass. Nevertheless, credential cleanup has a sign-out race, certificate pinning does not enforce the advertised invariant, and connection cancellation/supervision have gaps. Administrator-registration sign-in is absent. Password-host interoperability and the required stability matrix remain unproven. CI's strict lint gate currently fails.

Severity totals: **1 High, 11 Medium, 2 Low; no Critical findings confirmed.** Severity reflects concrete impact, not whether an item was already mentioned in the roadmap. Findings below are confirmed code defects or directly observable omissions; live exploitability and service compatibility are qualified separately.

## Scope, method, and limitations

- Read `specs.md`, README, security/contribution guidance, architecture decisions 0001–0023, milestone evidence, roadmap, settings inventory, and manual acceptance list. Documents were treated as claims/reference material. Their requests to an implementation agent were not treated as review instructions or permission to change code. No applicable `AGENTS.md` was found in the repository or checked ancestors.
- Examined the entire production Rust implementation in `src/` and `signin/src/`, including UI routing/state, authentication, HTTP/feed/icon/RDP parsing, persistence, process launch/supervision, diagnostics, tray, XSETTINGS and Wayland integration. Inspected existing test bodies and their boundaries, the separate fuzz workspace, manifests/lockfiles, `build.rs`, `justfile`, native packaging, desktop/AppStream resources, CI and license-generation script. Research scripts/server material was inspected as supporting compatibility evidence, not assumed to be production functionality or independently certified as an RDP implementation.
- Traced sign-in → callback → token exchange → validated identity → save/renew → discovery → connection-file download → password/Entra choice → private argument pipe → FreeRDP events → reconnect/disconnect/sign-out/quit. Separately traced cache and password writes against account removal.
- Checked FreeRDP 3.32.2 certificate API behavior against upstream source, rather than trusting repository comments. Inspected locally available FreeRDP helper/OAuth/ARM source and cached Rust dependency APIs where relevant. The app accepts newer FreeRDP versions as well; this review does not certify those versions.
- No application code was modified. Only this report was added in the repository; normal Cargo/build outputs were generated. A small independent harness under `/tmp/deskfeed-review-harness` compiled unchanged repository modules with dependency versions matching the repository lockfile for its dependencies. It used synthetic data and a cache rooted under `/tmp`.
- Did not use real credentials, access the real keyring, connect to Microsoft production authentication/feed/RDP services, install dependencies, alter system configuration, publish artifacts, or run ignored desktop/service tests. A read-only GitHub source lookup was used to verify FreeRDP's tagged API. Existing local mock-server tests were rerun outside the sandbox after the sandbox prohibited loopback binds.
- This is a source/build review, not a penetration test, supply-chain audit of every transitive crate, accessibility certification, or new proof of the manual release matrix. Repository statements that the author's tenant worked are useful context, not independently reproduced results.

## Confirmed findings, ordered by severity

### F01 — High: late credential writes can recreate secrets after sign-out

**Locations:** `src/app/connect.rs:879–897`, `src/app/connect.rs:73–85`; `src/accounts.rs:408–414`, `src/accounts.rs:519–524`; cleanup at `src/app/account_flow.rs:906–924`.

**Trigger/defect:** On `Connected`, a pending password is moved into an independent blocking `secrets::save_password` task. That task has no account epoch, sign-out counter, cancellation flag, or shared account mutation lock. Sign-out deletes passwords in a separate task. If the save is queued, paused, or waiting on keyring interaction while sign-out finishes deletion, it can subsequently create the password item. The late `PasswordSaved` message also marks the resource as having a saved password without checking that the account still exists.

There is a related gap in refresh-token protection: `restore_in` and `renew_in` capture `sign_out_count` only when their blocking function begins, rather than when the account operation is scheduled. If a queued restore or expiring-token renewal starts after sign-out deletion has completed, it captures the new counter and can save a rotated refresh token as if it were current. The task still holds the old `Stored`/`SignedIn` credential. A mutex serializes execution but cannot recognize that this queued task belongs to a removed account. UI epoch checks happen after these persistence side effects.

**Impact:** A reusable Windows password or rotated refresh token can remain in the keyring after an apparently successful sign-out; the latter can restore the account on restart. Configuration can also regain the saved-password flag. The password path can race an explicit Forget Password action.

**Correction:** Route all credential save/delete operations through a per-account serialized mutation service, capture the account lifetime generation before scheduling any restore/renew/save task, check that captured generation inside the lock immediately before network use and writing, and have sign-out wait for/retire earlier password writes before deleting. Include account epoch in completion messages and reject stale completions. Add a barrier-controlled test that delays save until after deletion and verifies that no secret or saved flag survives.

**Specification:** §§6, 8 and 13(7, 9): sign-out must remove remembered passwords and prevent late operations restoring credentials.

### F02 — Medium: `/cert:fingerprint` is additional trust, not a strict certificate pin

**Locations:** `src/sessions/launcher.rs:407–414`; `src/app/connect.rs:977–1010`; storage at `src/config.rs:22–24`.

**Trigger/defect:** Deskfeed passes its saved fingerprint to FreeRDP and detects changes only when FreeRDP refuses the certificate. In [FreeRDP 3.32.2 TLS verification](https://github.com/FreeRDP/FreeRDP/blob/3.32.2/libfreerdp/crypto/tls.c#L1727-L1841), a matching fingerprint succeeds early; a mismatch continues through other trust paths, including valid chain/hostname verification. A changed certificate trusted through another path therefore connects without the application's changed-pin dialog. The saved entry also contains no host identity.

**Impact:** The promise that certificate changes block is false. A resource pin is also insufficiently scoped for a resource whose backing host changes. This is an integration defect, not evidence that normal CA verification is disabled.

**Correction:** Enforce host-scoped pin comparison in a backend certificate-verification integration before allowing another trust path. Test changed certificates that are otherwise CA-valid, plus broker-approved changes and host-pool rotation. Define explicitly how authenticated broker trust interacts with user pins.

**Specification:** §8 and §13(6); ADR 0016's changed-certificate guarantee.

### F03 — Medium: authenticated proxy credentials are copied into child environments

**Locations:** `src/sessions/launcher.rs:68–97`, `src/sessions/launcher.rs:712–718`; sign-in helper environment at `src/auth/signin.rs:223–224`.

**Trigger/defect:** With `https_proxy=http://user:password@proxy.example:8080` (or another accepted proxy variable), the full proxy URI is copied into the FreeRDP environment. The base environment also passes proxy variables to sign-in helpers. Normalizing which variable wins does not remove its userinfo.

**Impact:** Proxy passwords are exposed as process environment data and propagated to descendants, contrary to the application's protected-IPC credential policy. No actual credentials were used to test this path.

**Correction:** Separate proxy destination and authentication, remove credential-bearing proxy variables from child environments, and deliver backend proxy credentials using the protected argument channel or a vetted equivalent. Define a secure WebKit proxy-authentication path as well. Test with synthetic userinfo and inspect both direct child and helper environments.

**Specification:** §§6–8: proxy credentials follow token rules; credentials must not be in environment variables.

### F04 — Medium: connection filtering validates the gateway but not other destination-bearing properties

**Locations:** `src/sessions/rdpfile.rs:37–80`, `src/sessions/rdpfile.rs:211–238`.

**Trigger/defect:** Any nonempty printable `full address` is accepted. `alternate full address`, `kdcproxyname`, and other destination-bearing allowed strings receive no semantic endpoint validation; property names also do not have enforced per-property types/ranges. An independent harness confirmed that a loopback session address and an arbitrary HTTPS KDC-proxy URL are accepted and serialized unchanged when accompanied by an allowed AVD gateway.

**Impact:** The parser does not establish the destination boundary the specification requires. It hands malformed or unintended destinations to FreeRDP, potentially causing local/private-network access or authentication traffic outside the intended routing model. The exact traffic depends on FreeRDP negotiation and broker response; this review does not claim a demonstrated bearer-token exfiltration or arbitrary-host connection through AVD.

**Correction:** Define a typed schema per property, validate hostname/port/URL roles separately, disallow unnecessary auxiliary endpoints, and constrain session targets through the validated broker routing/identity model. Do not simply apply the feed hostname allowlist to session hosts. Add tests of actual backend destination selection for hostile full/alternate/KDC values.

**Specification:** §7: typed reviewed properties and separate gateway/session-host destination validation.

### F05 — Medium: a delayed metadata-cache write can undo sign-out cleanup

**Locations:** `src/app/account_flow.rs:219–230`; `src/cache.rs:102–118`; deletion at `src/app/account_flow.rs:885–892`.

**Trigger/defect:** Successful discovery schedules `store.save` independently, without account generation checks or coordination with sign-out. Sign-out can remove the cache before a queued save begins; save then recreates the directory and renames a new cache into it. The independent harness verified save → remove → delayed save leaves a readable cache again.

**Impact:** Account workspace/resource names survive a supposedly complete local sign-out. The cache contains no connection URLs or tokens, so this is privacy/cleanup failure, not cached authorization bypass.

**Correction:** Serialize account cache writes/deletion and check a captured account generation under that serialization. Drain or cancel outstanding writes before reporting sign-out file cleanup complete. Test delayed save and concurrent refresh/cleanup schedules.

**Specification:** §§6–7 and §13(7): remove account caches and prevent late work restoring removed data.

### F06 — Medium: concurrent password prompts can leave one resource permanently waiting

**Locations:** `src/app/connect.rs:294–322`; `src/app/connect.rs:51–56`; `src/app/views.rs:239–246`.

**Trigger/defect:** Each password-ready connection sets its row to `AwaitingUser` and unconditionally replaces the single `self.dialog`. Start two resources before their connection-file downloads/keyring lookups complete; if both require passwords, the second completion discards the first dialog. Neither its row nor the general Disconnect handler can cancel an `AwaitingUser` resource without that dialog.

**Impact:** The first resource remains busy indefinitely and cannot be connected again during normal use. This is reachable through parallel resources, including tray actions; it does not require malformed input.

**Correction:** Queue dialogs by attempt/resource or explicitly cancel/reset the displaced attempt before replacement. Provide a cancellable row state independent of the visible dialog and a bounded awaiting-user lifetime. Add a two-resource completion-order test.

**Specification:** §§3, 8 and 10: simultaneous resources, explicit lifecycle, every attempt terminal or bounded, no permanently busy UI.

### F07 — Medium: a backend that does not consume the argument pipe can block launch and quit forever

**Locations:** `src/sessions/launcher.rs:609–621`, `src/sessions/launcher.rs:824–833`; deadline starts at `src/app/connect.rs:775–779`; quit waits at `src/app/connect.rs:797–801`.

**Trigger/defect:** After spawning FreeRDP, launch synchronously writes the whole argument payload with blocking `write_all` before establishing supervision or returning the Session handle. Password input is not length-bounded and the accepted gateway token alone may be 16 KiB. If the backend hangs before reading fd 3 and the payload exceeds pipe capacity, launch cannot return. Disconnect only marks the launching entry; the actual deadline is scheduled after launch returns.

**Impact:** The child remains untracked by normal stop handling, the resource stays launching, and quit waits indefinitely for the launching map to empty. The backend executable need not be malicious: a startup hang is sufficient. This is confirmed by the blocking control flow; no real backend was intentionally hung during this review.

**Correction:** Register child ownership and a launch deadline immediately after spawn. Write through a nonblocking/polled pipe with cancellation and a deadline, drain output concurrently, and terminate/reap the process group on failure. Bound the total argument payload. Test a fake backend that never reads its arguments with a payload larger than pipe capacity.

**Specification:** §8 and §13(5): backend hang/timeouts, cancellation, cleanup and bounded terminal states.

### F08 — Medium: preparation and background network work have no effective cancellation

**Locations:** `src/app/views.rs:239–242`; `src/app/connect.rs:128–144`, `src/app/connect.rs:207–251`; `src/app/account_flow.rs:673–690`; `src/service/discovery.rs:52–65`.

**Trigger/defect:** Preparing rows offer a disabled Connect button, and Disconnect handles only running/launching sessions or reconnect timers. Connection-file work has no cancellation token or per-attempt ID. Discovery and icon loops likewise keep making requests after account removal; epoch checks reject results only after the work completes. With 200 icons and a 60-second per-request budget, a batch can continue for hours.

**Impact:** Users cannot promptly cancel a pre-launch connection or stop background requests at sign-out. Tokens and account state remain retained by long-running work. UI result filtering protects against some stale updates but does not cancel credential use or network execution.

**Correction:** Give every network workflow an owned cancellation/attempt context. Check cancellation before each request, propagate it into the transport, and expose Cancel during preparation. Invalidate pending connection-file results on cancel as well as account removal. Test sign-out between requests and cancellation while renewing/downloading.

**Specification:** §§6, 8 and 10: stop outstanding work, cancellation of network/auth work, bounded asynchronous execution.

### F09 — Medium: the password dialog cannot edit the Windows user name

**Locations:** `src/app/mod.rs:462–508`; `src/app/connect.rs:537–539`; `src/app/model.rs:244–257`.

**Trigger/defect:** The dialog displays the account user name but has only password/domain/logon controls. Launch always takes `signed_in.identity.username`; neither dialog state nor messages carry an editable Windows username. A deployment needing a different Windows login name cannot be configured. A validated ID token without `preferred_username` also leaves no way to supply one.

**Impact:** Password-host configurations with differing Entra and Windows user names remain unusable, including a central class of compatibility problems the spec specifically requires investigating. This does not prove that the author's tenant accepts any particular alternative username.

**Correction:** Add a prefilled, editable Windows username to the native dialog and typed launch request, validate its accepted domain/UPN forms using authorized interoperability evidence, and associate remembered credentials with that login identity.

**Specification:** §8 explicitly requires a prefilled, editable username; §§4 and 13 require password-host/domain-format validation.

### F10 — Medium: administrator registration/system-browser identity mode is missing

**Locations:** `src/auth/attempt.rs:35–49`; `src/accounts.rs:307–312`; `src/app/account_flow.rs:377–387`; `signin/src/protocol.rs:141–151`.

**Trigger/defect:** All production paths construct the public Remote Desktop client configuration. There is no per-account registration editor, system-browser authorization flow, loopback listener, or backend propagation of an administrator client identity. A generic `ClientConfig` struct and research loopback script do not implement that workflow.

**Impact:** Organizations unable or unwilling to use Microsoft's public client identity have no alternative. The roadmap acknowledges deferral, but the spec still requires both modes or evidence that one is unavailable; the milestone matrix says this mode was not tested, rather than demonstrating unavailability.

**Correction:** Complete the registration feasibility investigation, then implement the full browser/callback/token/backend route with account-separated persistence. If unavailable, record reproducible evidence and obtain a product-spec change; do not label it implemented.

**Specification:** §§3–4, 6 and 13.

### F11 — Medium: the configured sign-in duration can exceed the OAuth attempt lifetime

**Locations:** `src/auth/signin.rs:178–185`; `src/auth/attempt.rs:17–18`, `src/auth/attempt.rs:134–135`, `src/auth/attempt.rs:185–188`; setting at `src/settings.rs:181`.

**Trigger/defect:** The UI permits up to 15 minutes and passes that duration to the helper, but `Attempt::new` always uses a 10-minute lifetime. With an 11–15-minute setting, a valid MFA/federation completion after minute ten is rejected as expired even though the visible sign-in window is still within its advertised time.

**Impact:** A successful sign-in is discarded and the user must repeat it. This is deterministic from the two independent clocks, not a service interoperability hypothesis.

**Correction:** Construct the attempt with the same validated duration (`Attempt::with_lifetime`) as the helper and use a consistent deadline/cancellation budget. Add fake-clock boundary tests at 10, 11 and 15 minutes without actually sleeping.

**Specification:** §§5–6: accurate cancellable waiting state and bounded, expiry-checked authorization attempts.

### F12 — Medium: parent-death SIGTERM does not guarantee child cleanup after an app crash

**Locations:** `src/sessions/launcher.rs:759–781`; normal escalation at `src/sessions/launcher.rs:571–590`.

**Trigger/defect:** `spawn_tied` requests SIGTERM on parent death. The normal SIGKILL escalation lives in Deskfeed itself and is unavailable once Deskfeed has crashed or been killed. A backend that catches SIGTERM but fails to exit survives. The signal targets the direct child, not an independently supervised process tree. ADR 0022 itself records an instance where FreeRDP caught SIGTERM without exiting; the independent defect is the absence of an enforcement mechanism after parent death.

**Impact:** A remote session can outlive the application, and helper descendants depend on cooperative pipe/parent shutdown. The existing crash test with `sleep` proves the cooperative case only.

**Correction:** Use a lifetime owner capable of enforcing cleanup after the UI dies (for example an independent bounded supervisor or an appropriate process-group/cgroup design). If choosing an uncatchable death signal for the direct backend, address descendants as well. Keep graceful shutdown for normal exits. Add crash tests with a SIGTERM-resistant child and descendants.

**Specification:** §§6, 8 and 13: no orphaned sessions/helpers, bounded cleanup after failures.

### F13 — Low: the checked-in CI lint gate fails on the reviewed checkout

**Locations:** `.github/workflows/ci.yml:46–47`; examples `src/accounts.rs:530–531`, `src/app/account_flow.rs:10`, `src/app/background.rs:23`.

**Trigger/defect:** Running the CI Clippy command with offline dependency resolution fails on Rust 1.99.1: 71 errors for the application binary and 79 for its test target (150 emitted diagnostics including duplicates). Categories include `nonminimal_bool`, `too_many_lines`, `used_underscore_binding`, and `unused_self`. These are not being reported individually as product defects.

**Impact:** The declared required gate cannot pass on the reviewed toolchain, preventing CI from reaching its later checks. Floating Arch/toolchain inputs can change this result over time.

**Correction:** Resolve diagnostics or adopt narrowly justified, explicit lint allowances, and pin/document the CI toolchain policy. Run the exact workflow gate before release. Do not hide actual correctness warnings in a blanket suppression.

**Specification:** §11: working formatting/lint/test/build CI.

### F14 — Low: release-package source integrity is left disabled

**Location:** `packaging/aur/cosmic-ext-deskfeed/PKGBUILD:36–40`.

**Trigger/defect:** The versioned release tarball uses `sha256sums=('SKIP')`; the comment says it remains to be filled at tagging. Unlike the VCS package's moving Git source, this is intended to identify an immutable release archive.

**Impact:** The release recipe does not verify the expected source bytes and is not distribution-ready. This is a confirmed packaging placeholder, not evidence that an altered archive has been served.

**Correction:** Set and verify the actual tagged archive checksum before publishing the release package, and produce checksums for distributed artifacts. Keep the development package's semantics distinct.

**Specification:** §11: reproducible/versioned/checksummed distribution. Public release itself remains outside this review's authorization.

## Specification coverage

“Implemented” means there is a traced implementation and relevant automated evidence, not that every environment has been validated. “Partial” includes concrete gaps or deviations. “Unverified” means implementation exists but required interoperability/acceptance evidence was not reproduced.

| Requirement | Status | Supporting evidence / remaining gap |
|---|---|---|
| §§1–2 native Rust/libcosmic app, identity, template conventions, license | Implemented | Cargo manifests, main lifecycle, i18n/resources, MIT license; GTK isolated in sign-in helper. |
| §2 reviewed/pinned dependencies and lockfile | Partial | libcosmic explicit rev and committed Cargo.lock; transitive Git resolution locked. CI Arch image/packages/toolchain float; no fresh clean/offline vendor build was demonstrated here. |
| §§3–4 multi-account, multi-tenant feed resources | Implemented | `app/account_flow.rs`, `accounts.rs`, discovery/feed models and account/tenant/publisher/resource keys in `app/model.rs`. Live cross-tenant matrix unverified. |
| §§4,6 public identity authorization | Implemented | Fresh state/nonce/verifier, S256 challenge, exact callback and replay/expiry checks, token exchange and RS256 signature/issuer/audience/time/nonce validation; F11 timing mismatch. |
| §§4,6 administrator registration / system browser | Missing | F10; only research script and generic config shape exist. |
| §6 same account across feed and backend authentication | Unverified | Profiles are per account, but FreeRDP helper returns code/state without validating the resulting identity against feed tenant/object ID. See U1 below. |
| §6 keyring-only refresh tokens, memory access tokens | Implemented | `secrets.rs`, per-client/authority/account attributes; no plaintext refresh-token fallback found. Memory-only operation is automatic with a notice/retry, rather than a separate affirmative choice as worded in the spec. |
| §6 serialized renewal and safe rotation | Partial | `ACCOUNT_LOCKS`, `LATEST`, `SIGN_OUTS` and fake-store tests are meaningful. F01 bypasses this discipline for password writes; cancellation and live keyring behavior remain gaps. |
| §§6,8 sign-out, active-session confirmation, cleanup retry | Partial | Confirmation and epoch filtering exist, with retry on deletion failure. F01/F05/F08/F12 prevent the full cleanup/lifetime guarantee. |
| §6 optional Microsoft Identity Broker | Missing | No capability/UI check or launch option; roadmap states distro backend lacks it. Not a Rust Cargo feature of this app today. |
| §6 restricted per-account sign-in webview | Partial | Separate process; HTTPS navigation, host display, popup/download/permission denial, TLS failure handling, cookie isolation. Tests predominantly exercise pure policy functions, not WebKit's real callbacks/subresources. U2. |
| §7 shared TLS/endpoint/redirect policy | Implemented | One ureq client, system trust, redirects disabled before credentials can reach a next hop, actual local-server tests. Endpoint role/path checks restrict bearer destinations; no broad generic Microsoft-domain token policy. |
| §7 proxy equivalence and credential policy | Partial | FreeRDP proxy selection normalized to ureq order, unsupported schemes rejected. F03 environment leakage; corporate-proxy interoperability unverified. |
| §7 discovery and XML bounds | Implemented | HTTP role byte/time limits; DTD denial, depth/node/tenant/resource caps; malformed/oversized/random-input tests. Sequential feed processing can be slow; F08. |
| §7 resource icons | Partial | PNG header size/dimension checks, token-scoped fetches and account-keyed in-memory storage. Decoder integration/UI-thread behavior and malformed full PNG behavior not independently runtime-tested; F08 batch lifetime. |
| §7 RDP typed filtering and destination policy | Partial | Devices/commands/diagnostics filtered; gateway restricted; byte/line/value limits. F04: other destinations and per-property type/range rules incomplete. |
| §7 stable resource identity and offline cache | Partial | Opaque account/tenant/workspace/resource keys; no URLs/tokens in cache; schema, permissions, atomic rename, age/size caps. F05 late recreation; offline connection disabled. |
| §7 startup/manual/resume/periodic refresh | Partial | Startup/manual refresh, hourly default, jitter and wall-vs-monotonic sleep detection. Sleeps below the 60-second detection slack are not explicitly refreshed; cancellation absent (F08). |
| §7 incompatible client-version failure | Missing | One owned X-MS-User-Agent exists, but generic HTTP errors do not distinguish version rejection. Roadmap correctly says service error contract is not yet known. |
| §8 shell-free launch/private argument pipe | Partial | `Command`, fd 3, env allowlist, per-session directory/file and filtering; F03/F07. Feed-derived sensitive data stays off visible argv. |
| §8 certificates and strict change blocking | Partial | No explicit `/cert:ignore` or `authentication level:0` copied from feeds; unknown-cert UI exists. F02 pin semantics incomplete; inherited backend trust/configuration requires further testing. |
| §8 password hosts/paste/remember/retry | Partial | Native secure input, protected `/p:` argument channel, keyring opt-in, failure classification. F01/F09; required authorized password-host success not established. |
| §8 states, stale-event isolation, concurrent sessions | Partial | Attempt/epoch filtering on session events, evidence-based Connected state and duplicate-launch guard. F06/F07/F08; preparation lacks per-attempt cancellation identity. |
| §8 disconnect/reconnect/supervision | Partial | Process groups, TERM→KILL/reap normal shutdown, app-owned bounded reconnect/backoff. F07/F12; no hostile/hung startup acceptance test. |
| §§8,13 multi-hour stability / resume / device changes | Unverified | Manual list still contains required soak, network, suspend and device checks; not replaced by fake-backend test success. |
| §9 native Wayland UI/XWayland compatibility | Partial | Native libcosmic UI; documented `xfreerdp3` XWayland backend chosen in ADR 0004. No false native session claim. Full mixed-DPI/hotplug matrix unverified. |
| §9 shortcut capture and screen inhibition | Partial | XWayland keyboard-grab compatibility path (ADR 0018), release instructions, focused/fullscreen ScreenSaver inhibitor. Literal shortcuts-inhibit requirement deviates; real shortcut test still pending. |
| §§5,9 display, audio, clipboard, microphone, camera | Partial | Layered global/account/resource settings, safe defaults, direction/file clipboard options and feed device-policy limits. Monitor scale sampled from global XSETTINGS at launch, not tracked per target monitor/change. Capability detection checks AAD, not each optional channel; camera/audio/multimon tests outstanding. |
| §5 search/favorites/grouping/status/cache UI | Implemented | `app/views.rs`, config favorites, typed status text, refresh times, workspace groups and offline labels. Accessibility/keyboard focus not certified. |
| §5 background/tray/autostart | Partial | Opt-in, initial tray availability check, window reopen and quit choices. Watcher restart re-registration exists; loss of tray host does not emit Unavailable to the app (U3). |
| §10 UI/protocol separation and asynchronous work | Partial | Most network/keyring/process waits use spawn_blocking. Profile adoption/removal and startup directory cleanup still execute synchronously from UI handlers; their latency was not benchmarked. |
| §§10–11 configuration/native packaging | Partial | COSMIC versioned config and layered overrides, just install, Arch release/VCS recipes. F14; no clean package install/uninstall, MSRV build or vendor round-trip run. |
| §11 metadata, licensing, screenshots, versioning | Partial | Desktop and AppStream validation pass; consistent 0.1.0 manifest/UI/AppStream version; icons/license/security/contribution/changelog present. AppStream screenshots absent; license inventory is generated but not a full legal audit. |
| §11 CI/dependency security checks | Partial | SHA-pinned checkout and read-only permissions; format/test/build/metadata jobs exist. F13; no advisory/license policy check in CI. |
| §12 safe diagnostics and privacy | Partial | Bounded rotating private log, redaction and user-previewed export; raw feed/RDP diagnostics not uploaded. Heuristic redaction has limitations (U4); proxy secrets and late cleanup F01/F03/F05. |
| §13 behavioral/failure tests/random inputs | Partial | 218 enabled tests pass, real local HTTP redirects/headers/bounds, signed JWT fixture/tampering, fake keyring/rotation, fake process lifecycle, stable random tests. Missing integration/race boundaries listed below. |

## Uncertainties requiring runtime or service interoperability evidence

These are not additional confirmed vulnerabilities or duplicates of the findings.

- **U1 — backend identity association.** Isolated cookie profiles and `/u:` do not prove that an interactive FreeRDP sign-in cannot select a different organizational identity. The helper does not inspect a validated result identity, and FreeRDP redeems its own code. Test A/B identities and account switching, including different tenants and silent-to-interactive fallback. Verify that gateway/session-host identity disagreement is rejected or explicitly handled; do not infer unauthorized access merely from this missing proof.
- **U2 — actual WebKit enforcement and concurrent profile use.** HTTPS/popup/download policy functions exist and compile against gtk4 0.11.5 / webkit6 0.6.1. There are no headless integration tests proving signals cover redirects, subframes, subresource schemes, TLS errors, downloads and popups. Multiple FreeRDP helpers can share the same persistent profile and runtime cookie jar; concurrent-cookie updates and sign-out need runtime testing. A synthetic harness confirmed the redirect parser accepts a 1 MiB code; the app-side reader has a separate 16 KiB bound. The helper should gain its own early URI/result bound and random-input tests; no secret leak from this was demonstrated.
- **U3 — tray disappearance.** The watcher owner-change task only attempts re-registration on a new owner. It does not notify AppModel when the current host disappears or re-registration fails. Initial absence is handled correctly. Test removal/restart of the tray while hidden and verify there remains a visible/reliable reopen route. The single-instance activation route exists, so “permanently inaccessible” was not established.
- **U4 — diagnostics redaction.** The redactor uses patterns (long alphanumeric tokens, dotted hosts, GUIDs, UPNs, IPv4), not a guarantee for arbitrary secrets/metadata. Short values, IPv6, single-label hosts and unknown user/resource names are not universally covered. Current call sites avoid intentionally formatting passwords/tokens and the backend log level is constrained. Test actual failure log formats with synthetic sensitive data before claiming comprehensive redaction. README and SECURITY still say Windows passwords are never stored, which is obsolete now that explicit keyring remembering exists; update those claims with the remediation.
- **U5 — interoperability/release evidence.** No independent Windows 365, AVD RemoteApp, federated IdP, Conditional Access/broker, corporate authenticated proxy, password-host success, mixed monitor/DPI, audio/camera device change, suspend/resume, or multi-hour soak was performed. The existing milestone matrix documents public-client Entra success on the author's setup, not the whole acceptance matrix. RemoteApp remains correctly described as preview. Camera is exposed without detecting whether the installed backend has that channel.
- **U6 — OIDC metadata/key scope.** Main ID-token verification does check signature and core claims rather than decoding alone. It downloads common JWKS for each validation and does not model JWK issuer metadata. Review Microsoft tenant/key-issuer binding and custom signing-key/federation cases before expanding supported identity modes; no signature bypass was established here.

## Validation commands and exact results

Host compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, Arch Linux. All Cargo checks used the existing dependency cache; no dependency installation or update was requested. Ordinary artifacts under target were permitted.

| Command/check | Result |
|---|---|
| `git status --short`; `git rev-parse HEAD` before work | Clean; commit recorded above. |
| `cargo fmt --all -- --check` | PASS, exit 0. |
| `cargo test --locked --offline --workspace` in sandbox | Exit 101: main binary 165 passed, 27 failed, 18 ignored. Failures were local `TcpListener::bind` permission errors (`Operation not permitted`); helper suite was not reached in this run. These were environmental, not accepted as product defects. |
| Same test command outside sandbox, approved for local mock listeners | PASS, exit 0. Main: 192 passed, 0 failed, 18 ignored. Helper: 26 passed, 0 failed. Total 218 passed, 18 ignored. Ignored live/service/keyring/desktop tests remained disabled; one ignored helper test is invoked internally as a subprocess by the enabled crash test. |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings -W clippy::pedantic` | FAIL, exit 101, F13. |
| Same Clippy check with `--message-format=json` | FAIL, exit 101; structured capture confirms 71 binary + 79 test-target errors. Temporary captures: `/tmp/deskfeed-review-clippy.jsonl` and `.stderr`. This second run was for exact classification, not a new code change. |
| `cargo build --locked --offline --workspace` | PASS, exit 0, debug workspace build. |
| `cargo build --locked --offline --release --workspace` | PASS, exit 0, optimized workspace build. |
| `desktop-file-validate target/xdgen/app.desktop` | PASS, no diagnostics. |
| `appstreamcli validate --no-net target/xdgen/app.metainfo.xml` | PASS, validation successful. |
| `cargo metadata --offline --locked --no-deps --format-version 1` | PASS, exit 0; output captured under `/tmp`. |
| `bash -n` separately for both PKGBUILDs and all three `research/*.sh` scripts | PASS, all five exit 0. Does not execute packaging/research commands. |
| Python `ast.parse` of `scripts/third-party-licenses.py` | PASS; generator was not executed because it rewrites an existing repository document. |
| `XDG_CACHE_HOME=/tmp/deskfeed-review-harness/cache cargo run --offline --manifest-path /tmp/deskfeed-review-harness/Cargo.toml` | PASS with one harmless unused-import warning in the temporary harness. Confirmed accepted arbitrary RDP/KDC targets, cache recreation after deletion, and unbounded helper redirect code. Exact unchanged repository source modules were included; relevant serde/serde_json/url/dirs versions were checked against Cargo.lock. No real user cache touched. |
| Read-only FreeRDP source inspection | Verified tag 3.32.2 TLS fingerprint control flow against upstream; checked helper/OAuth/ARM interfaces in locally available source. This is API verification, not a real backend connection test. |

**Checks not run:** real-service/desktop/keyring ignored tests; research scripts/local GUI RDP server; interactive application smoke test; installed package or system modification; cargo-fuzz/nightly campaigns; fresh clean-room vendoring/rebuild; alternate architecture/distribution/MSRV builds; online dependency vulnerability/license database checks. `cargo-audit` and `cargo-deny` were not present, and installing tools/dependencies was prohibited. The CI file itself also has no dependency/security audit step. No assertion is made that the dependency graph is free of known advisories. Existing build cache reuse means a successful offline build is not proof the release source archive alone is self-contained.

## Important test gaps

1. **Stateful application orchestration:** many `decide.rs` tests cover pure decisions, not message/task interleavings. Add an AppModel-level or extracted coordinator harness for two simultaneous password-ready resources, account reauthentication during refresh, resource removal during preparation, and cancel/disconnect before launch completion.
2. **Persistence mutation races:** queued restore/renew beginning only after sign-out completes, barrier-driven password save vs sign-out/Forget, cache save vs removal, retry cleanup vs re-add, and unavailable keyring deletion. Existing account refresh-token lock tests do not cover separate password/cache paths.
3. **Backend boundaries:** fake executable that does not read fd 3, writes enough output to backpressure startup, ignores TERM, forks a child holding output fds, exits during argument handover, and crashes while an auth helper is active. Verify all children reaped/terminated and directories removed, including app death.
4. **Certificate integration:** wrong pin with a CA-valid certificate, different host with the same certificate, broker-pinned rotation, inherited FreeRDP certificate store/config behavior, and changed pin after refresh. Pure `certificate_step` comparisons and self-signed failure tests do not verify FreeRDP rejects all mismatches.
5. **HTTP transport failure boundaries:** existing local-server redirect/header/body tests are useful. Add same-host different-origin redirect variants for every role, gzip expansion limits, slow headers/body/DNS, authenticated proxies, cancellation and interrupted response handling. No live TLS assertions were run.
6. **Authentication/webview boundaries:** random/mangled helper protocol and redirect inputs, response-size limits, GTK signal-level blocked navigation/popups/downloads/TLS, stale/cancelled callbacks after token exchange, and a deterministic shared deadline. Main callback random tests do not cover helper parsing.
7. **Real feature claims:** authorized success for password hosts and every advertised optional channel; real multi-account/tenant isolation; all-monitor/mixed-scale behavior; keyboard focus and clipboard direction; network/suspend recovery and several-hour soak. Automated parser/state tests cannot replace these.
8. **Distribution:** reproducible clean build at MSRV, staged install with nondefault prefix/CARGO_TARGET_DIR, file inventory/permissions and runtime dependency absence/too-old cases. Build scripts currently generate metadata under a fixed `target/xdgen`; document and test that convention when packaging with another Cargo target directory.
9. **Privacy:** synthetic log cases for short opaque codes, IPv6, single-label hostnames, certificate subjects, resource names and errors after sign-out. Overwriting an existing diagnostics file also needs a permissions test: `.mode(0o600)` only controls creation, not the permissions of a pre-existing destination.

## Prioritized remediation plan

1. **Before broader use:** fix F01's credential lifetime/write-delete races and F02's certificate enforcement, then lock the intended behavior with deterministic tests. Coordinate all credential/cache mutations with account lifetime (F05), and update obsolete security claims about password remembering.
2. **Before declaring lifecycle acceptance:** add cancellation contexts through preparation/network work, queue password prompts, supervise the child before pipe handover, enforce startup deadlines, and handle app-crash cleanup (F06–F08/F12). Test hung and TERM-resistant backends, not only cooperative fake programs.
3. **Before expanding organizational compatibility:** remove proxy credentials from child environments (F03), validate every destination/property role (F04), support editable Windows usernames (F09), and complete or formally revise administrator-mode requirements (F10). Align sign-in timers (F11).
4. **Before release tagging:** restore the strict CI gate (F13), add a scoped dependency/security check, complete release checksums (F14), screenshots and capability/documentation inventory. Verify a staged native package and clean/MSRV build without installing anything into the reviewer's system.
5. **Release acceptance:** run and record the authorized manual matrix, including an actual working password-host flow and multi-hour COSMIC stability. Keep unvalidated optional capabilities labeled as such. Resolve uncertainties through evidence; passing the present 218 tests does not waive them.
