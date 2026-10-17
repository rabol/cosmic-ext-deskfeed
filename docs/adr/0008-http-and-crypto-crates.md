# 0008. HTTP, TLS and crypto crates

- Status: Accepted
- Date: 2026-10-17

## Context

[ADR 0003](0003-minimal-dependencies.md): reuse what is already in the dependency tree. Checked in `Cargo.lock` (2026-10-17), already present through libcosmic: `zbus` 5 (D-Bus, for the Secret Service), `serde` and `serde_json`, `roxmltree` (XML without external entity loading), `sha2`, `base64`, `rand`/`getrandom`, `url`, `percent-encoding`, `image`, `tokio`, `futures`. Missing: an HTTP client with TLS.

Spec §7 needs: no automatic authenticated redirects, bounded sizes and timeouts, system trust store (enterprise CAs), system proxy settings, and observable requests in tests.

## Decision

- **HTTP:** `ureq` 3 (blocking, run on `tokio::task::spawn_blocking`) with `rustls`, the `ring` crypto provider, and `rustls-platform-verifier` for the system trust store. Redirects disabled (`max_redirects(0)`); the app follows redirects itself through its endpoint policy. Proxy from the environment. Chosen over `reqwest` (pulls in `hyper`, `tower` and more) and raw `hyper` (much more code to write).
- **Secret Service:** talk D-Bus directly with the existing `zbus`, instead of adding `oo7` or `secret-service`.
- **ID token signature checks:** use `ring` (already present via rustls) for RS256 verification of Microsoft's JWKS keys, instead of a JWT crate.
- **XML:** `roxmltree` with DTDs disallowed.
- **Web view helper only:** `gtk4` and `webkit6` ([ADR 0007](0007-sign-in-web-view.md)).

Measured on 2026-10-17: `ureq` 3.4.2 with features `rustls-no-provider`, `platform-verifier`, `gzip`, plus `rustls` 0.23 (`ring`, `std`, `tls12`, `logging`), added 21 packages to `Cargo.lock` (637 → 658). Several of them only build on other platforms (`schannel`, `security-framework`, `rustls-platform-verifier-android`). No bundled root certificates are used: `RootCerts::PlatformVerifier` reads the system store, so enterprise CAs installed system-wide work. A live test (`cargo test -- --ignored`) confirmed TLS to `rdweb.wvd.microsoft.com` with the system store.

## Consequences

- Few new crates in the main app: `ureq`, `rustls`, `ring`, `rustls-platform-verifier` and their small dependencies.
- We write a small amount of protocol code (Secret Service calls, JWT checks) ourselves, with tests, instead of pulling in larger crates.
