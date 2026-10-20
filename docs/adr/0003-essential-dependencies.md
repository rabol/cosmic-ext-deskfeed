# 0003. Keep dependencies to essentials

- Status: Accepted
- Date: 2026-10-17

## Context

The project owner wants as few dependencies as possible: Rust crates, system libraries, and tools. Every dependency adds build time, review work, supply-chain risk, and Flatpak size. The spec (§2) asks for maintained libraries for HTTP, OAuth, XML, Secret Service and async work, but leaves the exact choices to milestone 1.

## Decision

- Prefer the standard library, then crates already in the libcosmic dependency tree (for example `tokio`, `zbus`, `serde`), before adding anything new.
- Each new direct crate or system library needs a short justification in an ADR: why it is needed, which alternatives were rejected, and what it adds to `Cargo.lock`.
- Use small, focused crates instead of large frameworks when both are maintained, and enable only the features we need.
- Prefer designs that avoid a dependency when evidence allows. Example: if sign-in can use the system browser with a loopback redirect, no web view engine is needed (see [ADR 0004](0004-freerdp-version-and-sign-in-helper.md)).
- Research scripts under `research/` use only tools already on a typical developer system (bash, curl, openssl, Python standard library) and are not part of the app.
- Security does not lose to dependency count: do not hand-write cryptography, TLS, or JWT signature validation to avoid a crate.

## Consequences

- Removed unused libcosmic features `markdown` and `qr_code` ([ADR 0002](0002-pin-libcosmic-revision.md)).
- The sign-in web view (spec §6) is the largest possible new dependency. Milestone 1 tests whether it can be avoided before any engine is chosen.

## Addendum (2026-10-20): compare options; minimal isn't hand-written

The owner restated the rule: the use of an external dependency is considered deliberately, and not the first possibility taken. For each need, 2–3 options are investigated before choosing. Writing it ourselves, and crates already in the tree, count as options; "few dependencies" never means hand-coding what a mature crate does well.

The trigger was the tests' four hand-written fake HTTP servers. A mature test-only crate (`mockito`) does that job better.

The ADR for each new dependency records the comparison briefly:
- the options looked at;
- maturity and maintenance;
- what each pulls into `Cargo.lock`;
- fit, including the edge cases we need;
- whether it ships or is test-only.

Test-only (dev) dependencies don't ship, so they weigh less. The security point above stands: no hand-written cryptography, TLS or JWT validation.
