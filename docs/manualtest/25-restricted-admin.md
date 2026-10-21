# 25. Restricted admin mode

- **Status:** Completed but not tested
- **Area:** Plain RDP connections
- **Why it's manual:** Needs a Windows administrator account and a registry change on the Windows machine.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed. An administrator account there.
- **Background:** [ADR 0029](../adr/0029-plain-rdp-connections.md)

## Steps

1. Edit the saved connection, turn on Restricted admin mode, save and connect, without changing anything in Windows.
2. If it fails: note the exact text on the row, and copy the last lines from View → Open log folder.
3. In Windows, as administrator in a terminal: `reg add HKLM\System\CurrentControlSet\Control\Lsa /v DisableRestrictedAdmin /t REG_DWORD /d 0 /f`. Connect again.
4. Optionally connect as a user who isn't an administrator there, and note the row's text.
5. Afterwards, set it back if you like: the same command with `/d 1`.

## Expected

- Without the registry change: it connects (the mode is already allowed), or the row says it was refused or that restricted admin mode is on.
- With the registry change: it connects as the administrator account.

## Notes

Built and checked against the local test server, which accepts the mode, and with the server set to refuse it (ADR 0029). What Windows itself answers is not known. If you run it, send the row text and log lines: they let the message be made specific.
