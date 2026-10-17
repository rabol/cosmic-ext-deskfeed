# 0001. Record architecture decisions

- Status: Accepted
- Date: 2026-10-17

## Context

The specification (`specs.md` §4, §14) requires decisions to be based on evidence and recorded, especially in milestone 1, where service access, FreeRDP integration and backend choice are still open.

## Decision

Record each significant decision as an ADR in `docs/adr/`, named `NNNN-short-title.md`, with these sections:

- **Status:** Proposed, Accepted, Superseded by NNNN, or Rejected.
- **Date:** when the status last changed.
- **Context:** the problem and the evidence, including the dates of observations of undocumented Microsoft interfaces.
- **Decision:** what we do.
- **Consequences:** what follows, including open questions and what would make us revisit it.

A Proposed ADR may list tests that must pass before it is accepted. When evidence changes a decision, write a new ADR that supersedes the old one instead of rewriting history. If a decision changes the design, update `specs.md` as well.

Evidence from real tenants is redacted: no host names, tenant or workspace IDs, user names, or feed URLs.

## Consequences

Decisions and their evidence stay reviewable in the repository next to the code they affect.
