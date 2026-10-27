# 18. Remembered password

- **Status:** Automated (2026-10-27)
- **Area:** Plain RDP connections
- **Why it's manual:** Needs a real Windows machine and the real keyring.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed.
- **Background:** [ADR 0029](../../adr/0029-plain-rdp-connections.md)

## Steps

1. Connect with "Remember this password" ticked.
2. Disconnect and connect again.
3. Open Seahorse and look for the item.
4. Edit the connection and give the password in the form instead (a new connection, or after removing the remembered one in its settings), and connect.

## Expected

- The second connect asks no password.
- Seahorse shows "Deskfeed RDP password (<name>)".
- With the password in the form: the same, without the dialog.

## Automated

App tests: `app::tests::saved_connections::a_remembered_password_is_read_and_a_missing_one_asked_for` (second connect without the dialog) and `a_password_given_is_marked_remembered_never_saved_in_the_settings` (the password in the form). Keyring tests against a stand-in Secret Service: `secrets::tests::connection_passwords_are_their_own_items` (one item per connection, labelled "Deskfeed RDP password (<name>)").

Not checked: your real keyring (oo7) and Seahorse. In the release check.
