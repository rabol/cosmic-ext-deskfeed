# 0014. The app reconnects lost sessions, not FreeRDP

- Status: Accepted (pending a real-session test: network drop, suspend/resume)
- Date: 2026-10-18

## Context

Spec §3 and §8 ask for reconnecting after a lost connection, with bounded backoff, a visible status and a stop action.

FreeRDP 3.32.2 has its own loop (`client/common/client.c`, `client_auto_reconnect_ex`):

- Off by default; a feed's `autoreconnection enabled:i:1` turns it on (Deskfeed passed that property through).
- 20 attempts at a fixed delay (`TcpConnectTimeout`, 15 s); no backoff.
- The X11 client opens its own "FreeRDP - Reconnecting" window during it (`client/X11/xf_reconnect.c`), in English and outside the app.
- Reconnecting reuses the gateway token it was given (`libfreerdp/core/gateway/arm.c`: a set `GatewayHttpExtAuthBearer` is kept), so after that token expires its attempts can't succeed.

Measured with `xfreerdp3` 3.32.2 and the local test server (`research/rdp-test-server`):

- With its loop off (`-auto-reconnect`), a lost connection ends FreeRDP at once with exit code 147 (`XF_EXIT_CONNECT_TRANSPORT_FAILED`) and the line `[client_auto_reconnect_ex]: Network disconnect!`. That line is logged before FreeRDP checks whether it may reconnect, so it is there either way. Without a known error the exit code can be 0 (`xf_client.c`), so the line, not the code, is the signal.
- With its loop on, the sample server's reconnected session dropped again after 1.5 s, so this setup gave no evidence that FreeRDP's own reconnect works.

## Decision

- FreeRDP always gets `-auto-reconnect` (its loop off, overriding the feed).
- The launcher reports `SessionEnd::ConnectionLost` when a connected session's log has the lost-connection line (`sessions/launcher.rs`, `LOST_MARKERS`).
- The app reconnects (`app/connect.rs`, `app/decide.rs` `reconnects`): each attempt is a full new connection (connection file, token renewal, FreeRDP), after a wait of 5 s doubling to at most 60 s. During the wait the desktop's row says "Connection lost. Reconnecting, attempt n of m…" with a Stop button. Attempts that fail for network-type reasons (DNS, TLS, FreeRDP failure, timeout, connection file not loading) count and continue; credential failures, a normal end or a cancel stop it. After the last attempt the row says reconnecting didn't work.
- Entra sign-in reconnects by itself. A password logon reconnects by itself only with a remembered password (ADR 0009 addendum); without one there's no password to use, and an automatic password dialog could surprise the user. A password Windows refuses (wrong, locked out, expired, disabled, authentication failed) is never sent again by itself, and a remembered one is deleted (ADR 0005; the non-"wrong password" refusals since 2026-10-20). *Updated 2026-10-20: this line said password logons never reconnect, which stopped being true with remembered passwords.*
- Settings at App → Account → Connection: "Reconnect after a lost connection" (on) and "Reconnect attempts" (5, 1–10). The waits are fixed.

## Consequences

- The desktop window closes and opens again on reconnect; Windows keeps the session, so the user returns to it. "Open on a new workspace" applies again.
- Entra sign-in may show the sign-in window during a reconnect if Microsoft asks for interaction.
- Not tested yet against a real host: network drop, suspend/resume, and how long a disconnected AVD session waits (the host pool's policy decides that).
