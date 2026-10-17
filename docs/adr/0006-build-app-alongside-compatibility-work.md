# 0006. Build the app alongside the compatibility work

- Status: Accepted
- Date: 2026-10-17

## Context

Spec §4 asks to prove sign-in → discovery → connection with research tools before substantial UI work. In practice this meant the owner running a series of manual scripts against the reference app, which was slow and frustrating, and several steps were blocked by Microsoft (the `nativeclient` page now prevents copying the code; see `docs/milestone-1.md`).

What we learned on 2026-10-17 is enough to design the core:

- FreeRDP 3.32.2 already implements the AVD gateway, the sign-in helper protocol and RDSTLS password logon ([ADR 0004](0004-freerdp-version-and-sign-in-helper.md)).
- Password logon on host pools without single sign-on depends on the logon domain, and deployments differ: FreeRDP defaults to `AzureAD`, some setups need an empty domain, and some may connect with Entra authentication to the session host instead ([ADR 0005](0005-password-login-host-pools.md), FreeRDP issues #11887 and #10298). The owner's host pool rejected both `AzureAD` (password pasted) and empty (password typed).

## Decision

The owner decided (2026-10-17) to stop manual troubleshooting and build the app. Remaining compatibility questions are answered **inside the app**, as normal use with a paste-capable password dialog, instead of with research scripts:

- Each resource has a logon setting: automatic (FreeRDP default), empty domain, custom domain, or Entra authentication to the session host. The app suggests the next option after a logon failure and remembers what worked; it never retries on its own.
- Research scripts stay in `research/` for reference but are not part of the workflow.
- Spec §4's completion criteria (both identity modes, multi-hour session, backend comparison, compatibility matrix) still apply before release; they are now checked with the app itself.

Build order: sign-in (web view helper, [ADR 0007](0007-sign-in-web-view.md)) → feed discovery and resource list → FreeRDP launch with the sign-in helper and password dialog → session lifecycle and settings.

## Consequences

- Faster progress; the owner tests the real app instead of scripts.
- Spec §4 and §14 describe a stricter order; update `specs.md` to match this decision.
- Some code may need changes when real-account results arrive. That is accepted.
