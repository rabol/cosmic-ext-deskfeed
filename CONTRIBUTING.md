# Contributing

Thanks for helping! A few things matter more than usual in this project.

## Before you start

- Read [specs.md](./specs.md) and the [architecture decision records](./docs/adr/README.md). Decisions with a trade-off get an ADR.
- **Dependencies are kept to a minimum** ([ADR 0003](./docs/adr/0003-essential-dependencies.md)). Prefer the standard library and crates already in libcosmic's tree. A new crate or system library needs a short ADR saying why.
- **Native packages only**; no Flatpak ([ADR 0010](./docs/adr/0010-native-only-and-dependency-check.md)). A new runtime dependency also needs a line in the README and a check in `src/deps.rs`.
- **Follow the COSMIC design language**: libcosmic's own widgets (settings sections, togglers, dropdowns, the context drawer, nav bar), patterned on COSMIC Settings.

## Never share tenant data

Don't put real credentials, tokens, cookies, tenant or workspace IDs, host names, user names or feed URLs in code, tests, issues, logs you paste, or commit messages. Use synthetic values (`anna@example.com`, `00000000-…`).

## Working on the code

Recommended tools (ADR 0025): `rustup`, which uses the Rust release pinned in `rust-toolchain.toml` with rust-src, and `cargo-deny`. On Arch Linux: `sudo pacman -S rustup cargo-deny` (rustup replaces the `rust` package), then `rustup toolchain install` in this folder. With Arch's `rust` package instead, also install `rust-src`, or editors show false errors (`{unknown}` types, `None`, `!` on `bool`) while `cargo build` is fine.

The checks CI runs, besides the ones below: `cargo deny --locked check` (dependencies, licenses, sources; `deny.toml`) and `rust-analyzer diagnostics .` (what editors report; CI uses a pinned release).

```sh
just build-debug
just test
just check            # the strict lint check, as CI runs it
just ci               # everything CI checks, in its order
cargo fmt --all
```

- Keep tests meaningful: test behaviour and failure cases, not the implementation. Decisions belong in small pure functions with tests (see `src/app/decide.rs`).
- A parser of untrusted input (network data, files, FreeRDP's output) gets a random-input test with `common::testkit::random_inputs` (in the app also as `test_util::random_inputs`) and real samples (ADR 0023). For a deeper run: `DESKFEED_RANDOM_ROUNDS=200000 cargo test --release random`.
- Optional, deeper still: coverage-guided fuzzing with cargo-fuzz (`fuzz/`, ADR 0023 addendum). It needs rustup's nightly compiler, used only for this; the app is always built with stable. Setup: `rustup toolchain install nightly` and `cargo install cargo-fuzz`, then `just fuzz feed 300` or `just fuzz rdpfile 300`. A crash is saved under `fuzz/artifacts/`; turn it into a normal test before fixing it.
- Tests must not need a Microsoft account. Tests that touch the real keyring or desktop are `#[ignore]`d and run on request.
- Behaviour that depends on FreeRDP or Microsoft: cite the source (file and line, or documentation) in a comment rather than guessing.
- User-facing text goes into `i18n/en/cosmic_ext_deskfeed.ftl`. Write plainly; say what happened and what to do.

## Commits and pull requests

- One logical change per commit, with a message that says why.
- Run the tests and clippy before opening a pull request.
- Describe how you tested it, and on which setup (distribution, FreeRDP version, single sign-on or password host pool).
