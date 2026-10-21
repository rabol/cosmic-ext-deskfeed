# 23. Window size, color depth, network type

- **Status:** To do
- **Area:** Desktop sessions
- **Why it's manual:** Only your eyes can judge the size and colors.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed. Or an AVD desktop.
- **Background:** [ADR 0029](../adr/0029-plain-rdp-connections.md)

## Steps

1. In the settings, set Window size to 70 % of the monitor, Color depth to 16-bit and Network type to Slow broadband.
2. Connect.
3. Set them back to their first entries (or "Account setting") and connect again.

## Expected

- With COSMIC's tiling off (floating window), the window opens at about 70 % of the monitor; with tiling on, COSMIC sizes it.
- Colors look coarser (gradients show bands).
- It connects as before.
- Set back: as before these settings existed.
