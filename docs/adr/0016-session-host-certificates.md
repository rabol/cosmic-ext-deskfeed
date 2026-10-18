# 0016. Session host certificates: the broker's pin, else the user's approval

- Status: Accepted (the approval path is checked locally; real AVD hosts take the broker path)
- Date: 2026-10-18

## Context

Spec §8: certificate approval with identity and fingerprint, pinned, changes blocking. Since the switch to `xfreerdp3`, a certificate FreeRDP can't verify is refused at its terminal prompt, which the app can't answer, so the connection just failed.

How FreeRDP 3.32.2 checks an AVD session host (source at tag `3.32.2`):

- The AVD broker's answer carries the session host's certificate (`redirectedServerCert`, `libfreerdp/core/gateway/arm.c`). FreeRDP stores it as the accepted certificate for that connection (`rdp_set_target_certificate`, `libfreerdp/core/redirection.c`), and the TLS check accepts the session host only if it presents exactly that one (`libfreerdp/crypto/tls.c`, `is_accepted`). The broker is reached over TLS with a public CA and the user's token, so this pin is stronger than anything the user could check. This is why the owner's connections never asked.
- Otherwise FreeRDP checks the system store and its own store (`~/.config/freerdp/server/`), then asks on the terminal. Under the app it logs `The fingerprint for the host key sent by the remote host is <sha256>` and `certificate not trusted, aborting`, and prints a "Certificate details" block (name, subject, issuer, validity, thumbprint) on stdout.
- `/cert:fingerprint:sha256:<fingerprint>` accepts exactly that certificate (`is_accepted_fingerprint`, case-insensitive, colons optional) and writes nothing to FreeRDP's store.

Measured with the local test server's self-signed certificate: the right fingerprint connects; a wrong one is refused; nothing is stored.

## Decision

- The broker's pin is used as is. No option ever passes `/cert:ignore` or lowers `authentication level` (ADR 0008 / `rdpfile.rs`).
- The launcher reads the refusal (`SessionEnd::CertificateNotTrusted`): the fingerprint from FreeRDP's log line, details from the description when it is the same certificate. stdout and stderr are read separately, so their order doesn't matter. All text is bounded and stripped of control characters; only a well-formed sha256 fingerprint counts.
- No approved certificate for the desktop: a dialog shows the host, name, issuer, validity and full fingerprint, explains that Azure Virtual Desktop didn't vouch for it, and offers "Trust and connect" or Cancel. Trusting saves the fingerprint for that desktop (config `certificate_pins`, by opaque resource key) and connects with `/cert:fingerprint:sha256:…`.
- An approved certificate that no longer matches is blocked: the dialog shows both fingerprints and has only Close. The approved certificate can be removed in the desktop's settings (Security → Remove); only then can a new one be approved.
- A certificate refusal never reconnects by itself.

## Consequences

- FreeRDP still reads the user's own FreeRDP store (`~/.config/freerdp/server/`); the app never writes to it (it never accepts at FreeRDP's prompt). A certificate the user accepted there in a manual `xfreerdp` run is trusted by FreeRDP for that host.
- Pins are kept when signing out, like desktop settings.
