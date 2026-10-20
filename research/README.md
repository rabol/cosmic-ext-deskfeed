# Research scripts (retired)

These scripts were used on 2026-10-17 during the milestone 1 investigation. They are **not needed to build, test or use the app**, and you don't need to run them.

Since [ADR 0006](../docs/adr/0006-build-app-alongside-compatibility-work.md), open questions are answered with the app itself. The terminal sign-in step in `password-login-test.sh` no longer works because Microsoft blocks copying the `nativeclient` URL (see `docs/compatibility.md`).

Automated tests run with `cargo test` and need no account.

## Local RDP test server (in use)

[`rdp-test-server/`](rdp-test-server/README.md) is a local RDP server for checking FreeRDP's window on COSMIC (tiling, resizing, workspaces) without an account. It was used for the client comparison in [ADR 0004](../docs/adr/0004-freerdp-version-and-sign-in-helper.md).
