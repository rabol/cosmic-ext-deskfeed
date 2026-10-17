app-title = AVD Client
app-comment = Azure Virtual Desktop and Windows 365 client for the COSMIC™ desktop
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
unofficial-notice = AVD Client is an unofficial community app for Azure Virtual Desktop and Windows 365. It is not a Microsoft product. For compatibility it signs in with Microsoft's public Remote Desktop client identity.

# Resources
loading-desktops = Loading your desktops…
no-desktops = No desktops or apps are assigned to this account.
unnamed-workspace = Workspace
workspace-failed = This workspace couldn't be loaded: { $detail }
kind-desktop = Desktop
kind-remoteapp = App (not supported yet)
kind-other = Unknown resource type (not supported)

# Errors
error-helper-missing = The sign-in window component (cosmic-ext-avd-signin) is not installed.
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
error-freerdp-missing = FreeRDP (sdl-freerdp3) is not installed. Install the freerdp package.
error-launch = The remote desktop client couldn't start ({ $detail }).
error-account-locked = Your account is locked. Contact your IT department.
error-password-expired = Your Windows password has expired or must be changed. Change it, then try again.
error-account-disabled = Your account is disabled. Contact your IT department.
error-auth-failed = Authentication failed. Try signing out and in again.
error-freerdp-crashed = The remote desktop client stopped unexpectedly.
error-session-failed = The connection ended with an error (code { $code }).
