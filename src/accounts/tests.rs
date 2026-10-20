// SPDX-License-Identifier: MIT

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use super::*;
use crate::auth::attempt::ClientConfig;
use crate::auth::id_token::Identity;
use crate::auth::signin::SignedIn;
use crate::auth::token::{Secret, TokenSet};
use crate::secrets::fake::{Answer, FakeKeyring};
use crate::secrets::{self, SecretsError, Stored};
use crate::service::endpoint::EndpointPolicy;
use crate::service::http::BearerToken;
use crate::service::http::Http;
use crate::test_util::token_server;
use std::time::Duration;
use url::Url;

/// Accounts whose token endpoint is a local test server on `port`.
/// Their keyring is this thread's fake one, if a test started it;
/// without one, every keyring call fails as with no Secret Service.
fn accounts(port: u16) -> Accounts {
    let mut config = ClientConfig::public_remote_desktop();
    config.authority = Url::parse(&format!("http://127.0.0.1:{port}/organizations")).unwrap();
    Accounts::new(config)
}

/// A fake keyring holding `token` for `identity`.
fn keyring_with(a: &Accounts, identity: &Identity, token: &str) -> FakeKeyring {
    let keyring = FakeKeyring::start(Answer::Unlock);
    secrets::save(&a.config, identity, &Secret::new(token.into())).unwrap();
    keyring
}

/// The saved refresh tokens.
fn tokens(a: &Accounts) -> Vec<String> {
    secrets::load_all(&a.config)
        .unwrap()
        .iter()
        .map(|s| s.refresh_token.expose().to_owned())
        .collect()
}

/// Remembered passwords: (account object ID, desktop).
fn passwords(keyring: &FakeKeyring) -> Vec<(String, String)> {
    let mut all: Vec<_> = keyring
        .items()
        .into_iter()
        .filter(|i| {
            i.attributes
                .get("type")
                .is_some_and(|t| t == "windows-password")
        })
        .map(|i| {
            (
                i.attributes["account"].clone(),
                i.attributes["desktop"].clone(),
            )
        })
        .collect();
    all.sort();
    all
}

fn http(port: u16) -> Http {
    Http::new(EndpointPolicy::local_test(port))
}

fn stored(identity: &Identity, token: &str) -> Stored {
    Stored {
        identity: identity.clone(),
        refresh_token: Secret::new(token.into()),
    }
}

/// A signed-in account whose access token expires in `valid_for`.
fn signed_in(identity: &Identity, refresh: Option<&str>, valid_for: Duration) -> Arc<SignedIn> {
    Arc::new(SignedIn {
        identity: identity.clone(),
        tokens: TokenSet {
            access_token: BearerToken::new("old-access".into()),
            refresh_token: refresh.map(|t| Secret::new(t.into())),
            id_token: None,
            expires_at: Instant::now() + valid_for,
            expires_wall: SystemTime::now() + valid_for,
        },
    })
}

/// `a.restore`, scheduled now.
fn restore_now(
    a: &Accounts,
    port: u16,
    stored: &Stored,
) -> Result<(Arc<SignedIn>, Option<NotSaved>), RenewError> {
    a.restore(
        &http(port),
        stored,
        a.sign_out_count(&account_id(&stored.identity)),
    )
}

/// `a.renew`, scheduled now.
fn renew_now(
    a: &Accounts,
    port: u16,
    account: Arc<SignedIn>,
    when: Renewal,
) -> Result<Arc<SignedIn>, RenewError> {
    let started = a.sign_out_count(&account_id(&account.identity));
    a.renew(&http(port), account, when, started)
}

const OK_SAME: &str = r#"{"access_token":"new-access","token_type":"Bearer","expires_in":3600,"refresh_token":"rt-old"}"#;
const OK_ROTATED: &str = r#"{"access_token":"new-access","token_type":"Bearer","expires_in":3600,"refresh_token":"rt-new"}"#;
const BAD: &str = "400 Bad Request";

#[test]
fn restore_without_rotation_saves_nothing() {
    let me = identity("00000000-0000-0000-0000-0000000000a1");
    let server = token_server("200 OK", OK_SAME);
    let port = server.port;
    let a = accounts(port);
    let _keyring = FakeKeyring::start(Answer::Unlock);
    match restore_now(&a, port, &stored(&me, "rt-old")) {
        Ok((account, notice)) => {
            assert_eq!(account.identity, me);
            assert!(notice.is_none());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(tokens(&a), Vec::<String>::new());
}

#[test]
fn restore_saves_a_rotated_token() {
    let me = identity("00000000-0000-0000-0000-0000000000a2");
    let server = token_server("200 OK", OK_ROTATED);
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let outcome = restore_now(&a, port, &stored(&me, "rt-old"));
    assert!(matches!(outcome, Ok((_, None))), "{outcome:?}");
    assert_eq!(tokens(&a), ["rt-new"]);
    // The renewal used the saved token.
    assert!(server.sent().contains("refresh_token=rt-old"));
}

#[test]
fn restore_reports_when_the_rotated_token_cant_be_saved() {
    let me = identity("00000000-0000-0000-0000-0000000000a3");
    let server = token_server("200 OK", OK_ROTATED);
    let port = server.port;
    // No keyring.
    match restore_now(&accounts(port), port, &stored(&me, "rt-old")) {
        // Signed in all the same, with a notice.
        Ok((_, Some(NotSaved::Keyring(SecretsError::Unavailable(_))))) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn invalid_saved_sign_in_is_deleted() {
    let me = identity("00000000-0000-0000-0000-0000000000a4");
    let server = token_server(BAD, r#"{"error":"invalid_grant","error_codes":[70008]}"#);
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let outcome = restore_now(&a, port, &stored(&me, "rt-old"));
    assert!(matches!(outcome, Err(RenewError::Revoked)), "{outcome:?}");
    assert_eq!(tokens(&a), Vec::<String>::new());
}

#[test]
fn sign_in_frequency_is_explained() {
    let me = identity("00000000-0000-0000-0000-0000000000a5");
    let server = token_server(BAD, r#"{"error":"invalid_grant","error_codes":[70043]}"#);
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let outcome = restore_now(&a, port, &stored(&me, "rt-old"));
    assert!(
        matches!(outcome, Err(RenewError::SignInFrequency)),
        "{outcome:?}"
    );
    assert_eq!(tokens(&a), Vec::<String>::new());
}

#[test]
fn interaction_required_keeps_the_saved_sign_in() {
    // An interactive sign-in will replace it; nothing is deleted.
    let me = identity("00000000-0000-0000-0000-0000000000a6");
    let server = token_server(
        BAD,
        r#"{"error":"interaction_required","error_codes":[50076]}"#,
    );
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let outcome = restore_now(&a, port, &stored(&me, "rt-old"));
    assert!(matches!(outcome, Err(RenewError::Refused)), "{outcome:?}");
    assert_eq!(tokens(&a), ["rt-old"]);
}

#[test]
fn a_restore_queued_before_sign_out_does_nothing_after_it() {
    let me = identity("00000000-0000-0000-0000-0000000000f1");
    // Port 1: any network attempt would end as Retry, not SignInAgain.
    let a = accounts(1);
    let _keyring = keyring_with(&a, &me, "rt-old");
    // Scheduled, then the sign-out completes before it runs.
    let started = a.sign_out_count(&account_id(&me));
    a.delete(&me).unwrap();
    assert_eq!(tokens(&a), Vec::<String>::new());
    let outcome = a.restore(&http(1), &stored(&me, "rt-old"), started);
    assert!(matches!(outcome, Err(RenewError::SignedOut)), "{outcome:?}");
    assert_eq!(tokens(&a), Vec::<String>::new(), "the sign-in came back");
}

#[test]
fn a_renewal_queued_before_sign_out_saves_nothing_after_it() {
    let me = identity("00000000-0000-0000-0000-0000000000f2");
    // A server that would hand out a rotated token, if asked.
    let server = token_server("200 OK", OK_ROTATED);
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let account = signed_in(&me, Some("rt-old"), Duration::ZERO);
    let started = a.sign_out_count(&account_id(&me));
    a.delete(&me).unwrap();
    for when in [Renewal::IfNeeded, Renewal::Now] {
        let result = a.renew(&http(port), account.clone(), when, started);
        assert!(matches!(result, Err(RenewError::SignedOut)), "{result:?}");
    }
    assert_eq!(
        tokens(&a),
        Vec::<String>::new(),
        "a rotated token was saved"
    );
    assert!(a.newest(&account).is_none(), "its tokens are still known");
}

#[test]
fn a_password_save_queued_before_sign_out_or_forget_writes_nothing() {
    let me = identity("00000000-0000-0000-0000-0000000000f3");
    let id = account_id(&me);
    let pw = Secret::new("hunter2".into());
    let a = accounts(1);
    let keyring = FakeKeyring::start(Answer::Unlock);
    // Normal: saved.
    let ticket = a.password_ticket(&id, "desk-1");
    assert_eq!(a.save_password(&me, "desk-1", "D", &pw, ticket), Ok(true));
    // Saved first, deleted by the sign-out after it: gone.
    a.delete(&me).unwrap();
    assert_eq!(passwords(&keyring), []);
    // Scheduled before the sign-out, run after it: nothing written.
    let other = identity("00000000-0000-0000-0000-0000000000f4");
    let other_id = account_id(&other);
    let ticket = a.password_ticket(&other_id, "desk-1");
    a.delete(&other).unwrap();
    assert_eq!(
        a.save_password(&other, "desk-1", "D", &pw, ticket),
        Ok(false)
    );
    assert_eq!(passwords(&keyring), []);
    // Scheduled before Forget, run after it: nothing written, and
    // another desktop's password is untouched.
    let third = identity("00000000-0000-0000-0000-0000000000f5");
    let third_id = account_id(&third);
    let keep = a.password_ticket(&third_id, "desk-2");
    assert_eq!(a.save_password(&third, "desk-2", "D", &pw, keep), Ok(true));
    let ticket = a.password_ticket(&third_id, "desk-1");
    let forget = a.note_forget(&third_id, "desk-1");
    a.forget_password(&third, "desk-1", forget).unwrap();
    assert_eq!(
        a.save_password(&third, "desk-1", "D", &pw, ticket),
        Ok(false)
    );
    assert_eq!(
        passwords(&keyring),
        [(third.object_id.clone(), "desk-2".to_owned())]
    );
}

#[test]
fn a_late_forget_keeps_a_password_saved_after_it() {
    let me = identity("00000000-0000-0000-0000-0000000000f7");
    let id = account_id(&me);
    let pw = Secret::new("hunter2".into());
    let a = accounts(1);
    let keyring = FakeKeyring::start(Answer::Unlock);
    // Forget asked for (its deletion is held up), then the desktop is
    // connected again with "remember", and that save runs first.
    let forget = a.note_forget(&id, "desk");
    let ticket = a.password_ticket(&id, "desk");
    assert_eq!(a.save_password(&me, "desk", "D", &pw, ticket), Ok(true));
    a.forget_password(&me, "desk", forget).unwrap();
    assert_eq!(
        passwords(&keyring),
        [(me.object_id.clone(), "desk".to_owned())]
    );
    // A Forget asked for after that save does delete it.
    let forget = a.note_forget(&id, "desk");
    a.forget_password(&me, "desk", forget).unwrap();
    assert_eq!(passwords(&keyring), []);
}

#[test]
fn a_cache_save_queued_before_sign_out_writes_nothing() {
    let id = account_id(&identity("00000000-0000-0000-0000-0000000000f6"));
    let a = accounts(1);
    let tmp = tempfile::tempdir().unwrap();
    let store = crate::cache::Store::at(tmp.path().to_path_buf());
    let list = vec![crate::cache::CachedWorkspace {
        name: "Workspace".into(),
        resources: Vec::new(),
    }];
    // Normal: saved.
    a.save_cache(&store, &id, &list, a.sign_out_count(&id))
        .unwrap();
    assert!(store.load(&id, Duration::from_hours(1)).is_some());
    // Scheduled, then the sign-out (its counter and the removal), then
    // the save runs: nothing comes back.
    let started = a.sign_out_count(&id);
    a.note_sign_out(&id);
    store.remove(&id).unwrap();
    a.save_cache(&store, &id, &list, started).unwrap();
    assert!(
        store.load(&id, Duration::from_hours(1)).is_none(),
        "the offline list came back"
    );
}

#[test]
fn network_trouble_offers_retry_and_keeps_the_saved_sign_in() {
    let me = identity("00000000-0000-0000-0000-0000000000a7");
    let server = token_server("503 Service Unavailable", "busy");
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let outcome = restore_now(&a, port, &stored(&me, "rt-old"));
    assert!(matches!(outcome, Err(RenewError::Network)), "{outcome:?}");
    assert_eq!(tokens(&a), ["rt-old"]);
}

#[test]
fn a_renewed_id_token_is_never_taken_unchecked() {
    // Its signature must be checked against Microsoft's keys; when they
    // can't be fetched (here the test server answers only the token
    // request), retry later.
    let me = identity("00000000-0000-0000-0000-0000000000a8");
    let server = token_server(
        "200 OK",
        r#"{"access_token":"a","token_type":"Bearer","refresh_token":"rt-old","id_token":"x.y.z"}"#,
    );
    let port = server.port;
    let outcome = restore_now(&accounts(port), port, &stored(&me, "rt-old"));
    assert!(matches!(outcome, Err(RenewError::Network)), "{outcome:?}");
}

#[test]
fn renewal_is_skipped_while_the_token_is_fresh() {
    let me = identity("00000000-0000-0000-0000-0000000000b1");
    let account = signed_in(&me, Some("rt-old"), Duration::from_secs(3600));
    // Port 1: any request would fail, so success means none was made.
    let renewed = renew_now(&accounts(1), 1, account.clone(), Renewal::IfNeeded).unwrap();
    assert!(Arc::ptr_eq(&renewed, &account));
}

#[test]
fn renewal_counts_time_asleep() {
    // The monotonic clock says an hour is left, but the computer slept
    // through it: the wall clock says it has expired.
    let me = identity("00000000-0000-0000-0000-0000000000b7");
    let server = token_server("200 OK", OK_ROTATED);
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let mut account =
        Arc::into_inner(signed_in(&me, Some("rt-old"), Duration::from_secs(3600))).unwrap();
    account.tokens.expires_wall = SystemTime::now() - Duration::from_hours(5);
    let renewed = renew_now(&a, port, Arc::new(account), Renewal::IfNeeded).unwrap();
    assert_eq!(renewed.tokens.access_token.expose(), "new-access");
}

#[test]
fn forced_renewal_renews_a_fresh_looking_token() {
    let me = identity("00000000-0000-0000-0000-0000000000b8");
    let server = token_server("200 OK", OK_ROTATED);
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let account = signed_in(&me, Some("rt-old"), Duration::from_secs(3600));
    let renewed = renew_now(&a, port, account, Renewal::Now).unwrap();
    assert_eq!(renewed.tokens.access_token.expose(), "new-access");
}

#[test]
fn renewal_saves_a_rotated_token() {
    let me = identity("00000000-0000-0000-0000-0000000000b2");
    let server = token_server("200 OK", OK_ROTATED);
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let account = signed_in(&me, Some("rt-old"), Duration::ZERO);
    let renewed = renew_now(&a, port, account, Renewal::IfNeeded).unwrap();
    assert_eq!(renewed.tokens.access_token.expose(), "new-access");
    assert_eq!(tokens(&a), ["rt-new"]);
}

#[test]
fn renewal_failures_are_explained() {
    let me = identity("00000000-0000-0000-0000-0000000000b3");
    let server = token_server("503 Service Unavailable", "busy");
    let port = server.port;
    let account = signed_in(&me, Some("rt"), Duration::ZERO);
    assert_eq!(
        renew_now(&accounts(port), port, account, Renewal::IfNeeded).unwrap_err(),
        RenewError::Network
    );
    let server = token_server(BAD, r#"{"error":"invalid_grant","error_codes":[70008]}"#);
    let port = server.port;
    let account = signed_in(&me, Some("rt"), Duration::ZERO);
    assert_eq!(
        renew_now(&accounts(port), port, account, Renewal::IfNeeded).unwrap_err(),
        RenewError::Revoked
    );
    let server = token_server(BAD, r#"{"error":"invalid_grant","error_codes":[70043]}"#);
    let port = server.port;
    let account = signed_in(&me, Some("rt"), Duration::ZERO);
    assert_eq!(
        renew_now(&accounts(port), port, account, Renewal::IfNeeded).unwrap_err(),
        RenewError::SignInFrequency
    );
    // No refresh token: nothing to renew with, no request.
    let account = signed_in(&me, None, Duration::ZERO);
    assert_eq!(
        renew_now(&accounts(1), 1, account, Renewal::IfNeeded).unwrap_err(),
        RenewError::Refused
    );
}

#[test]
fn a_second_renewal_reuses_the_first_ones_result() {
    // Two connections start with the same expired copy. The first
    // renews (and rotates the refresh token); the second must not send
    // the old refresh token again: only one renewal request is sent.
    let me = identity("00000000-0000-0000-0000-0000000000b5");
    let server = token_server("200 OK", OK_ROTATED);
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let copy = signed_in(&me, Some("rt-old"), Duration::ZERO);
    let first = renew_now(&a, port, copy.clone(), Renewal::IfNeeded).unwrap();
    let second = renew_now(&a, port, copy, Renewal::IfNeeded).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(server.sent().matches("grant_type=refresh_token").count(), 1);
    assert_eq!(tokens(&a), ["rt-new"]);
}

#[test]
fn signing_out_forgets_the_newest_tokens() {
    let me = identity("00000000-0000-0000-0000-0000000000b6");
    let a = accounts(1);
    let fresh = signed_in(&me, Some("rt"), Duration::from_secs(3600));
    a.signed_in(&fresh, a.sign_out_count(&account_id(&me)));
    let stale = signed_in(&me, Some("rt"), Duration::ZERO);
    assert!(a.newest(&stale).is_some());
    a.note_sign_out(&account_id(&me));
    assert!(a.newest(&stale).is_none());
    // Late work from before the sign-out records nothing.
    a.note_tokens(&fresh, a.sign_out_count(&account_id(&me)) - 1);
    assert!(a.newest(&stale).is_none());
}

#[test]
fn renewal_checks_a_renewed_id_token_too() {
    // Same rule as restoring: the ID token's signature is checked
    // against Microsoft's keys. Here they can't be fetched (the test
    // server answers only the token request), so the renewal is
    // retried later and nothing is saved.
    let me = identity("00000000-0000-0000-0000-0000000000b4");
    let server = token_server(
        "200 OK",
        r#"{"access_token":"a","token_type":"Bearer","refresh_token":"rt-new","id_token":"x.y.z"}"#,
    );
    let port = server.port;
    let a = accounts(port);
    let _keyring = keyring_with(&a, &me, "rt-old");
    let account = signed_in(&me, Some("rt-old"), Duration::ZERO);
    assert!(matches!(
        renew_now(&a, port, account, Renewal::IfNeeded),
        Err(RenewError::Network)
    ));
    assert_eq!(tokens(&a), ["rt-old"]);
}

#[test]
fn work_started_before_a_sign_out_never_saves_afterwards() {
    // Spec §6: late callbacks must not restore the account.
    let me = identity("00000000-0000-0000-0000-0000000000c1");
    let a = accounts(1);
    let _keyring = FakeKeyring::start(Answer::Unlock);
    let account = signed_in(&me, Some("rt-late"), Duration::from_secs(3600));
    let started = a.sign_out_count(&account_id(&me));
    a.delete(&me).unwrap();
    assert_eq!(a.save(&account, started), Ok(()));
    assert_eq!(tokens(&a), Vec::<String>::new());
    // A sign-in after the sign-out saves normally.
    let started = a.sign_out_count(&account_id(&me));
    a.save(&account, started).unwrap();
    assert_eq!(tokens(&a), ["rt-late"]);
}

#[test]
fn sign_out_blocks_late_saves_even_if_the_keyring_fails() {
    let me = identity("00000000-0000-0000-0000-0000000000c2");
    // No keyring.
    let a = accounts(1);
    let before = a.sign_out_count(&account_id(&me));
    assert!(matches!(a.delete(&me), Err(SecretsError::Unavailable(_))));
    assert_eq!(a.sign_out_count(&account_id(&me)), before + 1);
}

#[test]
fn loading_saved_sign_ins() {
    let me = identity("00000000-0000-0000-0000-0000000000d1");
    let a = accounts(1);
    let keyring = keyring_with(&a, &me, "rt");
    let all = a.load_saved().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].identity, me);
    // No keyring any more.
    drop(keyring);
    assert!(matches!(a.load_saved(), Err(SecretsError::Unavailable(_))));
}

fn identity(oid: &str) -> Identity {
    Identity {
        tenant_id: "11111111-2222-3333-4444-555555555555".into(),
        object_id: oid.into(),
        username: Some("anna@example.com".into()),
        display_name: None,
    }
}

#[test]
fn account_ids_are_stable_opaque_and_distinct() {
    let a = account_id(&identity("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"));
    let b = account_id(&identity("ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee"));
    assert_eq!(
        a,
        account_id(&identity("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"))
    );
    assert_ne!(a, b);
    assert_eq!(a.len(), 20);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn sign_outs_are_counted_per_account() {
    let a = accounts(1);
    a.note_sign_out("test-a");
    assert_eq!(a.sign_out_count("test-a"), 1);
    assert_eq!(a.sign_out_count("test-b"), 0);
}
