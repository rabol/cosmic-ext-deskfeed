# Security policy

AVD Client handles work sign-ins, so security reports are welcome and taken seriously.

## Reporting a vulnerability

Please **don't open a public issue**. Report it privately through GitHub: on the repository page, open **Security → Report a vulnerability** (GitHub's private vulnerability reporting). Include what you found, how to reproduce it, and which version or commit you used.

You'll get an answer as soon as possible. Fixes are released before details are made public.

**Never include real credentials, tokens, cookies, or your organization's tenant details** (tenant IDs, host names, user names) in a report. Redact them; synthetic values are enough.

## Supported versions

Only the latest release and the `main` branch get security fixes while the project is in early development (0.x).

## Scope

In scope: this repository's code: the app, the sign-in helper (`cosmic-ext-avd-signin`), how they store and pass credentials, which addresses receive tokens, and the files they write.

Out of scope: vulnerabilities in FreeRDP, WebKitGTK, GTK, libcosmic, the Secret Service implementation, or Microsoft's services. Please report those to their projects; we're happy to help coordinate.

## What the app is designed to guarantee

- Refresh tokens only in the desktop keyring (Secret Service); access tokens only in memory; Windows passwords never stored.
- Passwords and tokens never on a command line, in the environment, or in logs; FreeRDP gets them through a private pipe.
- Tokens are only sent over HTTPS to the reviewed Microsoft endpoints (`src/service/endpoint.rs`).
- The sign-in window allows HTTPS only, no pop-ups, no downloads, and returns only the sign-in result.

A way to break any of these is a vulnerability. The design is described in [specs.md](./specs.md) (§6–§8) and the [ADRs](./docs/adr/README.md).
