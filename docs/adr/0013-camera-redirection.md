# 0013. Camera redirection through FreeRDP's camera channel

- Status: Accepted (pending a real-session test with a camera)
- Date: 2026-10-18

## Context

The owner wants camera redirection soon. Spec §9: camera defaults off; prefer portals, PipeWire and narrow device access; camera support that needs broad device access must have a documented tradeoff and packaging decision before release. Spec §Settings: respect server policy and show the effective behaviour. Until now Deskfeed dropped the feed's `camerastoredirect` and had no setting.

What FreeRDP 3.32.2 does (source at tag `3.32.2`, `channels/rdpecam/client`, `client/common/file.c`, `client/common/cmdline.c`):

- The camera channel (MS-RDPECAM) is built into the distro's `libfreerdp-client3`, with the V4L2 backend. Frames are encoded with ffmpeg, which FreeRDP already links. No new dependency.
- There is no `/camera` option. The channel is loaded with `/dvc:rdpecam`, or from an `.rdp` file's `camerastoredirect`.
- The channel ignores its arguments: device names and `encode:`/`quality:` are parsed into arguments but the V4L2 backend is loaded with none (`ecam_load_hal_plugin(..., nullptr)`). It offers **every** `/dev/video0`–`63` that is a capture device with a supported format.
- Cameras are listed once, when the session host opens the channel. There is no hot-plug: a camera connected later appears only after reconnecting.
- FreeRDP opens `/dev/video*` directly, not through the camera portal or PipeWire.

## Decision

- A **Camera** setting, per account with per-desktop overrides, off by default (`src/settings.rs`). On adds `/dvc:rdpecam`.
- The feed's `camerastoredirect` is the administrator's policy and wins over the setting (`src/sessions/rdpfile.rs`, `CameraPolicy`):
  - `*` alone: cameras are shared when the setting is on.
  - not set or empty: no camera.
  - a list of specific cameras, or `*` with exclusions (`-…`): no camera, because FreeRDP can only share all of them.
  The property itself is still not passed to FreeRDP.
- When the policy stops a camera the user turned on, the desktop row says so while the session runs ("Camera not shared: …"), and the log records the policy (no device or tenant data).
- The setting's description says the organization must allow cameras and that a camera plugged in during a session needs a reconnect.
- **Device access:** Deskfeed is a native package only ([ADR 0010](0010-native-only-and-dependency-check.md)), so there is no sandbox permission to grant. FreeRDP reads V4L2 devices with the user's own access (on systemd systems the active seat's user gets it through `uaccess`). The tradeoff: no camera portal prompt and no choice of camera; when the setting is on, every local camera is offered to the desktop, and Windows decides which one an app uses. This is accepted because the setting is off by default, per desktop, and also gated by the administrator.

## Evidence

Local, with `research/rdp-test-server` extended with the camera enumeration channel (`sf_rdpecam.c`) and the distro's `xfreerdp3` 3.32.2:

- With `/dvc:rdpecam`: the client logs `Loading Dynamic Virtual Channel rdpecam` and `Loaded v4l HAL for ecam`; the server logs `client protocol version 2` and answers. The test machine has no camera, so no device was listed.
- Without it: the server offers the channel; the client never answers.

Still needed: a real AVD session with a camera attached (device listed in Windows, picture in an app such as the Camera app or Teams), and unplug/replug during a session (spec §13).

## Consequences

- If FreeRDP gains device selection, the `Specific` policy and a camera picker can follow; until then Deskfeed shares all cameras or none.
- No new dependency: the channel is part of the distro FreeRDP build the app already requires. A FreeRDP build without the channel, or without V4L2, can't load it, and then none of the session's dynamic channels start (`channels/drdynvc/client/drdynvc_main.c`: a failed `dvcman_load_addin` stops their setup). The dependency check doesn't test for the channel yet (roadmap).
