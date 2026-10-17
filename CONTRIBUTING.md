# Contributing

Thanks for helping! A few things matter more than usual in this project.

## Before you start

- Read [specs.md](./specs.md) and the [architecture decision records](./docs/adr/README.md). Decisions with a trade-off get an ADR.
- **Dependencies are kept to a minimum** ([ADR 0003](./docs/adr/0003-minimal-dependencies.md)). Prefer the standard library and crates already in libcosmic's tree. A new crate or system library needs a short ADR saying why.
- **Native packages only**; no Flatpak ([ADR 0010](./docs/adr/0010-native-only-and-dependency-check.md)). A new runtime dependency also needs a line in the README and a check in `src/deps.rs`.
- **Follow the COSMIC design language**: libcosmic's own widgets (settings sections, togglers, dropdowns, the context drawer, nav bar), patterned on COSMIC Settings.

## Never share tenant data

Don't put real credentials, tokens, cookies, tenant or workspace IDs, host names, user names or feed URLs in code, tests, issues, logs you paste, or commit messages. Use synthetic values (`anna@example.com`, `00000000-…`).

## Working on the code

```sh
just build-debug
just test
just check            # clippy, pedantic
cargo fmt --all
```

- Keep tests meaningful: test behaviour and failure cases, not the implementation. Decisions belong in small pure functions with tests (see `src/app/decide.rs`).
- Tests must not need a Microsoft account. Tests that touch the real keyring or desktop are `#[ignore]`d and run on request.
- Behaviour that depends on FreeRDP or Microsoft: cite the source (file and line, or documentation) in a comment rather than guessing.
- User-facing text goes into `i18n/en/cosmic_ext_avd.ftl`. Write plainly; say what happened and what to do.

## Commits and pull requests

- One logical change per commit, with a message that says why.
- Run the tests and clippy before opening a pull request.
- Describe how you tested it, and on which setup (distribution, FreeRDP version, single sign-on or password host pool).
