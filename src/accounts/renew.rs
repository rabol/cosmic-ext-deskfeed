// SPDX-License-Identifier: MIT

//! Restoring saved sign-ins and renewing tokens (spec §6).

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::{Accounts, NotSaved, account_id};
use crate::auth::attempt::ClientConfig;
use crate::auth::id_token::{self, Identity};
use crate::auth::signin::SignedIn;
use crate::auth::token::{self, Secret, TokenError, TokenSet};
use crate::secrets::{self, Stored};
use crate::service::http::Http;

/// When to renew an account's tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renewal {
    /// When the access token expires within the margin.
    IfNeeded,
    /// Now: the service refused an access token that looked valid
    /// (revoked, or the clocks disagree).
    Now,
}

/// Why tokens couldn't be renewed, or a saved sign-in restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenewError {
    /// Network trouble, or Microsoft's signing keys couldn't be fetched:
    /// the sign-in stays; try again later.
    Network,
    /// The account was signed out after the work was scheduled.
    SignedOut,
    /// Revoked or expired for good (`invalid_grant`).
    Revoked,
    /// The organization asks for a new sign-in after a set time
    /// (`invalid_grant`, AADSTS70043).
    SignInFrequency,
    /// Interaction or consent needed, no refresh token, or a renewed ID
    /// token that is invalid or names another account.
    Refused,
}

impl Accounts {
    /// Signs in with a saved refresh token: the account, and why a rotated
    /// refresh token couldn't be saved, if so. `started`:
    /// [`Self::sign_out_count`] when this was scheduled, so a restore
    /// queued before a sign-out does nothing after it. A revoked sign-in is
    /// deleted.
    pub fn restore(
        &self,
        http: &Http,
        stored: &Stored,
        started: u64,
    ) -> Result<(Arc<SignedIn>, Option<NotSaved>), RenewError> {
        let id = account_id(&stored.identity);
        let busy = self.busy(&id);
        let _busy = busy.lock();
        // Signed out since this was scheduled: no network, no saving.
        if self.sign_out_count(&id) != started {
            return Err(RenewError::SignedOut);
        }
        let result = self.redeem(http, &stored.identity, &stored.refresh_token, started);
        if let Err(RenewError::Revoked | RenewError::SignInFrequency) = result {
            let deleted = secrets::delete(&self.config, &stored.identity);
            crate::app_log!("saved sign-in: deleted (invalid_grant): {deleted:?}");
        }
        result.map(|(account, saved)| (account, saved.err()))
    }

    /// Renews the tokens (see [`Renewal`]) and saves a rotated refresh
    /// token, best effort: if that fails, the next start asks to sign in.
    /// `started`: [`Self::sign_out_count`] when the work was scheduled.
    pub fn renew(
        &self,
        http: &Http,
        account: Arc<SignedIn>,
        when: Renewal,
        started: u64,
    ) -> Result<Arc<SignedIn>, RenewError> {
        let force = when == Renewal::Now;
        if !force && !account.tokens.needs_refresh(Instant::now()) {
            return Ok(account);
        }
        let busy = self.busy(&account_id(&account.identity));
        let _busy = busy.lock();
        // Signed out since this was scheduled: no network, no saving.
        if self.sign_out_count(&account_id(&account.identity)) != started {
            return Err(RenewError::SignedOut);
        }
        // Another renewal may have finished while this one waited for the
        // lock: use its result, or at least its (rotated) refresh token.
        let refused = account.tokens.access_token.expose().to_owned();
        let account = match self.newest(&account) {
            Some(known)
                if !known.tokens.needs_refresh(Instant::now())
                    && (!force || known.tokens.access_token.expose() != refused) =>
            {
                return Ok(known);
            }
            Some(known) => known,
            None => account,
        };
        let Some(refresh_token) = &account.tokens.refresh_token else {
            return Err(RenewError::Refused);
        };
        self.redeem(http, &account.identity, refresh_token, started)
            .map(|(renewed, _)| renewed)
    }

    /// Renews `identity`'s tokens with `refresh_token`, checks a renewed ID
    /// token, saves a rotated refresh token (the result of that save comes
    /// back) and records the tokens as the newest. The caller holds the
    /// account's lock and has checked for a sign-out since `started`.
    fn redeem(
        &self,
        http: &Http,
        identity: &Identity,
        refresh_token: &Secret,
        started: u64,
    ) -> Result<(Arc<SignedIn>, Result<(), NotSaved>), RenewError> {
        let tokens = token::refresh(http, &self.config, refresh_token).map_err(|e| {
            crate::app_log!("renewal failed ({})", token_error_kind(&e));
            match e {
                TokenError::Transient(_) => RenewError::Network,
                // AADSTS70043 "BadTokenDueToSignInFrequency": the
                // organization's Conditional Access asks for a new sign-in
                // after a set time (Microsoft Entra error code reference).
                TokenError::InvalidGrant {
                    aadsts: Some(70043),
                } => RenewError::SignInFrequency,
                TokenError::InvalidGrant { .. } => RenewError::Revoked,
                _ => RenewError::Refused,
            }
        })?;
        let identity = renewed_identity(http, &self.config, &tokens, identity)?;
        let rotated = tokens
            .refresh_token
            .as_ref()
            .is_some_and(|t| t.expose() != refresh_token.expose());
        crate::app_log!(
            "renewed (refresh token {})",
            if rotated { "rotated" } else { "unchanged" }
        );
        let account = Arc::new(SignedIn { identity, tokens });
        let saved = if rotated {
            self.save_while_busy(&account, started)
        } else {
            Ok(())
        };
        self.note_tokens(&account, started);
        Ok((account, saved))
    }
}

/// The identity for renewed tokens. A renewed ID token must name the same
/// account and is checked like any other (signature, issuer, audience,
/// lifetime); without one, the known identity stands.
fn renewed_identity(
    http: &Http,
    config: &ClientConfig,
    tokens: &TokenSet,
    expected: &Identity,
) -> Result<Identity, RenewError> {
    let Some(raw) = tokens.id_token.as_deref() else {
        return Ok(expected.clone());
    };
    let keys = id_token::fetch_keys(http).map_err(|_| RenewError::Network)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    id_token::validate_renewed(raw, &keys, &config.client_id, now, expected).map_err(|e| {
        crate::app_log!("renewed ID token refused: {e:?}");
        RenewError::Refused
    })
}

/// The kind of a token error, for diagnostics (no server text).
fn token_error_kind(e: &TokenError) -> String {
    match e {
        TokenError::Transient(_) => "network".into(),
        TokenError::InvalidGrant { aadsts } => format!("invalid_grant, AADSTS {aadsts:?}"),
        TokenError::InteractionRequired { aadsts } => {
            format!("interaction_required, AADSTS {aadsts:?}")
        }
        TokenError::ConsentRequired { aadsts } => format!("consent_required, AADSTS {aadsts:?}"),
        TokenError::Denied { aadsts } => format!("denied by policy, AADSTS {aadsts:?}"),
        TokenError::Other { error, aadsts } => format!("{error}, AADSTS {aadsts:?}"),
        TokenError::Malformed => "malformed response".into(),
    }
}
