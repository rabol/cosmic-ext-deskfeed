# 0026. The desktop sign-in uses the app's account

- Status: Accepted. The hint works; the account check is inactive (addendum, 2026-10-20).
- Date: 2026-10-19

## Context

Review finding U1: nothing proved that FreeRDP's own Entra sign-in for the session host uses the account Deskfeed is signed in as.

What FreeRDP 3.32.2 does (read in the source):
- Its authorize URL for the session host (`client/common/client.c`) has `client_id`, `scope`, `redirect_uri`, `state` and PKCE only. There is no `login_hint`, `domain_hint` or `prompt`.
- `/u:` doesn't reach that URL.
- FreeRDP redeems the code itself. Nothing checks who signed in.

The sign-in window uses the account's own cookie profile (ADR 0007), so the right account is the likely one. "Use another account" on Microsoft's page could still pick a different one.

## Decision

The sign-in helper does what Microsoft's own libraries (MSAL) do:

- **Hint:** it adds `login_hint=<the account's user name>` to FreeRDP's authorize URL, unless FreeRDP sent one. This applies to both the silent attempt (`prompt=none`) and the visible sign-in. It pre-selects the account.
- **Check:** it adds `client_info=1`. Entra then names the account in its answer: `client_info` is base64url JSON with `uid` (object ID) and `utid` (tenant ID).
  - An answer naming a different account than the app's (the `oid` and `tid` of the app's verified ID token) is refused. FreeRDP gets the error `wrong_account`, never the code.
  - The helper prints a status line. The launcher turns it into the message "You signed in to the desktop with a different account than this one…".
  - This is never reconnected, and it doesn't count as Entra sign-in failing, so no password logon is offered for it.
- If the silent attempt comes back as another account (the window remembers someone else), the helper doesn't refuse at once. It shows Microsoft's page with `prompt=select_account`, so the user can pick the right account. Only a visible sign-in as another account is refused.
- An answer without `client_info` is let through, and the log says "account not confirmed". Refusing it could block every connection if Entra stopped sending it.

The app passes the account to the helper in two environment variables, `COSMIC_EXT_DESKFEED_SIGNIN_ACCOUNT` (`<oid>.<tid>`) and `COSMIC_EXT_DESKFEED_SIGNIN_LOGIN_HINT`. These are identifiers, not secrets. The app sends the IDs only when they look like GUIDs, and the helper checks their format again. If the helper can't read the account value, it logs "account check off".

Decoding `client_info` needs base64. The helper now depends on `base64` 0.22, which the app already uses, so no new crate enters the tree (`Cargo.lock` gains only the edge).

## Not done (yet)

- **Pinning the tenant** (`/azure:tenantid:<tid>,use-tenantid:on`, so FreeRDP signs in at the tenant instead of `common`): this changes which Entra endpoint FreeRDP talks to. It waits until it can be tried on the real tenant.

## Consequences

- Verified with tests:
  - the URL changes (added once, existing parameters kept);
  - reading `client_info` (valid, padded, upper case, malformed, random input);
  - the refusal reaching FreeRDP as an error without the code;
  - the launcher mapping the helper's line to the new end, ahead of FreeRDP's own logon failure;
  - the environment reaching FreeRDP's children.
- **Not verified yet: that FreeRDP passes the helper's error output on to Deskfeed.** FreeRDP starts the helper (ADR 0004), and the launcher reads FreeRDP's error output. If it isn't passed on, the refusal still happens, but the row shows the general "sign-in rejected" message instead of the new one. Manual test 3e shows which.
- **Not verified yet: the real tenant.** That Entra returns `client_info` on FreeRDP's request, and that its `uid`/`utid` equal the ID token's `oid`/`tid`. Both are MSAL's documented behaviour, and the app signs in through `/organizations`, which is the home tenant. A guest account is the case to watch. If the IDs don't match there, every Entra connection is refused with the new message. That is manual test 3e (docs/manual-tests.md), and the log line "signed in as a different account" would show it.

## Addendum (2026-10-20): the check never runs on the real tenant

Seen on the owner's tenant: every desktop sign-in logs "account not confirmed (no client_info)". Entra returns `client_info` in the token response, when the code is redeemed. It doesn't return it on the authorize redirect, which is all the helper sees, and FreeRDP redeems the code itself. The assumption above was MSAL's behaviour applied to the wrong step, and it wasn't checked against a real redirect before it was written down.

What this means:
- **The hint works.** The user name pre-selects the account on Microsoft's page. This is the part of U1 that is in effect.
- **The check doesn't.** Nothing compares the signed-in account with the app's, so a different account picked with "Use another account" isn't refused. U1 is reduced, not closed.
- **The code stays, inactive** (the owner's choice). It does nothing while Entra sends no `client_info`, and it would act if Entra ever did.

Also verified that day: the helper's status lines reach Deskfeed through FreeRDP's error output. They appear in the app's log. So a refusal would show its own message.

A check that works would have to see the token, and only FreeRDP does. That would need a change in FreeRDP, or Deskfeed redeeming the code itself, which FreeRDP 3.32.2's helper protocol doesn't allow.
