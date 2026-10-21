# 0029. Plain RDP connections ("Add connection")

- Status: Accepted (the owner's answers below, 2026-10-20); built (steps 1–5, 2026-10-20). Spec §1, §3 and §13 changed to match. Follow-up steps: [Compared with Remmina](#compared-with-remmina-follow-up-steps).
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
4. **Screens,** in two commits:
   - 4a: the sidebar entries, the Connections page, the Add/edit form (one Server field, `host`, `host:port` or `[IPv6]:port`; user name; an optional password straight to the keyring; domain, empty for automatic), Remove connection. App tests: add → listed and saved; invalid input refused with a message; edit keeps the ID; remove → password, pin, settings and favorite gone; removed elsewhere → the open form closes.
   - 4b: connecting: the password dialog without Entra, or the remembered password; the Direct launch target; settings app → connection; certificate texts without Azure; remove while connected asks first.
   - How 4b fits in: a row's *owner* is its account, or the saved connection itself (its ID is both its key and its owner's ID, with epoch 0, since removing and adding again gives a new ID). The connect path records owners where it recorded accounts. A connection's password uses the desktops' keyring guards (a save queued before a Forget or removal writes nothing). The logon that worked is kept in the connection, where the form shows it. A user name is required: with NLA, FreeRDP would otherwise ask on a terminal Deskfeed doesn't have.
   - Not covered by automated tests: a session that finishes starting after its connection was removed is stopped (it needs a real process); manual test 20.
5. **Tray, docs, spec:** connections in the tray menu; README, CHANGELOG, this ADR to Accepted; spec §1 and §13; manual tests.

Size: about the accounts rework plus the settings pages together, a few thousand lines with tests. RD Gateway or `.rdp` import would add about half again, each.

## Testing on the owner's side

A Windows machine or VM the owner sets up. In Windows: Settings → System → Remote Desktop → on; a local user with a password; note the PC's name or IP address. Then manual tests: add, connect (password dialog, certificate approval the first time), remember the password, reconnect after a network drop, remove.

## Decisions (owner, 2026-10-20)

1. **Sidebar:** one "Connections" entry listing all saved connections.
2. **Security:** NLA only (`/sec:nla`); older machines without NLA don't connect.
3. **Scope:** direct connections only in the first version; RD Gateway and `.rdp` import later, each its own decision.
4. **Spec:** widened: §1 now also covers Windows machines by host name, and §13 lists a direct connection's checks.

## Compared with Remmina (follow-up steps)

The owner compared the form with Remmina's RDP profile (2026-10-20). The first version has Server, User name, Password and Domain. The rest of Remmina's fields, with what FreeRDP 3.32.2 offers for each and what Deskfeed would do. None of this is built; each line is its own step.

**Connection settings** (on the settings levels, so for AVD desktops too, not only for saved connections). Built 2026-10-20: window size, color depth, network type and span; each default passes no argument, so FreeRDP behaves as before. Checked against FreeRDP 3.32.2 with the local test server ([compatibility](../compatibility.md)); `/multimon:force` left out, see below.

| Remmina | FreeRDP | Plan |
|---|---|---|
| Resolution (initial window size, client resolution, custom) | `/size:N%`, `/dynamic-resolution` | A "window size" choice next to full screen: FreeRDP's 1024 × 768 or a share of the monitor; the desktop already follows the window (`/dynamic-resolution`). No custom size: with tiling on, COSMIC sizes the window anyway. |
| Color depth | `/bpp:` | A choice; the default stays FreeRDP's. |
| Network connection type | `/network:` (`modem` … `lan`, `auto`) | A choice, default `auto`. |
| Span, force multi monitor | `+span`, `/multimon:force` | Span: a toggle under "all monitors" (one wide screen instead of one per monitor). Force: left out. It only sends the monitor layout to a server that didn't announce support for it, and FreeRDP itself warns it "may break connectivity" (`libfreerdp/core/gcc.c`). |
| List monitor IDs | `/list:monitor`, `/monitors:` | Already on the roadmap: choosing specific monitors (needs a second monitor to test). |
| Keyboard mapping | `/kbd:` | Only if the automatic layout turns out wrong for someone; FreeRDP takes it from the system. |

**Need the owner's decision** (each widens what a connection can reach or how it logs on):

| Remmina | FreeRDP | Why it needs a decision |
|---|---|---|
| Share folder | `/drive:name,path` | **Built 2026-10-21** (below). |
| Smartcard logon (and PIN) | `/smartcard-logon`, `/smartcard` | The spec excludes smartcard redirection (§3); it would need a card and reader to test. |
| Restricted admin mode | `+restricted-admin` | **Built 2026-10-21** (below). |

**Advised against:**

| Remmina | FreeRDP | Why |
|---|---|---|
| Password hash | `/pth:` | Pass-the-hash: an NT hash is as good as the password, and storing or typing one is an attack technique more than a logon method. |

**Not needed:**

| Remmina | Why |
|---|---|
| Server history | The Connections list is the history. |
| Left-handed mouse, disable smooth scrolling | Remmina's own input handling. FreeRDP gets the pointer from XWayland with COSMIC's mouse settings already applied; to be checked once someone misses them. |

### Shared folder (built 2026-10-21)

The owner started it on 2026-10-21, as proposed above.

- **Per desktop or saved connection, off by default** (spec §9): "Choose…" in its settings drawer opens COSMIC's folder chooser (the desktop portal, libcosmic's `xdg-portal` feature, already used for diagnostics), "Stop sharing" removes it. No app or account level: sharing a folder is a choice for one machine. Stored in `config.shared_folders` by key; removing a saved connection removes it.
- **Read and write.** FreeRDP 3.32.2 has no read-only share; the setting's text says so.
- **The administrator's policy wins** for AVD: only when the connection file has `drivestoredirect` with a value (`*`, `DynamicDrives`, a list). Without it the row says the folder isn't shared and why, like the camera. The file's own `drivestoredirect` still never reaches FreeRDP, so only the chosen folder is shared. A saved connection has no such policy.
- **What FreeRDP can carry** (`client/common/cmdline.c`, `channels/drive/client/drive_main.c`): `/drive:` splits at commas and reads the path up to 260 bytes, so such paths are refused when chosen and again at launch. The share name is the folder's own name in letters, digits, `-` and `_` (FreeRDP would replace `:<>"/\|` and spaces anyway), "Deskfeed" when nothing is left.
- **A missing folder** (deleted, a disk not plugged in) doesn't stop the connection: it connects without it and the row says so. The check touches the disk, so it runs with the launch, off the UI thread.
- Checked: the real `xfreerdp3` 3.32.2 takes the argument (a path with a space) and loads its device channel (`rdpdr`). The local test server has no drive channel, so Windows showing the folder is manual test 24.

### Restricted admin mode (built 2026-10-21)

The owner started it on 2026-10-21.

- **What FreeRDP 3.32.2 does** (`client/common/cmdline.c`, `libfreerdp/core/nla.c`, `nego.c`): `+restricted-admin` asks for the mode in the first request; the password still logs on over NLA but isn't handed to the machine (empty `TSPasswordCreds`), so a compromised machine can't reuse it. It also joins the console session (`ConsoleSession`), which on a Windows PC is the only one anyway. A machine that doesn't announce the mode in its first answer is refused by FreeRDP with "restricted-admin mode requested but not supported, terminating".
- **Where:** a "Restricted admin mode" switch in a saved connection's form, stored with it (`restricted_admin`, off; older saved connections load with it off). It travels in the launch target (`Target::Direct { restricted_admin }`), so an AVD desktop can never get it: AVD doesn't support it.
- **When it's refused:** FreeRDP's own refusal line has its own ending and text (turn it off, or ask the machine's administrator), is never retried, and isn't taken for a wrong password (it was never tried). What a Windows machine that doesn't allow the mode, or a user who isn't an administrator there, actually answers is **not known yet**: possibly an account restriction (FreeRDP exit 156), possibly a logon failure. So any failure before connecting with the mode on adds a hint that it is on, instead of guessing the cause. Manual test 25 finds out; then the hint can become a specific message.
- Checked against the local test server: with the mode offered (FreeRDP's server default) the client asks for it (`RDP_NEG_REQ::flags RESTRICTED_ADMIN_MODE_REQUIRED`) and connects. Not offered (`DESKFEED_TEST_NO_RESTRICTED_ADMIN=1`), FreeRDP's server drops the request itself, so the client ended with a generic failure (147), not its refusal line: that line is tested from FreeRDP's source text.

