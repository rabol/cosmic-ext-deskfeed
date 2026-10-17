# 0002. Pin libcosmic to a reviewed revision

- Status: Accepted
- Date: 2026-10-17

## Context

The COSMIC app template depends on libcosmic as a Git dependency without a revision, so `cargo update` can move to any new upstream commit. The spec (§2, §11) requires Git dependencies pinned to reviewed revisions, a committed `Cargo.lock`, and a documented libcosmic revision.

## Decision

Pin `libcosmic` in `Cargo.toml` with `rev = "e1f2cd2e52226aa50d59bb2cd6d43ffe290106ed"` (libcosmic `master`, committed 2026-10-14, "fix(text_input): remove capture_escape property"). This is the revision the template resolved when the project was generated, so pinning it changes no code. `cargo check` passes with it on Rust 1.99.0.

Also remove the template's `markdown` and `qr_code` features, which the app does not use (4 fewer packages in `Cargo.lock`; see [ADR 0003](0003-minimal-dependencies.md)). Keep `about`, `single-instance`, `wgpu` and `xdg-portal`.

To update libcosmic: change `rev`, run `cargo update -p libcosmic`, build and run the app, check the libcosmic changes for anything that affects us, and record the new revision here or in a new ADR.

## Consequences

- Builds are reproducible from `Cargo.toml` and `Cargo.lock`.
- libcosmic fixes arrive only through deliberate updates.
- The minimum supported Rust version still has to be decided and documented (§11).
