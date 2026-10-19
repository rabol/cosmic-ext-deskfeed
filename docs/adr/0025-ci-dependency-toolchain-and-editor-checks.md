# 0025. CI: dependency checks, a pinned toolchain, and an editor check

- Status: Accepted
- Date: 2026-10-19

## Context

The owner asked for the usual Rust best practices that the project lacked:
- a dependency and license check (review F21);
- a pinned toolchain, since CI used whatever Arch's `rust` package was that day;
- a check that matches what editors report.

Editors had shown 124 errors that the compiler didn't. rust-analyzer analyzes code differently from rustc, and CI only ran rustc and clippy.

## Decision

- **`cargo deny check`** in CI (`deny.toml`, Arch's `cargo-deny` 0.20.2):
  - **Advisories:** RustSec. Six "unmaintained" notices inside libcosmic's tree are accepted, each with its reason: the zbus 5 parts (`zvariant`, `zvariant_derive`, `zbus_names`) and the `paste`, `rustybuzz` and `ttf-parser` parts of the renderer. None has a fix Deskfeed can make alone. Any other advisory fails the check.
  - **Licenses:** an allow list of permissive licenses plus MPL-2.0 (libcosmic). `xdgen`'s GPL-3.0 is an exception for that crate only: it is build-only and not compiled into the programs. COSMIC crates without a license field are clarified from their repository's license file. cargo-deny 0.20.2 applies a clarification even when the file's hash differs (tried), so check those files when updating libcosmic.
  - **Sources:** crates.io and libcosmic's pinned Git repositories only.
  - **Bans:** no wildcard versions. Duplicate versions are allowed, because libcosmic brings several.
- **`rust-toolchain.toml`** pins Rust 1.99.1 with rustfmt, clippy and rust-src. CI installs `rustup` and `rustup toolchain install`. Locally, rustup users get the same release automatically. Without rustup, the file is ignored.
- **rust-analyzer in CI:** `rust-analyzer diagnostics .` from a pinned release (2026-10-19, the one Zed downloaded that day), verified by SHA-256. It isn't the toolchain's own rust-analyzer component: 1.99.1's component reported a false error (`NonNull<()>: Send` on a spawned future in `tray.rs`) that the current release doesn't, and editors run their own current release anyway.
- The app's manifest says `publish = false`: it is an application, not a crates.io library.

## Consequences

- Run locally, all of it passed under the pinned toolchain: formatting, strict clippy, tests, cargo-deny, and rust-analyzer with 0 errors. The rust-analyzer step exits 1 when it finds an error (seen with the older component).
- Updating Rust: change `channel`. Updating rust-analyzer: change both `RA_URL` and `RA_SHA256` in the workflow. Updating libcosmic: rerun `cargo deny check`, look at the clarified license files, and review the accepted notices.
- The workflow couldn't be run on GitHub from here (the repository isn't pushed). Each of its commands was run locally with the same tool versions.
