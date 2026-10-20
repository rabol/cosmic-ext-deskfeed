# 0009. Secret Service over zbus, no keyring crate

- Status: Accepted
- Date: 2026-10-17

## Context

Spec §6: keep refresh tokens only in the desktop Secret Service, separated by account, tenant, authority and client identity; never fall back to files; keyring calls off the UI thread. Until now the app forgot the sign-in on exit, so every start needed a full Microsoft sign-in.

Candidates: the `oo7`, `secret-service` and `keyring` crates, or the D-Bus API directly. `zbus` 5 is already in the tree through libcosmic, with the `blocking-api` feature enabled ([ADR 0008](0008-http-and-crypto-crates.md)).

## Decision

- Talk to `org.freedesktop.secrets` directly with `zbus`'s blocking API (`src/secrets.rs`, about 200 lines). Added as a direct dependency with no new packages in `Cargo.lock`. The crates above would add code we don't need (file backends, portal support, their own crypto).
- Use the specification's `plain` algorithm. The secret travels over the user's own session bus; any process that can watch that bus could also ask the Secret Service for the item. The encrypted algorithm would need hand-written Diffie-Hellman, which ADR 0003 rules out.
- One item per account. Attributes: application ID, `type=refresh-token`, client ID, authority, tenant ID, account object ID. The secret is JSON with the refresh token, user name and display name, so no personal name is in the searchable attributes. Content type `text/plain`: `oo7-daemon` 0.6 (the owner's desktop) rejects `application/json`; newer oo7 and gnome-keyring accept any type.
- Locked items and collections are unlocked through the service's prompt.
- App start: load, renew with the refresh token, check a renewed ID token (signature, audience, issuer, expiry, same tenant and account; there is no nonce on renewal). `invalid_grant` deletes the item; network errors keep it and offer "Try again"; other errors ask for an interactive sign-in, which replaces the item. A rotated refresh token is saved at once.
- Renewals and keyring writes are serialized. Sign-out increments a counter checked under the same lock, so a renewal that started before sign-out can't save afterwards.
- Keyring unavailable: the app works for the session and shows that the sign-in wasn't saved. No file fallback.

## Consequences

- No new crates. Tested against `oo7-daemon` 0.6 with an opt-in test (`cargo test -- --ignored keyring`) that uses a test client ID.
- No Flatpak ([ADR 0010](0010-native-only-and-dependency-check.md)), so no sandbox permission is needed.

## Addendum (2026-10-18): remembered Windows passwords

Spec §8. For desktops that log on with a Windows password, the password dialog has "Remember this password in the keyring", off by default.

- One item per account and desktop, type `windows-password` (sign-ins are `refresh-token`, so neither search sees the other), attributes app, tenant, account object ID and the desktop's opaque resource key; the secret is the password only, at most 1 024 bytes. No file fallback. The config only records which desktops have one (`saved_passwords`), never the password.
- Stored only after the connection with it worked, so a mistyped password is never kept.
- Next time it is read and used without the dialog; that also lets such desktops reconnect by themselves (ADR 0014).
- If Windows rejects it, it is deleted at once and the dialog asks; it is never tried again (ADR 0005).
- Deleted on sign-out (all of the account's) and with Security → Remembered password → Remove in the desktop's settings.
- Checked against the session's real keyring (oo7 0.6): store, replace, read, delete; the sign-in list never shows it (`secrets::tests::real_keyring_password`).

## Addendum (2026-10-20): the crates compared properly (ADR 0003 addendum)

The decision above dismissed the crates mainly to avoid new ones. Under the restated rule they were compared against what Deskfeed needs, from their sources (2026-10). Versions: `keyring` 4.2 (with `keyring-core` 1.0 and its zbus store), `oo7` 0.6, `secret-service` 5.2.

| Need | keyring 4 | oo7 0.6 | secret-service 5.2 | ours |
|---|---|---|---|---|
| Our attributes and label; search and delete by part of them | partly: its own attribute scheme; ours need a second, non-atomic call | yes, but no search across the service | yes | yes |
| Unlock prompt with our 2-minute limit, then Dismiss; cancel, no answer and no keyring told apart | **no**: no time limit; cancel and locked look the same | **no time limit**; Dismiss only behind its `unstable` feature | **no**: waits for ever (`util.rs` `exec_prompt_blocking`), the prompt can't be reached to dismiss it | yes |
| `text/plain` (oo7-daemon 0.6 refuses some types) | forces `application/octet-stream` | yes | yes | yes |
| New crates | 19–26 | 26 (async only) | 7 | 0 |
| Test support | a mock with no attributes and no lock or prompt states | none; session bus only | works with our private-bus fake | `fake.rs`, real code paths |
| Encrypted session (DH) | always | yes | yes | no (`plain`, accepted above) |

Decision: keep our own client. The deciding need is the unlock prompt. With any of the crates, an unlock prompt nobody answers would hold a worker thread for ever; with `keyring`, it would also block every other keyring call. keyring's `MockCredential` couldn't test what matters here: account and password items kept apart, cancel, and no answer. `fake.rs` tests those on the real D-Bus calls.

If an encrypted session is ever wanted, `secret-service` (with `crypto-rust`) is the candidate to revisit: it fits everything except the prompt time limit.

Not verified: any crate against a live daemon (none was run).
