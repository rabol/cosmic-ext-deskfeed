// SPDX-License-Identifier: MIT

//! The app's own logic, driven through `update` like the real app, without
//! a window. Background work comes back as tasks, which these tests don't
//! run: they deliver the results as messages themselves.

use cosmic::Application as _;
use cosmic::widget::menu::action::MenuAction as _;

#[allow(clippy::wildcard_imports)] // see app/mod.rs
use super::*;
use crate::auth::id_token::Identity;
use crate::auth::token::TokenSet;
use crate::service::feed::ResourceKind;
use crate::service::http::BearerToken;

#[tokio::test]
async fn crashed_work_gives_the_fallback_not_a_normal_result() {
    assert_eq!(off_ui(|| 3, 7).await, 3);
    let crashed: Result<Vec<u8>, String> =
        off_ui(|| panic!("simulated crash"), Err("crashed".into())).await;
    assert_eq!(crashed, Err("crashed".to_owned()));
}

/// The app with no settings store, no offline list, and a network policy
/// that reaches nothing.
fn app() -> AppModel {
    AppModel::new(
        cosmic::Core::default(),
        Config::default(),
        None,
        Http::new(EndpointPolicy::local_test(1)),
        crate::accounts::Accounts::new(crate::auth::attempt::ClientConfig::public_remote_desktop()),
        None,
        false,
    )
}

fn signed_in(oid: &str) -> Arc<SignedIn> {
    Arc::new(SignedIn {
        identity: Identity {
            tenant_id: "11111111-2222-3333-4444-555555555555".into(),
            object_id: oid.into(),
            username: Some("anna@example.com".into()),
            display_name: None,
        },
        tokens: TokenSet {
            access_token: BearerToken::new("access".into()),
            refresh_token: None,
            id_token: None,
            expires_at: Instant::now() + Duration::from_hours(1),
            expires_wall: SystemTime::now() + Duration::from_hours(1),
        },
    })
}

/// Adds a signed-in account `id` with one desktop `key`, whose logon
/// setting asks for a password (so connecting stops at the dialog and
/// never starts FreeRDP). Returns the account's epoch.
fn add_account(app: &mut AppModel, id: &str, oid: &str, key: &str) -> u64 {
    let signed_in = signed_in(oid);
    let desktop = ResourceView {
        key: key.into(),
        title: "Win 11".into(),
        kind: ResourceKind::Desktop,
        connection_file: Some(Url::parse("https://rdweb.wvd.microsoft.com/x.rdp").unwrap()),
        icon: None,
    };
    let state = AccountState::Ready {
        signed_in: signed_in.clone(),
        loading: false,
        workspaces: vec![WorkspaceView {
            name: "Work".into(),
            error: None,
            resources: vec![desktop],
        }],
        error: None,
    };
    app.counter += 1;
    let epoch = app.counter;
    app.accounts.push(Account::new(
        id.into(),
        signed_in.identity.clone(),
        state,
        epoch,
    ));
    app.config
        .logon_settings
        .insert(key.into(), LogonSetting::Automatic);
    epoch
}

/// A connection file as the service would send it.
fn connection_file() -> Arc<ConnectionFile> {
    Arc::new(
        ConnectionFile::parse(
            b"full address:s:rdgateway.example\r\n\
              gatewayhostname:s:rdgateway.wvd.microsoft.com:443\r\n",
        )
        .unwrap(),
    )
}

/// FreeRDP starting for `key` (a desktop that counts as open).
fn launching(app: &mut AppModel, id: &str, key: &str) {
    app.desks.at(key).launching = Some(Launching {
        start: Start {
            attempt: 1,
            account: id.into(),
            title: "Win 11".into(),
            grabs_shortcuts: false,
            keep_awake: false,
            remote_app: false,
        },
        deadline: Duration::from_mins(1),
        stop: false,
    });
}

/// Manual test 3a, now automated: signing out with a desktop open asks
/// first; Cancel keeps everything, and confirming signs out and stops it.
#[test]
fn sign_out_with_a_desktop_open_asks_first() {
    let mut app = app();
    add_account(
        &mut app,
        "acct-3a",
        "00000000-0000-0000-0000-0000000003a0",
        "k1",
    );
    launching(&mut app, "acct-3a", "k1");

    let _ = app.update(Message::Account(AccountMessage::SignOut("acct-3a".into())));
    assert_eq!(app.sign_out_asking.as_deref(), Some("acct-3a"));
    assert!(app.dialog().is_some(), "the sign-out question is shown");
    assert!(app.account("acct-3a").is_some());

    // Cancel keeps everything.
    let _ = app.update(Message::Account(AccountMessage::SignOutCancel));
    assert!(app.sign_out_asking.is_none());
    assert!(app.dialog().is_none());
    assert!(app.account("acct-3a").is_some());
    assert!(app.desks.launching("k1").is_some_and(|l| !l.stop));

    // Disconnect and sign out.
    let _ = app.update(Message::Account(AccountMessage::SignOut("acct-3a".into())));
    let _ = app.update(Message::Account(AccountMessage::SignOutConfirm));
    assert!(app.sign_out_asking.is_none());
    assert!(app.account("acct-3a").is_none());
    assert!(app.signing_out.contains("acct-3a"));
    assert!(
        app.desks.launching("k1").is_none_or(|l| l.stop),
        "the starting desktop is stopped once it has started"
    );
}

/// Without anything open, sign-out doesn't ask.
#[test]
fn sign_out_with_nothing_open_doesnt_ask() {
    let mut app = app();
    add_account(
        &mut app,
        "acct-so",
        "00000000-0000-0000-0000-0000000003a1",
        "k1",
    );
    let _ = app.update(Message::Account(AccountMessage::SignOut("acct-so".into())));
    assert!(app.sign_out_asking.is_none());
    assert!(app.account("acct-so").is_none());
}

/// Manual test 3d, now automated: Cancel while the connection file is on
/// its way. The row goes back to Connect, and the file arriving later
/// starts nothing.
#[test]
fn cancel_while_preparing_starts_nothing_later() {
    let mut app = app();
    let epoch = add_account(
        &mut app,
        "acct-3d",
        "00000000-0000-0000-0000-0000000003d0",
        "k1",
    );
    let _ = app.update(Message::Connection(ConnectionMessage::Connect("k1".into())));
    assert_eq!(app.desks.conn("k1"), Some(&Connection::Preparing));

    let _ = app.update(Message::Connection(ConnectionMessage::Disconnect(
        "k1".into(),
    )));
    assert_eq!(app.desks.conn("k1"), None, "back to Connect");

    let file = Ok((
        connection_file(),
        signed_in("00000000-0000-0000-0000-0000000003d0"),
    ));
    let _ = app.update(Message::Connection(ConnectionMessage::ConnectionFileReady(
        "acct-3d".into(),
        epoch,
        "k1".into(),
        file,
    )));
    assert_eq!(app.desks.conn("k1"), None);
    assert!(app.dialogs.shown().is_none(), "no password dialog");
    assert!(app.desks.launching("k1").is_none());
}

/// Manual test 3d, the other half: Cancel while the password dialog is
/// open closes it, and the row goes back to Connect.
#[test]
fn cancel_while_waiting_for_the_password_closes_the_dialog() {
    let mut app = app();
    let epoch = add_account(
        &mut app,
        "acct-3e",
        "00000000-0000-0000-0000-0000000003d1",
        "k1",
    );
    let _ = app.update(Message::Connection(ConnectionMessage::Connect("k1".into())));
    let file = Ok((
        connection_file(),
        signed_in("00000000-0000-0000-0000-0000000003d1"),
    ));
    let _ = app.update(Message::Connection(ConnectionMessage::ConnectionFileReady(
        "acct-3e".into(),
        epoch,
        "k1".into(),
        file,
    )));
    assert_eq!(app.desks.conn("k1"), Some(&Connection::AwaitingUser));
    assert!(app.dialogs.shown().is_some_and(|d| d.key == "k1"));

    let _ = app.update(Message::Connection(ConnectionMessage::Disconnect(
        "k1".into(),
    )));
    assert_eq!(app.desks.conn("k1"), None, "back to Connect");
    assert!(app.dialogs.shown().is_none(), "the dialog closed");
    assert!(app.desks.launching("k1").is_none());
}

/// Quit from the menu (the message the routing once lost): with a desktop
/// open it asks; Cancel goes back, confirming starts stopping.
#[test]
fn quit_from_the_menu_asks_while_a_desktop_is_open() {
    let mut app = app();
    add_account(
        &mut app,
        "acct-q",
        "00000000-0000-0000-0000-0000000000e1",
        "k1",
    );
    launching(&mut app, "acct-q", "k1");
    let _ = app.update(MenuAction::Quit.message());
    assert_eq!(app.quit, Quit::Asking);
    assert!(app.dialog().is_some(), "the quit question is shown");
    let _ = app.update(Message::Quit(QuitMessage::Cancel));
    assert_eq!(app.quit, Quit::No);
    let _ = app.update(MenuAction::Quit.message());
    let _ = app.update(Message::Quit(QuitMessage::Confirm));
    assert_eq!(app.quit, Quit::Stopping);
}

/// With nothing open, Quit from the menu quits without asking.
#[test]
fn quit_from_the_menu_with_nothing_open_doesnt_ask() {
    let mut app = app();
    let _ = app.update(MenuAction::Quit.message());
    assert_eq!(app.quit, Quit::Stopping);
}
