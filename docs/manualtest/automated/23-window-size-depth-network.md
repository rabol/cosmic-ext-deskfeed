# 23. Window size, color depth, network type

- **Status:** Automated (2026-10-27)
- **Area:** Desktop sessions
- **Why it's manual:** Only your eyes can judge the size and colors.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed. Or an AVD desktop.
- **Background:** [ADR 0029](../../adr/0029-plain-rdp-connections.md)

## Steps

1. In the settings, set Window size to 70 % of the monitor, Color depth to 16-bit and Network type to Slow broadband.
2. Connect.
3. Set them back to their first entries (or "Account setting") and connect again.

## Expected

- With COSMIC's tiling off (floating window), the window opens at about 70 % of the monitor; with tiling on, COSMIC sizes it.
- Colors look coarser (gradients show bands).
- It connects as before.
- Set back: as before these settings existed.

## Automated

`just session-checks` (`nested_window_size_depth_network`) connects to the local test server with window size 50 %, 16-bit color and network type LAN, and checks what FreeRDP asked the server for: half the monitor each way, 16 bpp, connection type LAN. That the settings set back leave these arguments out is a unit test (`settings.rs`).

Not checked: how Windows draws with fewer colors; that is Windows' part.
