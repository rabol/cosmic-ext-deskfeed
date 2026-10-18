# 0015. Windows' display scaling follows the X11 scale from XSETTINGS

- Status: Accepted (pending the owner's look at a real desktop)
- Date: 2026-10-18

## Context

Spec §3 and §8: display scaling follows the monitor's scale, derived, never hardcoded. With `xfreerdp3` (an X11 window, ADR 0004) on the owner's 4K monitor at 1.25, Windows ran at 100 % and its text was smaller than the local apps'.

How COSMIC scales X11 windows (cosmic-comp 1.10.0, `41497b4`, `src/xwayland.rs` `update_xwayland_settings`; `cosmic-comp-config` `XwaylandDescaling`):

- Default "fractional" descaling: X11 windows get the full pixels of the XWayland-primary monitor (or the largest scale), and X11 programs are told the matching DPI through XSETTINGS: `Xft/DPI` = 96 × 1024 × scale.
- With descaling off, COSMIC scales X11 windows up itself and `Xft/DPI` is 96 × 1024.

So `Xft/DPI` is exactly the scale FreeRDP's window gets, in both modes, without Deskfeed copying COSMIC's rules or reading its private state files.

FreeRDP 3.32.2: `xfreerdp3` doesn't derive a scale itself. `/scale-desktop:<100–500>` sets the desktop scale, sent when connecting (`TS_UD_CS_CORE` desktopScaleFactor, `libfreerdp/core/gcc.c`) and with every display-control layout (`client/X11/xf_disp.c`). FreeRDP's SDL client, which derives it from the display, sends the display scale as desktop scale and 100 as device scale (`client/SDL/SDL3/sdl_window.cpp`); Deskfeed does the same.

## Decision

- `display.rs` reads `Xft/DPI` from the XSETTINGS manager (`_XSETTINGS_S<screen>` selection owner, `_XSETTINGS_SETTINGS` property, at most 64 KiB, parsed per the XSETTINGS specification) with `x11rb`, core protocol only. `x11rb` 0.13 is already built into the app through libcosmic's renderer (`iced_wgpu`); the direct dependency adds no crate (`Cargo.lock` gained one dependency edge only).
- A "Display scaling" setting at App → Account → Connection: Automatic (default) or 100, 125, 150, 175, 200, 250, 300 %. Automatic is resolved when a desktop starts: the X11 scale, rounded to a percent, 100–500; 100 % with a log line when it can't be read.
- FreeRDP always gets `/scale-desktop:<percent>`; the device scale stays FreeRDP's default (100).

## Evidence

- On the owner's session the reader returns 1.25 → 125 % (`display::tests::real_x11_scale`).
- With Deskfeed's launch path against the local test server, the server logged `client desktop scale 125 %` at connect and `monitor layout 1904x1000, desktop scale 125 %, device scale 100 %`.

Still needed: the owner's look at a real Windows desktop (text the same size as local apps).

## Consequences

- The scale is read when a desktop starts; changing COSMIC's scale during a session applies at the next connect.
- With monitors of different scales, X11 has one scale (COSMIC's XWayland-primary monitor), so Windows matches that monitor.
- Scaling the picture instead of resizing Windows' desktop (`/smart-sizing`) stays out: it would need a fixed desktop size beside it (settings inventory).
