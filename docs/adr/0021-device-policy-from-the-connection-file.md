# 0021. The administrator's device policy from the connection file wins

- Status: Accepted
- Date: 2026-10-19

## Context

Spec §5: respect server policy, and show the effective behaviour when policy overrides a preference. Deskfeed already did this for cameras (`camerastoredirect`, ADR 0013). The clipboard, microphone and sound followed only the user's settings: `rdpfile.rs` drops the feed's device properties, and the FreeRDP arguments come from the settings. So a host pool with `redirectclipboard:i:0` still got the clipboard when the user turned it on.

In Azure Virtual Desktop these properties are the host pool's RDP properties, delivered in the connection file; the client applies them. Microsoft's RDP property reference (checked 2026-10-19) gives the values and the defaults for a missing property:
- `redirectclipboard`: `1` available, `0` not; default `0`.
- `audiocapturemode`: `1` microphone redirected, `0` not; default `0`.
- `audiomode`: `0` play on this device, `1` play in the remote session, `2` no sound; default `0`.

Azure's default host pool properties set `redirectclipboard:i:1`, so most feeds allow the clipboard explicitly.

The same reference says `remoteapplicationexpandcmdline:i:0` (and `…expandworkingdir`) expands environment variables with the local device's values. FreeRDP 3.32.2 reads both but uses neither.

## Decision

- `rdpfile.rs` reads the three properties into `DevicePolicy`, with Microsoft's defaults for missing or malformed ones. The file passed to FreeRDP still turns these devices off; the command line decides.
- At launch, `Effective::limit_to` turns off what the policy doesn't allow: the clipboard (with clipboard files), the microphone, the sound. Only what the user had on counts.
- The row of a connecting or connected desktop says what was turned off and why ("Clipboard off: your organization doesn't allow it on this desktop"), next to the camera note.
- The log records the policy each time (`Device policy from the connection file: …`, values only).
- The `expand` properties are no longer passed on (ADR 0020 changed with this).

## Consequences

- A feed that doesn't set `redirectclipboard` now gets no clipboard, as with Microsoft's clients. If that surprises a user, the row and the log say why.
- Policies set only on the session host (Group Policy, Intune) are still enforced by Windows and can't be seen here.
