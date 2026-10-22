# Deskfeed user guide

Everything Deskfeed does and every setting it has. For installing it, see the [README](../README.md#installation); for what it can't do, [Known limitations](../README.md#known-limitations).

The names below are the ones the app shows. Each setting also has a short description under it in the app.

## Contents

- [Accounts](#accounts)
- [Desktops](#desktops)
- [Saved connections (a Windows machine by name)](#saved-connections-a-windows-machine-by-name)
- [Inside a desktop: keys and windows](#inside-a-desktop-keys-and-windows)
- [Running in the background and the tray](#running-in-the-background-and-the-tray)
- [Settings](#settings)
- [When something goes wrong](#when-something-goes-wrong)

## Accounts

An account is your Microsoft work or school account (Azure Virtual Desktop or Windows 365). Each one has its own entry in the sidebar.

- **Sign in:** select **Sign in**. Microsoft's sign-in page opens in a small window; sign in as you normally do (password, Authenticator, your company's own page). The window stays open for at most the [Time allowed for signing in](#time-allowed-for-signing-in).
- **Add another account:** **Add account** in the sidebar, or **Account → Add account**.
- **Staying signed in:** Deskfeed keeps the sign-in in your desktop keyring (Secret Service), never in files, and renews it by itself. When your organization ends it (many ask for a fresh sign-in after some time), the account page shows **Sign in** again. Without a keyring you stay signed in only until Deskfeed quits.
- **Sign out:** **Account → Sign out**, or **Sign out** at the bottom of the account's settings. With desktops open, it asks first ("Disconnect and sign out"). Signing out deletes the account's saved sign-in, its cookies, its remembered Windows passwords and its offline list from this computer. The account stays in the sidebar, signed out, so you can sign in again with one click.
- **Remove account:** on a signed-out account's page. It takes the account off the sidebar, with its account settings.
- **Offline list:** when the live list can't be loaded (no network, or the sign-in has ended), the last list is shown, marked "Offline list from …". You can't connect from it. It is kept for [Keep the offline desktop list for](#keep-the-offline-desktop-list-for).

## Desktops

The account's page lists every desktop and app your organization assigned to you, from all its workspaces. The list loads again every [Refresh desktop lists every](#refresh-desktop-lists-every), and after the computer wakes up. "Updated …" shows when it last loaded.

- **Search desktops:** the search field filters by desktop or workspace name.
- **Favorites:** the star on a row marks a favorite. Favorites show at the top under **Favorites**.
- **Connect:** opens the desktop with its [settings](#settings). The row shows how far it got, and why when it fails.
- **Microsoft's page when connecting:** connecting with Entra sign-in opens Microsoft's page once more. Deskfeed passes your account along, so it is normally filled in already. FreeRDP fetches the desktop's sign-in itself, so Deskfeed can't skip this page (see [Known limitations](../README.md#known-limitations)).
- **Show:** brings a connected desktop's window to the front, switching to its workspace.
- **Disconnect:** ends the connection. Windows normally keeps your session, and connecting again returns to it.
- **Reconnecting:** after a lost connection the row shows "Connection lost. Reconnecting, attempt 1 of 5…". See [Reconnect after a lost connection](#reconnect-after-a-lost-connection).
- **Desktop settings:** the gear on a row. See [Settings](#settings).

### Logon setting and the password dialog

Most desktops connect with **Entra sign-in, no password (recommended)**. Some organizations set their desktops up for a Windows password instead; then Deskfeed shows a password dialog. Its **Logon setting** is remembered for that desktop:

- **Entra sign-in, no password (recommended):** your Microsoft sign-in logs you on to Windows.
- **Password (standard):** your Windows password, with the user name and domain Azure Virtual Desktop gives for your account.
- **Empty domain:** the same, with an empty domain. Some setups need it.
- **Custom domain…:** the same, with a domain you type, for example CORP.

If Entra sign-in fails for a desktop, Deskfeed asks for the Windows password the next time. Ticking **Remember this password in the keyring** saves the password for that desktop, in the keyring only; the desktop's settings then show **Remembered password** with **Remove**. A password Windows refuses is deleted again. Deskfeed never retries a refused password by itself, so your account doesn't get locked.

### Certificates

Azure Virtual Desktop vouches for its desktops' certificates, so you rarely see this. When a certificate can't be verified, Deskfeed shows it (name, issuer, validity, fingerprint) and asks before connecting. **Trust and connect** approves it for that desktop only; the desktop's settings then show **Approved certificate** with **Remove**. If an approved desktop later shows a different certificate that can't be verified either, Deskfeed warns that someone may be intercepting the connection.

### Apps (RemoteApps)

Apps from the feed are listed with the desktops and connect the same way. They open as ordinary windows, grouped under Deskfeed in the dock, on every monitor. Settings that only fit a desktop's window (full screen, window size, workspaces, keyboard) don't show for apps. Apps are a preview: see [Known limitations](../README.md#known-limitations).

## Saved connections (a Windows machine by name)

For a Windows PC or server you reach directly, without Azure.

- **Add connection** in the sidebar opens the form:
  - **Name:** optional; the server otherwise.
  - **Server:** host name or IP address, optionally `:port` (3389 otherwise), for example `pc01`, `pc01.example.lan:3390`.
  - **User name:** your Windows user name.
  - **Domain:** optional.
  - **Password:** optional. Given here, it is remembered in the keyring; left empty, Deskfeed asks when connecting (with "Remember this password in the keyring").
  - **Restricted admin mode:** your password logs you on but isn't handed to the machine, so a compromised machine can't reuse it. Only for administrators of that machine, and only where Windows allows it; you then have no access to network shares from the session. It also connects to the machine's console session.
- Saved connections are listed under **Connections** in the sidebar, with favorites at the top. They connect, show and disconnect like desktops, and appear in the tray.
- The gear edits the connection; **Remove connection** deletes it, its remembered password and its approved certificate (it asks first if it is open).
- **Connection settings** are the desktop settings below, for this connection only. A saved connection has no account, so "App setting" follows View → Settings.
- The machine needs Remote Desktop turned on with Network Level Authentication (Windows' default). The first connection usually asks you to approve the machine's own certificate.

## Inside a desktop: keys and windows

- **Ctrl+Alt+Enter** switches between full screen and a window (see [Ctrl+Alt+Enter switches full screen](#ctrlaltenter-switches-full-screen)).
- **Right Ctrl**, pressed and released, gives COSMIC's shortcuts back while [Send shortcuts like Super and Alt+Tab to the desktop](#send-shortcuts-like-super-and-alttab-to-the-desktop) is on. Moving the pointer out of the window does too.
- **Ctrl+Alt+M** minimizes the window and **Ctrl+Alt+D** disconnects. These are FreeRDP's own and can't be turned off.
- The desktop's window is grouped under Deskfeed in the dock. Resizing it resizes the Windows desktop.

## Running in the background and the tray

With [Keep running in the background](#keep-running-in-the-background) on, closing the window keeps Deskfeed and your desktops running, with an icon in the panel's status area:

- The tray menu lists your desktops and saved connections (favorites first), with a check mark on connected ones. Selecting a connected one brings it to the front; selecting another connects it. When Deskfeed has to ask something (a password, for example), the window opens.
- **Show Deskfeed** opens the window; **Quit** quits.
- **Quit** in the Account menu, or Ctrl+Q, quits completely. With desktops open it asks first: **Disconnect and quit**, **Cancel**, or (when the panel has a status area) **Keep running in the background**.
- With the setting off, closing the window quits the same way.

## Settings

Settings come in three levels. Each level starts with the one above and can change any setting:

1. **App:** **View → Settings**. The defaults for every account, desktop and saved connection.
2. **Account:** the gear in the header bar, for the selected account. Each setting is "App setting" or its own value.
3. **Desktop:** the gear on a desktop's row (or a saved connection's **Connection settings**). Each setting is "Account setting" (for a saved connection, "App setting") or its own value.

Settings take effect when a desktop connects. A desktop that is open keeps its settings until you connect it again; the settings page says so.

Your organization's rules win: if a desktop doesn't allow the clipboard, cameras or drives, the row says so and the setting has no effect there.

In the lists below, "Where" says on which levels a setting can be changed.

### Sign-in and desktop list

Only at the app and account levels.

#### Time allowed for signing in

How long Microsoft's sign-in window stays open, including extra verification such as an authenticator app. Default 5 minutes (2–15). Where: app, account.

#### Refresh desktop lists every

How often the list of desktops is loaded again, so new and removed desktops show up. Default 60 minutes (15 minutes to 24 hours). Where: app, account.

#### Keep the offline desktop list for

How long the last loaded list is kept for showing while the live one can't be loaded. It never lets you connect by itself, and signing out deletes it. Default 30 days (1–90). Where: app, account.

### Connection

#### Time to wait for the desktop

How long to wait for Windows to answer when connecting. Some desktops take close to a minute. Default 120 seconds (10–600). Where: app, account, desktop.

#### Reconnect after a lost connection

When the network drops or the computer wakes from sleep, connect again by itself, waiting a little longer each time. With Entra sign-in, Microsoft's page may appear again. Desktops that use a Windows password aren't reconnected by themselves. Default on. Where: app, account, desktop.

#### Reconnect attempts

How many times to try before giving up. The waits between tries grow from 5 seconds to 1 minute. **Stop** on the row ends the attempts. Default 5 (1–10). Where: app, account, desktop.

#### Network type

Windows adjusts image quality and effects to the connection: Automatic (measures it), LAN, WAN, Broadband, Slow broadband, or Modem. Default Automatic. Where: app, account, desktop.

### Display

#### Display scaling

How large Windows draws text and apps. Automatic matches the monitor's scale in COSMIC's display settings, so the desktop looks like the apps around it; or 100–300 %. Default Automatic. Where: app, account, desktop.

#### Color depth

Fewer colors need less bandwidth on a slow connection: 32-bit (full color), 24, 16, 15 or 8-bit. Default 32-bit. Where: app, account, desktop.

#### Full screen

Open the desktop in full screen. Ctrl+Alt+Enter switches while it is open. Default off. Where: app, account, desktop. Not for apps.

#### Window size

How large a desktop's window opens, as a share of the monitor (50–100 %), or 1024 × 768. The desktop follows when the window is resized. With COSMIC's tiling on, COSMIC sizes the window instead. Not used in full screen. Default 1024 × 768. Where: app, account, desktop. Not for apps.

#### Use all monitors

In full screen, the desktop covers all monitors, and Windows sees each monitor as its own screen. Default off. Where: app, account, desktop. Not for apps (their windows can always go on any monitor).

#### Span as one screen

With "Use all monitors" in full screen: Windows sees one wide screen instead of one per monitor. For older Windows versions or apps that don't handle several monitors. Default off. Where: app, account, desktop. Not for apps.

#### Open on a new workspace

The desktop opens on an empty COSMIC workspace, and COSMIC switches to it as connecting starts; Microsoft's sign-in page opens there too. If nothing opens (the connection fails or you cancel), it switches back. Default off. Where: app, account, desktop. Not for apps.

#### Keep the screen on in full screen

While a full-screen desktop is in front, this computer doesn't turn off the screen, lock or go to sleep. It does again as soon as you switch away or leave full screen. Default off. Where: app, account, desktop. Not for apps.

### Keyboard

Not for apps.

#### Send shortcuts like Super and Alt+Tab to the desktop

While the pointer is over the desktop's window, COSMIC's shortcuts go to Windows: Super opens the Start menu. Press and release Right Ctrl to get them back; moving the pointer out of the window does too. Default on. Where: app, account, desktop.

#### Ctrl+Alt+Enter switches full screen

Switch between full screen and a window from the keyboard. Off, Windows gets Ctrl+Alt+Enter. Default on. Where: app, account, desktop.

### Devices

#### Sound

Play the desktop's sound on this computer. Default on. Where: app, account, desktop.

#### Microphone

Let the desktop use this computer's microphone. Default off. Where: app, account, desktop.

#### Camera

Let the desktop use this computer's cameras. Your organization must allow cameras for the desktop too; otherwise the row says so. A camera plugged in during a session appears after you reconnect. Default off. Where: app, account, desktop.

#### Clipboard

Copy and paste between this computer and the desktop: Off, Both directions, Only to the desktop, or Only to this computer. Default off. Where: app, account, desktop.

#### Copy files through the clipboard

Also let copied files through, in the clipboard's direction. Off, only text and pictures are shared. Default off. Where: app, account, desktop.

### Desktop only

#### Share a folder

A folder on this computer that Windows shows as a drive ("<folder> on <this computer>"). **Choose…** picks it; **Stop sharing** removes it. Programs in Windows can read, change and delete what's in it: share only a folder meant for that. If the folder isn't there when you connect (deleted, or on a disk that isn't plugged in), the desktop connects without it and the row says so. Default not shared. Where: desktop and saved connection only. Your organization must allow drives on the desktop.

#### Approved certificate and Remembered password

Shown only when there is one; **Remove** forgets it. See [Certificates](#certificates) and [Logon setting and the password dialog](#logon-setting-and-the-password-dialog).

### App only

In View → Settings.

#### Theme

Match desktop, Dark or Light. Default Match desktop.

#### Keep running in the background

Closing the window keeps Deskfeed and your desktops running, with an icon in the panel's status area. When the panel has no status area, closing the window quits as usual. Default off. See [Running in the background and the tray](#running-in-the-background-and-the-tray).

#### Start when you log in

With "Keep running in the background" on, Deskfeed starts with only its icon in the panel; otherwise its window opens. Default off.

#### Log file size

When the log reaches this size it starts again; the previous log is kept as `app.log.1`. Default 1 MB (1–100).

## When something goes wrong

- **The row says why.** Failures are explained in plain words (for example "the desktop refused the sign-in", "the desktop is starting up"), with FreeRDP's code for support.
- **View → Dependencies:** what Deskfeed needs (FreeRDP, WebKitGTK, COSMIC's window management) and whether it's there. Deskfeed also checks at startup.
- **View → Open log folder:** the log (`~/.local/state/cosmic-ext-deskfeed/app.log`). It has no passwords or tokens, and names, addresses and IDs are replaced, but check it before sharing.
- **View → Diagnostics:** a report for support (versions, the dependency check, settings, the end of the log), without passwords, tokens or cookies. Read it, then **Save…**.
- **View → About:** the version and links.
