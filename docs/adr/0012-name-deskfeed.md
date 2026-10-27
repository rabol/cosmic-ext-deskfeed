# 0012. The app is called Deskfeed

- Status: Accepted
- Date: 2026-10-17

## Context

The working name was "AVD Client" (`cosmic-ext-avd`). "AVD" is Microsoft's own short form of Azure Virtual Desktop. Microsoft's trademark guidelines (microsoft.com/en-us/legal/intellectualproperty/trademarks, read 2026-10-17) say: "Don't use Microsoft's Brand Assets in the name of your business, product, service, app, domain name", and an app's name "must be unique to you and free of Microsoft's Brand Assets". They allow stating compatibility, for example "Contoso software works with Microsoft OneDrive". The spec also says to avoid Microsoft branding. Nothing had been published yet, so a rename was cheap.

## Decision

- Name: **Deskfeed**; one line: "Your work desktops on COSMIC™, working with Azure Virtual Desktop and Windows 365". Microsoft's product names appear only to state compatibility.
- Technical names: crates and binaries `cosmic-ext-deskfeed` and `cosmic-ext-deskfeed-signin`, app ID `io.github.rabol.cosmic-ext-deskfeed`, repository `rabol/cosmic-ext-deskfeed`; folders, the keyring's application attribute and environment variables follow.
- A quick web search (2026-10-17) found no other app called Deskfeed; it is not a trademark search.
- The contact address stays cosmic-ext-avd@rabol.dev.
- No migration from the old app ID: the only user signs in again.

## Consequences

- Saved sign-ins, settings and folders of the old name are not read; the old ones can be removed by hand (see the release notes of this change).

## Addendum (2026-10-27): app ID `dev.rabol.deskfeed`

- The app ID is now `dev.rabol.deskfeed`, in reverse-DNS form for the owner's domain rabol.dev, and shorter. The crates, binaries, repository and the `cosmic-ext-deskfeed` folders (cache, log, sign-in profiles, runtime) keep their names.
- What follows the ID changes with it: the settings folder `~/.config/cosmic/<app ID>/`, the keyring's application attribute, the autostart entry `~/.config/autostart/<app ID>.desktop`, the installed desktop, metainfo and icon files, the window class, and the single-instance bus name and path (`/dev/rabol/deskfeed`).
- Again no migration: the owner is the only known user and signs in again. Accounts, saved connections, favorites, settings, saved sign-ins and remembered passwords under the old ID are not read. "Start when you log in" has to be turned on again.
- Leftovers of the old ID can be removed by hand: `~/.config/cosmic/io.github.rabol.cosmic-ext-deskfeed/`, `~/.config/autostart/io.github.rabol.cosmic-ext-deskfeed.desktop`, and the keyring items with `application` = `io.github.rabol.cosmic-ext-deskfeed` (for example in Seahorse). A `just install` of the old version is removed with `sudo just appid=io.github.rabol.cosmic-ext-deskfeed uninstall` before installing the new one; the AUR package's upgrade removes its old files itself.
