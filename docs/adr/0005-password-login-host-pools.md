# 0005. Password login on host pools without Entra single sign-on

- Status: Accepted, revised 2026-10-17 15:30 (see *Revision: Entra sign-in first*)
- Date: 2026-10-17

## Context

The owner's host pool has no Entra single sign-on (issue #38 case; spec §3, §8). Its `.rdp` file has `enablecredsspsupport:i:1`, `promptcredentialonce:i:1`, `authentication level:i:1`, and no `enablerdsaadauth` or `targetisaadjoined`. The reference app connects with `/gateway:type:arm /sec:nla /u:<UPN> /d:` (empty domain) and lets the SDL client's own dialog ask for the password. The result was `RDSTLS_RESULT_LOGON_FAILURE [0x0000052E]` (wrong user name or password), shown to the user as "nothing happens".

FreeRDP 3.32.2 source (`libfreerdp/core/connection.c`, `redirection.c`, `rdstls.c`):

- The AVD broker answers with a server redirection. If it sets `LB_PASSWORD_IS_PK_ENCRYPTED`, FreeRDP uses RDSTLS to the session host and sends the redirection GUID, the user name, the domain, and the broker-supplied encrypted password.
- If the redirection includes `LB_USERNAME` or `LB_DOMAIN`, those values replace what the client sent.
- `0x52E` comes from the session host's Windows logon, so the user name and domain format, or the password itself, was wrong for that host.

**Upstream AVD behaviour (`libfreerdp/core/gateway/arm.c`, `arm_fill_rdstls`):** the AVD service's connection response includes `redirectedUsername`, and FreeRDP **replaces the client's user name with it**. If no domain was given (setting is NULL), FreeRDP sets the domain to `AzureAD` ("Azure/Entra requires the domain field to be set to 'AzureAD' in most cases. Some setups have been reported to require a different one"). FreeRDP then encrypts the password itself (AES with `redirectedAuthBlob`, then the session host's public key) and sends it over RDSTLS.

The reference app passes `/d:` (empty). FreeRDP stores that as an empty string, not NULL (`settings_getters.c`, `alloc_empty_string`), so the `AzureAD` default never applies and the logon goes out with the service's user name and an **empty domain**. The owner also reports (issue #38) a 32-character password typed by hand because paste did not work.

This explains why the official Windows and macOS clients accept any user name format: the typed name is not what decides the logon account.

Possible causes of the `0x52E`, to be told apart by testing:

1. The empty domain the reference app forces, instead of FreeRDP's `AzureAD` default.
2. A mistyped password (32 characters, no paste).
3. A host-pool or Conditional Access policy that blocks password logon (for example the Azure Windows VM Sign-In app; see Microsoft's Entra-joined session host docs).

## Test (owner's account)

The terminal sign-in in `research/password-login-test.sh` is blocked by Microsoft (see `docs/milestone-1.md`), so use the reference app, which reproduces FreeRDP's upstream default when given `/d:AzureAD` in **Default settings… → Advanced flags**. Type the password carefully. Success confirms the upstream default on a real host; it does not depend on which company it is.

## Decision (proposed, pending the test)

- **Follow FreeRDP's upstream AVD behaviour; don't ask for a user name.** The dialog asks only for the Windows password (paste allowed) and shows the signed-in account it applies to. Deskfeed passes no `/u:` and no `/d:` by default, so FreeRDP uses the service's `redirectedUsername` and the `AzureAD` domain default. This works the same for every company, without guessing a format.
- **Logon setting per resource** ([ADR 0006](0006-build-app-alongside-compatibility-work.md)): *automatic* (no `/d:`, FreeRDP's `AzureAD` default), *empty domain* (`/d:` empty, needed by some setups per FreeRDP #11887), *custom domain*, or *Entra authentication to the session host* (`/sec:aad`, no password; worked for the #11887 reporter without the host pool property). After a logon failure the dialog explains which setting was used and offers the next one; it never switches by itself. The setting that works is remembered. The owner's host pool failed with automatic (pasted password, 2026-10-17 12:13) and empty (typed password).
- **Advanced override, per resource:** an optional domain (or `DOMAIN\name` account) for the setups FreeRDP says need a different domain. Only the domain is offered (logon setting *custom domain*): the account part is useless because FreeRDP replaces the user name with the service's `redirectedUsername`. (A user-name parser for `UPN`, `AzureAD\UPN` and `DOMAIN\name` was written first and removed unused on 2026-10-17.) Never pass an empty domain unless the *empty domain* setting is chosen.
- Never retry automatically with other credentials or domains: failed logons can count toward account lockout, and spec §8 forbids retrying invalid credentials.
- On `0x52E`, show "Wrong password" (or "wrong user name or password" when an override is set), say which account was used, and offer retry.
- The password goes to FreeRDP only through `/args-from:fd:<n>` ([ADR 0004](0004-freerdp-version-and-sign-in-helper.md)), never through `/from-stdin`, the environment, or the command line.
- `ERRCONNECT_LOGON_FAILURE` / RDSTLS `0x52E` shows "Wrong user name or password" with a retry; other `ERRCONNECT_*` account errors (expired, locked, must change password) get their own messages.
- The security mode comes from the `.rdp` properties: `/sec:aad` only with `enablerdsaadauth:i:1`; otherwise NLA. No silent switch between them after a failure.

## Consequences

- The default user name format is based on evidence, not guessed.
- If no format works, the problem is the host-pool policy or credentials, not the client; record that and compare with the official Windows or macOS client.

## Revision: Entra sign-in first (2026-10-17)

The owner's host pool rejected password logon with `0x52E` for every domain setting, also after a password change, but **connected with Entra sign-in (`/sec:aad`)** although its `.rdp` file has no `enablerdsaadauth`. The official Windows and macOS apps never ask the owner for a Windows password either. So the `.rdp` property is not a reliable signal, and "`/sec:aad` only with `enablerdsaadauth:i:1`" above is replaced:

- **Every desktop tries Entra sign-in first**, unless the user picked a password setting for it. Entra sign-in sends no password, so trying it can't count toward an account lockout; the retry rule above (no automatic retries with credentials) still holds.
- If Entra sign-in fails, the app does not retry by itself. The desktop row says it didn't work, and the next Connect opens the password dialog with the reason and the other logon settings.
- The setting that connects is remembered per desktop, as before. The dropdown now labels Entra sign-in as recommended and the AzureAD-domain password logon as "Password (standard)".
