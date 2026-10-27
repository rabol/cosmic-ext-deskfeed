# 19. Wrong password and domain

- **Status:** Automated (2026-10-27)
- **Area:** Plain RDP connections
- **Why it's manual:** Needs a real Windows machine.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed.
- **Background:** [ADR 0029](../../adr/0029-plain-rdp-connections.md)

## Steps

1. Edit the connection and set a domain Windows doesn't know, or type a wrong password.
2. Connect.

## Expected

- The row says the user name or password was rejected.
- Nothing is remembered.

## Automated

App tests: `app::decide::tests::wrong_password_only_when_a_password_was_used` and `every_credential_refusal_counts_as_refusing_the_password` (what a refusal means, and that the remembered password is then deleted); the launcher tests read FreeRDP's refusal lines from recorded output.

Not checked: Windows' real refusal for a wrong password. In the release check.
