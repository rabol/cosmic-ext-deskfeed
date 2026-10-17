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
