app-title = Deskfeed
app-comment = Your work desktops on COSMIC™, working with Azure Virtual Desktop and Windows 365
app-keywords = AVD;Azure;Virtual Desktop;Windows 365;Remote Desktop;RDP;
about = About
repository = Repository
view = View
git-description = Git commit {$hash} on {$date}

# Sign-in
sign-in-window-title = Sign in to your work account
sign-in = Sign in
sign-out = Sign out
signing-in = Waiting for sign-in…
signing-in-hint = Finish signing in in the sign-in window.
restoring = Signing in…
restoring-hint = Using your saved sign-in.
try-again = Try again
error-saved-sign-in-expired = Your saved sign-in is no longer valid. Sign in again.
error-restore-network = Couldn't reach Microsoft to renew your saved sign-in. Check your network, then try again.
error-keyring-load = Your saved sign-in couldn't be read: { $detail } Sign in to continue.
notice-not-saved = Your sign-in couldn't be saved: { $detail } You'll need to sign in again the next time you start the app.
keyring-unavailable = the system keyring isn't available.
keyring-dismissed = the keyring stayed locked.
keyring-malformed = the saved data couldn't be read.
cancel = Cancel
refresh = Refresh
connect = Connect
unofficial-notice = Deskfeed is an unofficial community app that works with Azure Virtual Desktop and Windows 365. It is not a Microsoft product. For compatibility it signs in with Microsoft's public Remote Desktop client identity.

# Resources
loading-desktops = Loading your desktops…
no-desktops = No desktops or apps are assigned to this account.
unnamed-workspace = Workspace
workspace-failed = This workspace couldn't be loaded: { $detail }
kind-desktop = Desktop
kind-remoteapp = App (not supported yet)
kind-other = Unknown resource type (not supported)

# Errors
error-helper-missing = The sign-in window component (cosmic-ext-deskfeed-signin) is not installed.
error-no-data-dir = Couldn't find a folder to store sign-in data.
error-no-display = The sign-in window couldn't open.
error-sign-in-timeout = Sign-in took too long and was stopped. Try again.
error-access-denied = Your organization's policy blocked the sign-in. Contact your IT department.
error-access-denied-code = Your organization's policy blocked the sign-in. Contact your IT department and mention error AADSTS{ $code }.
error-consent-required = Your organization hasn't approved this app for your account yet. An administrator has to give consent.
error-provider = Microsoft refused the sign-in ({ $code }).
error-network = Couldn't reach Microsoft. Check your internet connection.
error-tls = The secure connection failed. A network device may be intercepting traffic.
error-sign-in-generic = Sign-in failed. Details: { $detail }
error-session-expired = Your sign-in has expired. Sign out and sign in again.
error-forbidden = The desktop service refused access to this account.
error-feed = Couldn't load your desktops: { $detail }
error-generic = Something went wrong. Try again.
error-sign-out-incomplete = Sign-out couldn't remove all sign-in data. Try signing out again.

# Connecting
disconnect = Disconnect
status-preparing = Preparing…
status-connecting = Connecting…
status-connected = Connected
status-logon-failed = Wrong user name or password. Select Connect to try again.
status-entra-failed = Sign-in without a password didn't work. Select Connect to use your Windows password.
connect-to = Connect to { $title }
password-for = Windows password for { $account }
windows-password = Windows password
logon-setting = Logon setting
logon-automatic = Password (standard)
logon-empty-domain = Empty domain
logon-custom-domain = Custom domain…
logon-entra = Entra sign-in, no password (recommended)
domain-placeholder = Domain, for example CORP
password-explainer = The password is used only for this connection and isn't saved. Most desktops connect with Entra sign-in, without a password; organizations set this up differently.
error-entra-failed = Entra sign-in didn't work for this desktop ({ $detail }). Enter your Windows password, or choose another logon setting.
error-logon-failed = Windows rejected the user name or password (logon setting: { $setting }). If your password is correct, try another logon setting; organizations set this up differently.
error-password-empty = Enter your Windows password.
error-password-chars = The password contains a line break, which can't be sent.
error-bad-domain = Enter a domain name using letters, digits, dots and dashes.
error-no-connection-file = The service didn't provide connection details for this desktop.
error-bad-connection-file = The connection details for this desktop couldn't be read ({ $detail }).
error-gateway-not-allowed = This desktop's connection details name a gateway outside Azure Virtual Desktop, so Deskfeed didn't connect.
error-freerdp-missing = FreeRDP (sdl-freerdp3) is not installed. Install the freerdp package.
error-launch = The remote desktop client couldn't start ({ $detail }).
error-account-locked = Your account is locked. Contact your IT department.
error-password-expired = Your Windows password has expired or must be changed. Change it, then try again.
error-account-disabled = Your account is disabled. Contact your IT department.
error-auth-failed = Authentication failed. Try signing out and in again.
error-freerdp-crashed = The remote desktop client stopped unexpectedly.
error-session-failed = The connection ended with an error (code { $code }).

# Dependencies
dependencies = Dependencies
check-again = Check again
deps-checking = Checking…
deps-missing = Some required software is missing or not usable, so connecting may not work. Close this message to see the details.
deps-required = Required
deps-optional = Optional
dep-ok = Found
dep-ok-version = Version { $version }
dep-missing = Not found
dep-too-old = Version { $version } found, { $min } or newer needed
dep-broken = Not usable ({ $reason })
dep-freerdp = FreeRDP (sdl-freerdp3)
dep-freerdp-hint = Connects to your desktops. Install FreeRDP 3.32.2 or newer with the SDL client (Arch Linux: freerdp).
dep-freerdp-entra = FreeRDP Microsoft Entra support
dep-freerdp-entra-hint = Needed for Entra sign-in to desktops. FreeRDP must be built with WITH_AAD=ON (Arch Linux's freerdp is).
dep-signin = Sign-in window
dep-signin-hint = Shows Microsoft's sign-in pages. Part of Deskfeed; needs WebKitGTK 6 (Arch Linux: webkitgtk-6.0).
dep-keyring = Keyring (Secret Service)
dep-keyring-hint = Keeps you signed in between starts. Without it you sign in each time. Install and start a Secret Service such as oo7 or GNOME Keyring.
dep-runtime = User runtime folder
dep-runtime-hint = Private folder for connection files (XDG_RUNTIME_DIR), provided by your login session.

# Accounts
account-menu = Account
add-account = Add account
unnamed-account = Account
loading-accounts = Loading saved accounts…
sign-in-needed = Sign in to see this account's desktops.

# Settings
account-settings = Account settings
account-settings-hint = Defaults for every desktop of this account. A desktop can override them in its own settings.
desktop-settings = Desktop settings
desktop-settings-hint = Settings for this desktop only. "Account default" uses the settings of { $account }.
desktop-gone = This desktop is no longer listed.
settings-connection = Connection
settings-display = Display
settings-devices = Devices
setting-timeout = Time to wait for the desktop
setting-timeout-desc = How long to wait for Windows to answer when connecting. Some desktops take close to a minute.
setting-timeout-desktop-desc = How long to wait for Windows to answer when connecting. The account setting is { $value } s.
seconds = { $value } s
use-account-setting = Use account setting
setting-fullscreen = Full screen
setting-fullscreen-desc = Open the desktop in full screen. Ctrl+Alt+Enter switches between full screen and a window.
setting-all-monitors = Use all monitors
setting-all-monitors-desc = In full screen, span the desktop over all monitors.
setting-new-workspace = Open on a new workspace
setting-new-workspace-desc = Move the desktop window to an empty workspace when it opens.
setting-sound = Sound
setting-sound-desc = Play the desktop's sound on this computer.
setting-microphone = Microphone
setting-microphone-desc = Let the desktop use this computer's microphone.
setting-clipboard = Clipboard
setting-clipboard-desc = Share copy and paste between this computer and the desktop.
follow-account-on = Account default (on)
follow-account-off = Account default (off)
on = On
off = Off
dep-cosmic-windows = COSMIC window management
dep-cosmic-windows-hint = Needed for "Open on a new workspace". Provided by the COSMIC desktop (cosmic-comp 1.0 or newer).
error-sign-in-frequency = Your organization asks you to sign in again after a set time. Sign in to continue.
open-log-folder = Open log folder
error-sign-in-rejected = Windows rejected the sign-in.
