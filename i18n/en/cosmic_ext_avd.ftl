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
connect-not-ready = Connecting isn't built yet. Signing in and listing your desktops work; connecting is the next step.

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
