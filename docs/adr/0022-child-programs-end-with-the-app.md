# 0022. Child programs end with the app (PR_SET_PDEATHSIG from one spawner thread)

- Status: Accepted
- Date: 2026-10-19

## Context

Spec §6 and §8: no child session should become orphaned. Deskfeed ends its FreeRDP sessions when it quits normally (`stop_all`), but a crash, a panic that aborts, or a kill left `xfreerdp3` running. FreeRDP runs in its own process group (so the app can signal it and its sign-in helper together), so nothing took it down with the app. The same held for the app's own sign-in window (`cosmic-ext-deskfeed-signin`).

Linux's `prctl(PR_SET_PDEATHSIG, sig)` makes the kernel signal a child when its parent dies. The "parent" is the thread that forked it, not the process (prctl(2)). Deskfeed starts children from the async runtime's blocking threads, which exit after they have been idle for a while. With the signal set there, a session would end whenever its thread was retired.

## Decision

- `launcher::spawn_tied` starts every child program: FreeRDP and the sign-in helper. Each child sets `PR_SET_PDEATHSIG` to SIGTERM between fork and exec, and gives up if the app is already gone (`getppid` changed).
- The start itself happens on one thread ("spawner") that lives as long as the app; callers hand it the command over a channel and wait for the result. So the signal comes only when the app ends.
- SIGTERM, not SIGKILL: FreeRDP then ends the RDP connection properly, and its sign-in helper loses its pipes and ends too.

## Consequences

- Checked: a test process that started `sleep` this way and then aborted took `sleep` with it, and without the signal `sleep` outlived it (`children_end_when_the_app_crashes`). A real RemoteApp session's `xfreerdp3` ended within 2 s of the test process that started it being killed with SIGKILL.
- FreeRDP once caught SIGTERM and didn't exit (ADR 0020). After a crash nothing would escalate to SIGKILL; such a leftover window can still be closed by hand.
- Leftover runtime files are still cleaned at the next start (`clean_stale_runtime_dirs`).
