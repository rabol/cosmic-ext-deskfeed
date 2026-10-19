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

## Addendum (2026-10-19): proxy credentials stay out of environments (review F03)

A proxy address can carry a user name and password (`http://user:password@proxy:3128`). Until now the app copied its proxy variables into FreeRDP's and the sign-in helper's environments.

- The app still takes its proxy from the environment, as before. Child programs no longer get any proxy variable with an address.
- FreeRDP gets the app's proxy as `/proxy:` through the private argument pipe, and only `no_proxy` as a variable. With `/proxy:` set, FreeRDP 3.32.2 still reads `no_proxy` (`libfreerdp/core/proxy.c`, `proxy_prepare`).
- The sign-in helper, whether the app or FreeRDP starts it, reads the proxy from `proxy.json` in a private folder (0600). `COSMIC_EXT_DESKFEED_SIGNIN_PROXY_FILE` holds only that file's path. The helper gives the proxy to WebKit (`NetworkSession::set_proxy_settings`, custom mode). If the variable is set but the file can't be used, the helper stops instead of connecting without the proxy.
- Checked with an authenticating local proxy: the sign-in window sent all of Microsoft's sign-in traffic through it with the right credentials. The launcher test checks that FreeRDP's environment holds no proxy password, that the pipe has `/proxy:`, and that the file is 0600.
- Found on the way: FreeRDP 3.32.2 can't authenticate to an HTTP proxy at all. It drops the last character of `user:password` before encoding it (`http_proxy_connect`: `size - 1` on a length that already excludes the NUL). The test proxy received `anna:pr0xy-p` for `anna:pr0xy-pw`. FreeRDP's master branch has the same code. Deskfeed doesn't work around it (that would break once FreeRDP is fixed). When the proxy answers 407, the session ends as `ProxyAuthRefused`, and the row says it's a known FreeRDP bug. To be reported upstream by the owner.
