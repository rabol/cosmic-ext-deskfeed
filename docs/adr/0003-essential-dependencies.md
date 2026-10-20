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

## Dependencies chosen under the addendum

- **`ron` 0.12, test-only (2026-10-20):** to check that settings files written by earlier versions still load, the way cosmic-config reads them.
  - Options compared:
    - `ron` as a dev-dependency: the version libcosmic already uses, so nothing new enters `Cargo.lock`;
    - loading through cosmic-config with a temporary settings folder: needs process-wide environment changes that race between parallel tests;
    - no test.
  - Chosen: `ron`.
- **`mockito` 1.7, test-only (2026-10-20):** mock HTTP servers in the tests, replacing three hand-written fake servers.
  - Options compared:

    | | mockito 1.7.2 | wiremock 0.6.5 | httpmock 0.8.3 |
    |---|---|---|---|
    | usable from plain (non-async) tests | yes | no, async only | yes |
    | new crates for our tests | 16 | 18 | 25 |

    All three can set status, headers and bodies, answer redirects, and check headers and bodies.
  - Chosen: mockito.
  - One small raw server (`one_connection` in `http.rs`) stays, for what a mock server can't send: a compressed bomb, a server that stalls, an answer cut off halfway.
- **`tempfile` 3, test-only (2026-10-20):** temporary folders in the tests, replacing about 14 hand-made `temp_dir()` names.
  - Options compared:
    - `tempfile`: already in `Cargo.lock` through cosmic-config (its atomic writes), so nothing new; the folder is removed when the test ends, also when it fails;
    - `temp-dir` or `testdir`: smaller, but new crates for the same job;
    - our own guard type in `testkit`: about 20 lines that `tempfile` already does, including unique names.
  - Chosen: `tempfile`, in the app and the sign-in helper.
- **`jiff` 0.2 (2026-10-20):** local time for the labels in the window and the report, and the log's UTC timestamps.
  - Options compared:
    - `jiff`: already built into the app through libcosmic (its calendar), with the system time zone on, so nothing new; reads `TZ`, else `/etc/localtime`, like the C library;
    - `chrono`: new crates, and finding the local zone needs `iana-time-zone` on top;
    - our own code: an `unsafe` call to `localtime_r` and a hand-written calendar algorithm for the log.
  - Chosen: `jiff`. A test compares the labels with coreutils' `date` around daylight-saving changes; it agreed under eight zones, `TZ` unset, empty and bogus.
