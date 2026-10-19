# 0023. Random-input tests on stable Rust instead of cargo-fuzz

- Status: Accepted (changes spec §13 item 10)
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
- Not covered: the sign-in helper's own parsing (`signin/`, a separate crate), which reads FreeRDP's requests and the web view's navigations. That would need a copy of the engine there.
