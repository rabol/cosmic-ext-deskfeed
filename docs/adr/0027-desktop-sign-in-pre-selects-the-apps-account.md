# 0027. The desktop's sign-in pre-selects the app's account

- Status: Accepted. Supersedes [0026](0026-desktop-sign-in-uses-the-apps-account.md).
- Date: 2026-10-20

## Context

Review finding U1: nothing proved that FreeRDP's own Entra sign-in for the session host uses the account Deskfeed is signed in as.

What FreeRDP 3.32.2 does (read in the source):
- Its authorize URL for the session host (`client/common/client.c`) has `client_id`, `scope`, `redirect_uri`, `state` and PKCE only. There is no `login_hint`, `domain_hint` or `prompt`.
- `/u:` doesn't reach that URL.
- FreeRDP redeems the code itself. Nothing checks who signed in.

The sign-in window uses the account's own cookie profile (ADR 0007), so the right account is the likely one. "Use another account" on Microsoft's page could still pick a different one.

ADR 0026 also tried to check the account, with `client_info=1`. That didn't work: on the owner's tenant (2026-10-20), Entra returned `client_info` only in the token response, when the code is redeemed, and not on the authorize redirect, which is all the helper sees. The check never ran, and was removed (owner's decision).

## Decision

- The sign-in helper adds `login_hint=<the account's user name>` to FreeRDP's authorize URL, unless FreeRDP sent one, as Microsoft's own libraries (MSAL) do. This applies to the silent attempt (`prompt=none`) and to the visible sign-in. It pre-selects the account on Microsoft's page (`signin/src/login_hint.rs`).
- The app passes the user name in `COSMIC_EXT_DESKFEED_SIGNIN_LOGIN_HINT` (`common::helper::LOGIN_HINT_ENV`), plain text only. It is an identifier, not a secret.
- Nothing checks which account finally signs in: only FreeRDP sees the token.

## Not done

- **Checking the account.** That would need the token, so either a change in FreeRDP or Deskfeed redeeming the code itself, which FreeRDP 3.32.2's helper protocol doesn't allow.
- **Pinning the tenant** (`/azure:tenantid:<tid>,use-tenantid:on`, so FreeRDP signs in at the tenant instead of `common`): this changes which Entra endpoint FreeRDP talks to. It waits until it can be tried on the real tenant.

## Consequences

- Review finding U1 is reduced, not closed.
- Verified: the hint pre-selects the account on the owner's tenant; the helper's status lines reach Deskfeed through FreeRDP's error output. Tested: the URL changes (added once, existing parameters kept) and the variable reaching FreeRDP's children.
