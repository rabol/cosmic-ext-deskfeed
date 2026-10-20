# 0029. Plain RDP connections ("Add connection")

- Status: Proposed. Needs the owner's answers to the open questions at the end, and a spec change (§1, §13), before work starts.
- Date: 2026-10-20

## Context

The owner asked what it would take for Deskfeed to also connect to Windows machines directly by host name, as a plain RDP client, next to Azure Virtual Desktop and Windows 365. The sidebar entry for it is to be "Add connection" (owner, 2026-10-20), next to "Add account".

The spec defines Deskfeed as an AVD and Windows 365 client (§1: "discover entitled Azure Virtual Desktop and Windows 365 resources"). This would widen that.

What Deskfeed already has that a plain connection can use:
- FreeRDP launch with the password sent through a private pipe, never the command line or environment (`/args-from:fd`), and every session supervised and ended with the app (ADR 0022).
- The password dialog, with the logon choices (automatic, empty domain, a typed domain), and remembering the password in the keyring, opt-in.
- The certificate approval dialog and per-desktop pins (ADR 0016). For AVD it is a fallback, because the broker vouches for the session host; for plain hosts, often with self-signed certificates, it becomes the main path. It already works against the local test server.
- Connection settings with levels (app, account, desktop), reconnect (ADR 0014), full screen and monitors, new workspace (ADR 0011), keyboard shortcuts (ADR 0018), keep the screen on (ADR 0019), favorites, search, and the tray menu (ADR 0017).

What assumes Azure today:
- `LaunchSpec` and `build_args` always add `/gateway:type:arm` and `/azure:auth-helper:…`, and take a connection file from Microsoft's feed.
- The connection-file rules (`rdpfile.rs`, ADR 0024) refuse any file without Azure's gateway.
- Remembered passwords are keyring items keyed by a Microsoft account (tenant and object ID) and a desktop.
- Settings levels are app → account → desktop; a plain connection has no account.
- Desktops come from an account's feed; desk state, rows and the tray are keyed by an opaque resource key.

## Proposal

### What the user sees

- The sidebar: the accounts, then (after a divider) **Connections**, one page listing every saved connection as rows like a desktop's (Connect, favorite, settings, status), then **Add account** and **Add connection** at the bottom. With nothing saved yet, only the two "Add" entries (the first reading "Sign in", as now).
- **Add connection** opens a form: name (optional; the host otherwise), host name or IP address, port (3389 by default), user name, and logon (automatic, empty domain, or a domain). Save, or Save and connect.
- A connection's settings drawer: the form's fields to edit, the connection settings (following the app's, overridable, as for a desktop), Security (the approved certificate, the remembered password), and **Remove connection**.
- Connect: the password dialog as for a password-logon desktop, without the Entra choice; "Remember this password" as now. A certificate the system doesn't trust: the approval dialog (ADR 0016), worded without "Azure didn't vouch for it".

### How it works

- **Saved connection** (`config.connections`, by a random opaque ID, never the host): name, host, port, user name, logon setting. No password: that stays in the keyring, opt-in.
- **Checked input:** host is a DNS name, an IPv4 address or a bracketed IPv6 address, with no spaces, commas, control characters or user info; port 1–65535; user name and name printable and bounded. The same checks run again before launch, since the arguments go to FreeRDP one per line.
- **Launch target:** `LaunchSpec` gets a target, either Azure (gateway token, sign-in helper, profile, login hint: as now) or Direct. Direct writes a connection file Deskfeed makes itself (`full address`, nothing from outside), and leaves out `/gateway:` and `/azure:auth-helper:`. Security is `/sec:nla` (see the open questions). Everything else is the existing path.
- **Keyring:** a remembered password is its own item kind (`type` `rdp-password`, attribute `connection` = the ID), apart from AVD items, so neither search sees the other. It is deleted with the connection.
- **Settings:** two levels, app → connection; the connection level is the existing desktop level keyed by the connection's ID.
- **Remove connection:** asks first if it is connected; deletes the remembered password, the approved certificate, its settings and favorite. Nothing about accounts changes.

### Not in the first version

Each is a later decision:
- **RD Gateway** (machines behind a company gateway): FreeRDP's `/gateway:g:…` with its own credentials.
- **Importing `.rdp` files:** needs its own rule table (an allowlist for plain hosts), not the AVD one.
- **Entra-joined PCs** (`/sec:aad` to a plain host) and RemoteApps on plain hosts.
- Choosing specific monitors (roadmap, needs a second monitor).

## Security

- The host is the user's choice; Deskfeed never connects anywhere it wasn't told to.
- No `/cert:ignore`, ever. An unknown certificate needs the user's approval, and a changed one is blocked unless the system trusts it (ADR 0016's addendum applies unchanged).
- The password never reaches the command line, the environment or the log, as for AVD.
- No connection file from outside in this version, so nothing can add drives, printers or a shell (`alternate shell`) behind the user's back.

## Work plan

One commit per step, `just ci` before each, tests before restructuring, each new test checked to fail with its rule broken:

1. **Data and checks:** the saved connection, its checks, `config.connections`. Unit tests, including random input for host and user name.
2. **Launch target:** Azure vs Direct in `LaunchSpec`/`build_args`. Fake-FreeRDP tests: no gateway or auth-helper arguments, password only through the pipe, the pin used, bad host or user refused. An ignored test against the local test server.
3. **Keyring item kind:** save, read, delete on the fake keyring; apart from AVD items.
4. **Screens:** the sidebar entries, the Connections page, the Add/edit form, Remove connection. App tests driving messages: add → listed and saved; invalid input refused with a message; connect → password dialog without Entra; remove → password, pin, settings and favorite gone; remove while connected asks first.
5. **Tray, docs, spec:** connections in the tray menu; README, CHANGELOG, this ADR to Accepted; spec §1 and §13; manual tests.

Size: about the accounts rework plus the settings pages together, a few thousand lines with tests. RD Gateway or `.rdp` import would add about half again, each.

## Testing on the owner's side

A Windows machine or VM the owner sets up. In Windows: Settings → System → Remote Desktop → on; a local user with a password; note the PC's name or IP address. Then manual tests: add, connect (password dialog, certificate approval the first time), remember the password, reconnect after a network drop, remove.

## Open questions for the owner

1. **Sidebar:** one "Connections" entry with all of them (proposed), or one sidebar entry per connection, like accounts?
2. **Security:** NLA only (proposed; Windows' default), or also allow TLS without NLA for older machines, as a per-connection setting?
3. **Scope:** the first version without RD Gateway and `.rdp` import (proposed), or include one of them?
4. **Spec:** agree to widen §1 from "AVD and Windows 365" to "and Windows machines by host name"?
