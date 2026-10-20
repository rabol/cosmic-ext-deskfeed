# 0023. Random-input tests on stable Rust instead of cargo-fuzz

- Status: Superseded by [0028](0028-random-input-tests-with-optional-cargo-fuzz.md) (2026-10-20): the random-input tests stay, now with proptest underneath, and cargo-fuzz is an optional extra.
- Date: 2026-10-19

## Context

Spec §13 item 10 asked to fuzz the feed XML, connection-file and callback parsers with `cargo-fuzz`. `cargo-fuzz` builds with coverage instrumentation (and usually AddressSanitizer) through `-Z` compiler options, which only the nightly Rust compiler accepts. The owner's view (2026-10-19): a requirement shouldn't depend on an unstable compiler. It would also add a tool used only by developers (ADR 0003: minimal dependencies).

## Decision

- The spec item now asks for random-input tests that run with `cargo test` on stable Rust.
- `test_util::random_inputs` (tests only, no new crate) mangles real samples with one to eight random edits each: changed, inserted, deleted, duplicated or repeated bytes, truncation, and pieces that tend to upset parsers (markup, entities, separators, byte-order marks, invalid UTF-8, long numbers, the markers the app looks for). One input in sixteen is pure noise. Each input must be handled without a panic and in under a second. Each parser's test also checks that parser's own rules.
- Covered parsers, with their rules:
  - Feed discovery and workspace feeds (`service/feed.rs`): only web addresses come out. The HTTP policy then allows only HTTPS to Microsoft's hosts.
  - Connection files (`sessions/rdpfile.rs`): an accepted file gives FreeRDP only reviewed properties, one per line, with no control characters, and an AVD gateway.
  - The sign-in callback (`auth/attempt.rs`): a code only with this attempt's state, and the attempt is used up either way.
  - Token endpoint errors (`auth/token.rs`).
  - ID tokens (`auth/id_token.rs`): nothing but the signed sample validates.
  - FreeRDP's output (`sessions/launcher.rs`): bounded log, certificate fields one printable line of at most 256 characters.
- The default is 1 500 rounds per parser with a fixed seed, so runs repeat exactly (about 0.25 s in all). `DESKFEED_RANDOM_ROUNDS` and `DESKFEED_RANDOM_SEED` run more or different inputs. A failure names the round, the seed and the input.

## Consequences

- Weaker than coverage-guided fuzzing: the inputs don't steer toward code not yet reached. The parsers are small and bounded, and the samples start from real formats, which covers most of what matters.
- First deep run: 200 000 rounds per parser, plus 100 000 with two other seeds, 1.4 million inputs in all. No panic and nothing slow. The only failure was a rule written too strictly: the feed parser also accepts `http` addresses, which the HTTP policy refuses later. That rule was corrected.
- Not covered at first: the sign-in helper's own parsing (`signin/`, a separate crate). Covered since 2026-10-19. The engine moved to a small test-only workspace crate (`testkit/`, standard library only, a dev-dependency of both) instead of being copied. The helper's tests cover the app's request line, every URL the web view navigates to (only a bounded redirect returns a code; errors pass on only short plain codes; anything allowed is HTTPS as the URL standard reads it), FreeRDP's protocol lines and command line, and the proxy file. A deep run of 100 000 rounds each found nothing.

## Addendum (2026-10-19): cargo-fuzz as an optional extra

Once it was clear that nightly is needed only on the machine that runs the fuzzer, and never for the app, the owner asked to try cargo-fuzz too. It is kept as an optional developer check next to the required random-input tests; the spec still requires only the stable tests.

- `fuzz/` is its own package and workspace (`libfuzzer-sys` plus the parsers' own crates), so the app's build, dependencies and `Cargo.lock` don't see it. It compiles the app's real `feed.rs`, `endpoint.rs` and `rdpfile.rs` (`#[path]`), so it tests what ships. Targets: `feed` (discovery and workspace feeds; only web addresses come out) and `rdpfile` (an accepted file gives FreeRDP printable `key:type:value` lines only). Starting samples are in `fuzz/corpus/`.
- `just fuzz <target> <seconds>` runs one target. Setup in CONTRIBUTING.md.
- First run, 5 minutes per target (nightly 2026-10-18, cargo-fuzz 0.13.2): `feed` 3.7 million inputs, 3 442 code edges found and still rising slowly; `rdpfile` 9.5 million inputs, 683 edges, flat after the first minute. No crash and no failed check.
- Other parsers (the sign-in callback, tokens, FreeRDP's output) depend on more of the app and stay with the random-input tests.


## Update (2026-10-20): proptest underneath

`testkit::random_inputs` keeps its rules and its signature, but proptest now generates the inputs and shrinks a failing one (ADR 0003). `testkit` is no longer standard library only. Since 2026-10-20 the tool is `common::testkit` in the shared workspace crate (`common/`), behind its `testkit` feature, which the app and the helper turn on for their tests only.
