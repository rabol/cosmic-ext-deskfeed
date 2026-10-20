# 0028. Random-input tests on stable Rust, with cargo-fuzz as an optional extra

- Status: Accepted (changes spec §13 item 10). Supersedes [0023](0023-random-input-tests-instead-of-cargo-fuzz.md).
- Date: 2026-10-20

## Context

Spec §13 item 10 asked to fuzz the feed XML, connection-file and callback parsers with `cargo-fuzz`. `cargo-fuzz` builds with coverage instrumentation through `-Z` compiler options, which only the nightly Rust compiler accepts. The owner's view (2026-10-19): a requirement shouldn't depend on an unstable compiler. So ADR 0023 made random-input tests on stable Rust the requirement. Later the same day, once it was clear that nightly is needed only on the machine that runs the fuzzer, the owner asked for cargo-fuzz too, as an optional extra.

## Decision

**Required: random-input tests in `cargo test`, on stable Rust.**
- The tool is `common::testkit::random_inputs` (also `test_util::random_inputs` in the app), in the shared crate behind its `testkit` feature, which the app and the helper turn on for their tests only. Its engine is proptest (ADR 0003), which shrinks a failing input to the fewest and smallest edits that still fail.
- It mangles real samples with one to eight random edits each: changed, inserted, deleted, duplicated or repeated bytes, truncation, and pieces that tend to upset parsers (markup, entities, separators, byte-order marks, invalid UTF-8, long numbers, the markers the app looks for). One input in sixteen is pure noise. Each input must be handled without a panic and in under a second; each parser's test also checks that parser's own rules.
- The default is 1 500 rounds per parser with a fixed seed, so runs repeat exactly. `DESKFEED_RANDOM_ROUNDS` and `DESKFEED_RANDOM_SEED` run more or different inputs. A failure names the seed and the (shrunk) input.
- Covered, with their rules:
  - feed discovery and workspace feeds (`service/feed.rs`): only web addresses come out;
  - connection files (`sessions/rdpfile.rs`): an accepted file gives FreeRDP only reviewed properties, one per line, with no control characters, and an AVD gateway;
  - the sign-in callback (`auth/attempt.rs`): a code only with this attempt's state, and the attempt is used up either way;
  - token endpoint errors (`auth/token.rs`), ID tokens (`auth/id_token.rs`: nothing but the signed sample validates), and log redaction (`log.rs`);
  - FreeRDP's output (`sessions/launcher`): bounded log, certificate fields one printable line of at most 256 characters;
  - the sign-in helper: the app's request line, every URL the web view navigates to (only a bounded redirect returns a code; errors pass on only short plain codes; anything allowed is HTTPS), FreeRDP's protocol lines and command line, and the proxy file.

**Optional: cargo-fuzz** for a deeper, coverage-guided run.
- `fuzz/` is its own package and workspace (`libfuzzer-sys` plus the parsers' own source files through `#[path]`), so the app's build, dependencies and `Cargo.lock` don't see it, and it tests what ships. Targets: `feed` and `rdpfile`, with starting samples in `fuzz/corpus/`.
- `just fuzz <target> <seconds>` runs one target; setup in CONTRIBUTING.md. Only that machine needs nightly; the app is always built with stable.

## Consequences

- The random-input tests are weaker than coverage-guided fuzzing: their inputs don't steer toward code not yet reached. The parsers are small and bounded, and the samples start from real formats.
- Deep runs found nothing beyond one rule written too strictly (the feed parser also accepts `http` addresses, which the HTTP policy refuses later; the rule was corrected): 1.4 million inputs over the app's parsers, 100 000 rounds each over the helper's. cargo-fuzz's first run, 5 minutes per target: 3.7 and 9.5 million inputs, no crash and no failed check.
- CI doesn't build `fuzz/`, so it can break unnoticed; it did once (fixed 2026-10-20). Build it before a deep run.
