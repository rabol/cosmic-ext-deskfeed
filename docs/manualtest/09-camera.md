# 9. Camera

- **Status:** Completed but not tested
- **Area:** Desktop sessions
- **Why it's manual:** Needs a real camera, and the desktop's administrator must allow camera redirection.
- **Needs:** A webcam; an AVD desktop whose administrator allows cameras (needs AVD admin rights to set up), or a Windows machine.
- **Background:** [ADR 0013](../adr/0013-camera-redirection.md)

## Steps

1. Turn on the Camera setting for the desktop and connect.
2. If the row says "Camera not shared: your organization doesn't allow cameras on this desktop" (or only specific cameras), stop: the test can't run on this desktop.
3. Open Windows' Camera app.
4. Unplug the camera and plug it back in.

## Expected

- The camera works in Windows' Camera app.
- After replugging: note what happens. FreeRDP 3.32.2 doesn't notice a camera plugged in later, so it may be gone until you reconnect.

## Notes

Built and covered by automated tests up to what FreeRDP sends: the local test server sees the camera offered (ADR 0013). Not checked against a real desktop: your organization's desktops may not allow cameras, and setting that up needs AVD admin rights. Can be tried on win-100 with a webcam.
