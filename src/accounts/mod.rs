// SPDX-License-Identifier: MIT

//! Per-account plumbing (spec §6), blocking; call it from `spawn_blocking`:
//! - `profiles`: account IDs' sign-in web view profiles and RAM cookie
//!   folders;
//! - `state`: the accounts' shared state, and saving and deleting their
//!   sign-ins, remembered passwords and offline lists;
//! - `renew`: restoring saved sign-ins and renewing tokens.
//!
//! Errors are typed; the app words them (`app/errors.rs`).

use sha2::Digest;

use crate::auth::id_token::Identity;

mod profiles;
mod renew;
mod state;
#[cfg(test)]
mod tests;

pub use profiles::{
    adopt_profile, clean_unfinished_profiles, new_profile_dir, prepared_profile,
    remove_unused_profile,
};
pub use renew::{RenewError, Renewal};
pub use state::{Accounts, NotSaved, PasswordTicket, shared};

/// Opaque, stable ID for an account: a hash of tenant and object ID, in
/// hex (so never `new-…` or `default`). Used for folder names and config
/// keys, never shown.
pub fn account_id(identity: &Identity) -> String {
    use std::fmt::Write as _;
    let mut h = sha2::Sha256::new();
    for part in [identity.tenant_id.as_str(), identity.object_id.as_str()] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    h.finalize()[..10].iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
