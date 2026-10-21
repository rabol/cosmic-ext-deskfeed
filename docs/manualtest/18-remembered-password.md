# 18. Remembered password

- **Status:** To do
- **Area:** Plain RDP connections
- **Why it's manual:** Needs a real Windows machine and the real keyring.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed.
- **Background:** [ADR 0029](../adr/0029-plain-rdp-connections.md)

## Steps

1. Connect with "Remember this password" ticked.
2. Disconnect and connect again.
3. Open Seahorse and look for the item.
4. Edit the connection and give the password in the form instead (a new connection, or after removing the remembered one in its settings), and connect.

## Expected

- The second connect asks no password.
- Seahorse shows "Deskfeed RDP password (<name>)".
- With the password in the form: the same, without the dialog.
