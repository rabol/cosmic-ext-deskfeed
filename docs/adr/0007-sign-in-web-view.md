# 0007. Sign-in web view: WebKitGTK 6 in a separate helper process

- Status: Accepted
- Date: 2026-10-17

## Context

With Microsoft's public Remote Desktop client ID, the only usable redirect is `https://login.microsoftonline.com/common/oauth2/nativeclient`. A normal browser can't return it to the app: since at least 2026-10-17 the page warns about phishing and redirects to `/common/wrongplace` after 3 seconds. FreeRDP 3.32.2 also requests tokens with this redirect on every connection (gateway, and session host on single sign-on host pools) through its helper protocol. So a web view that catches the redirect before the page loads is required, whatever the app's own sign-in uses. The loopback-redirect test is therefore not needed for this decision.

Spec §2 and §6 allow one exception to "no GTK / no embedded browser": a separate, sign-in-only web view helper process. [ADR 0003](0003-minimal-dependencies.md) asks for as few dependencies as possible.

Options considered:

| Option | Cost | Notes |
|---|---|---|
| **WebKitGTK 6 (`webkitgtk-6.0`) via the gtk-rs `webkit6` crate** | System library; on the owner's Arch system 6 packages, 39 MiB download (gtk4 already installed). Rust: `gtk4` + `webkit6` crates in the helper only | Maintained, packaged by every major distro, the engine FreeRDP's own webview helper uses on Linux |
| WebKitGTK 4.1 (GTK 3) | Same engine, older GTK | No reason to choose the older stack |
| Qt WebEngine | Qt plus Chromium, far larger | FreeRDP's Qt helper uses it; much heavier |
| `wry` | Wraps WebKitGTK on Linux anyway, plus its own deps | No saving |
| Patch FreeRDP / custom client | Large, ongoing maintenance | Spec §8 allows only if the helper protocol proves insufficient |

## Decision

- Use **WebKitGTK 6** in a separate binary, `cosmic-ext-deskfeed-signin`, a Cargo workspace member. The main app does not link GTK or WebKit.
- The helper follows spec §6: HTTPS only, starts at `login.microsoftonline.com`, shows the current host, blocks pop-ups, downloads and file access, no script bridge, stops at the `nativeclient` redirect, and returns only `code`, `state` and error fields over a private pipe. One cookie store per account in the app's private data directory, deleted on sign-out.
- The same helper serves FreeRDP's `/azure:auth-helper` requests, forwarded by the app ([ADR 0004](0004-freerdp-version-and-sign-in-helper.md)).
- Requires the distro's `webkitgtk-6.0` package (native only, [ADR 0010](0010-native-only-and-dependency-check.md)); the startup dependency check tests that the helper starts.

## Implementation notes (2026-10-17)

- Crate `signin/` (`cosmic-ext-deskfeed-signin`), installed to `libexec/cosmic-ext-deskfeed/`. Uses `gtk4` 0.11 (feature `v4_18`) and `webkit6` 0.6.1 (feature `v2_52`) against system GTK 4.24 and WebKitGTK 2.54.1. Adds 38 packages to `Cargo.lock` (658 → 696); none are linked into the main app.
- Protocol: the app writes one JSON request line (`url`, `title`, `silent_first`, `timeout_ms`) to the helper's stdin and reads one JSON reply line (`ok` with `code` and `state`; `error` with short codes; `cancelled`; `timeout`; `failed`). Closing stdin cancels; the helper then exits, so it never outlives the app.
- The redirect is caught in the navigation policy, before the request is sent, so Microsoft's `nativeclient` warning page never loads.
- `silent_first` keeps the window hidden and shows it only when a page has loaded and sign-in has not finished 1.5 s later, so renewals with valid cookies stay invisible.
- `glib` 0.22 no longer offers a Unix fd watch, so a small thread watches stdin for EOF.

## Consequences

- One large system dependency, isolated in one small process.
- Owner action for native builds: `sudo pacman -S webkitgtk-6.0`.
- If Microsoft's public client ID stops working, the administrator-registration mode (system browser + loopback) still avoids the web view for that mode.
