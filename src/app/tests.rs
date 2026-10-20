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
    let _ = app.update(Message::Connection(
        ConnectionMessage::ConnectionFileReady {
            account: "acct-3d".into(),
            epoch,
            key: "k1".into(),
            result: file,
        },
    ));
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
    let _ = app.update(Message::Connection(
        ConnectionMessage::ConnectionFileReady {
            account: "acct-3e".into(),
            epoch,
            key: "k1".into(),
            result: file,
        },
    ));
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

mod fresh_token {
    use super::*;
    use crate::app::account_flow::with_fresh_token;
    use crate::test_util::token_server;
    use std::sync::Mutex;

    const RENEWED: &str = r#"{"access_token":"new-access","token_type":"Bearer","expires_in":3600,"refresh_token":"rt"}"#;

    /// Accounts whose token endpoint is the test server on `port`.
    fn sign_ins(port: u16) -> crate::accounts::Accounts {
        let mut config = crate::auth::attempt::ClientConfig::public_remote_desktop();
        config.authority = Url::parse(&format!("http://127.0.0.1:{port}/organizations")).unwrap();
        crate::accounts::Accounts::new(config)
    }

    /// A fresh-looking account that can be renewed.
    fn account() -> Arc<SignedIn> {
        let mut a = Arc::into_inner(signed_in("00000000-0000-0000-0000-0000000000f9")).unwrap();
        a.tokens.refresh_token = Some(crate::auth::token::Secret::new("rt".into()));
        Arc::new(a)
    }

    fn status(code: u16) -> FetchError {
        FetchError::Http(HttpError::Status {
            status: code,
            body: Vec::new(),
        })
    }

    type Outcome = Result<(u32, Arc<SignedIn>), StepError>;

    /// Runs `with_fresh_token` with requests that answer `answers` in turn;
    /// returns the result and the access tokens the requests got.
    fn run(port: u16, answers: Vec<Result<u32, FetchError>>) -> (Outcome, Vec<String>) {
        let answers = Mutex::new(answers.into_iter());
        let seen = Mutex::new(Vec::new());
        let result = with_fresh_token(
            &sign_ins(port),
            &Http::new(EndpointPolicy::local_test(port)),
            account(),
            0,
            "test",
            |token| {
                seen.lock().unwrap().push(token.expose().to_owned());
                answers.lock().unwrap().next().expect("no more answers")
            },
        );
        (result, seen.into_inner().unwrap())
    }

    #[test]
    fn a_fresh_token_is_used_as_it_is() {
        let server = token_server("200 OK", RENEWED);
        let (result, seen) = run(server.port, vec![Ok(7)]);
        let (value, signed_in) = result.unwrap();
        assert_eq!(value, 7);
        assert_eq!(signed_in.tokens.access_token.expose(), "access");
        assert_eq!(seen, ["access"]);
        assert_eq!(server.sent(), "", "no renewal");
    }

    #[test]
    fn a_refused_token_is_renewed_once_and_the_request_tried_again() {
        let server = token_server("200 OK", RENEWED);
        let (result, seen) = run(server.port, vec![Err(status(401)), Ok(7)]);
        let (value, signed_in) = result.unwrap();
        assert_eq!(value, 7);
        assert_eq!(signed_in.tokens.access_token.expose(), "new-access");
        assert_eq!(seen, ["access", "new-access"]);
        assert_eq!(server.sent().matches("grant_type=refresh_token").count(), 1);
    }

    #[test]
    fn refused_twice_means_signing_in_again() {
        let server = token_server("200 OK", RENEWED);
        let (result, _) = run(server.port, vec![Err(status(401)), Err(status(401))]);
        assert!(
            matches!(&result, Err(StepError::SignInAgain(t)) if *t == fl!("error-session-expired")),
            "{result:?}"
        );
    }

    #[test]
    fn other_failures_are_shown_without_renewing() {
        let server = token_server("200 OK", RENEWED);
        let (result, seen) = run(server.port, vec![Err(status(403))]);
        assert!(
            matches!(&result, Err(StepError::Show(t)) if *t == fl!("error-forbidden")),
            "{result:?}"
        );
        assert_eq!(seen.len(), 1);
        assert_eq!(server.sent(), "", "no renewal");
    }

    #[test]
    fn a_renewal_that_fails_on_the_network_is_shown() {
        let server = token_server("503 Service Unavailable", "busy");
        let (result, seen) = run(server.port, vec![Err(status(401))]);
        assert!(
            matches!(&result, Err(StepError::Show(t)) if *t == fl!("error-generic")),
            "{result:?}"
        );
        assert_eq!(seen.len(), 1, "not tried again without a new token");
    }
}

mod settings_pages {
    use super::*;
    use crate::app::settings_ui::{app_choice, override_choice, override_toggle};
    use crate::settings::{Choice, Edit, Number, Toggle, clipboard};

    fn settings(app: &mut AppModel, m: SettingsMessage) {
        let _ = app.update(Message::Settings(m));
    }

    /// A change as the page for `level` sends it.
    fn edit(app: &mut AppModel, level: &Level, e: Edit) {
        settings(app, SettingsMessage::Edit(level.clone(), e));
    }

    #[test]
    fn app_settings_are_set_clamped_and_offered_values_only() {
        let mut app = app();
        let c = Choice::Clipboard;
        edit(
            &mut app,
            &Level::App,
            Edit::Toggle(Toggle::Fullscreen, Some(true)),
        );
        edit(
            &mut app,
            &Level::App,
            Edit::Number(Number::Timeout, Some(5)),
        );
        edit(&mut app, &Level::App, Edit::Choice(c, app_choice(c, 1)));
        let s = &app.config.app_settings;
        assert_eq!(s.fullscreen, Some(true));
        assert_eq!(s.timeout_secs, Some(10), "clamped to the smallest");
        assert_eq!(s.clipboard_mode, Some(clipboard::BOTH));
        // A position past the options unsets it.
        edit(&mut app, &Level::App, Edit::Choice(c, app_choice(c, 99)));
        assert_eq!(app.config.app_settings.clipboard_mode, None);
    }

    #[test]
    fn account_overrides_follow_or_override_and_empty_entries_go() {
        let mut app = app();
        let id = "acct-s".to_owned();
        let level = Level::Account(id.clone());
        let sound = |position| Edit::Toggle(Toggle::Sound, override_toggle(position));
        // Dropdown: 0 follows the app, 1 on, 2 off.
        edit(&mut app, &level, sound(2));
        assert_eq!(app.config.account_settings[&id].sound, Some(false));
        edit(&mut app, &level, sound(1));
        assert_eq!(app.config.account_settings[&id].sound, Some(true));
        edit(&mut app, &level, sound(0));
        assert!(
            !app.config.account_settings.contains_key(&id),
            "nothing left"
        );
        // Numbers: a value, then back to following.
        edit(&mut app, &level, Edit::Number(Number::Refresh, Some(30)));
        assert_eq!(app.config.account_settings[&id].refresh_mins, Some(30));
        edit(&mut app, &level, Edit::Number(Number::Refresh, None));
        assert!(!app.config.account_settings.contains_key(&id));
        // Choices: 0 follows, then the options from 1.
        let scale =
            |position| Edit::Choice(Choice::Scale, override_choice(Choice::Scale, position));
        edit(&mut app, &level, scale(2));
        assert_eq!(app.config.account_settings[&id].scale, Some(100));
        edit(&mut app, &level, scale(0));
        assert!(!app.config.account_settings.contains_key(&id));
    }

    #[test]
    fn desktop_overrides_are_kept_per_desktop() {
        let mut app = app();
        let (k1, k2) = (Level::Desktop("k1".into()), Level::Desktop("k2".into()));
        let c = Choice::Clipboard;
        edit(
            &mut app,
            &k1,
            Edit::Toggle(Toggle::Camera, override_toggle(1)),
        );
        edit(&mut app, &k1, Edit::Number(Number::Timeout, Some(10_000)));
        edit(&mut app, &k2, Edit::Choice(c, override_choice(c, 3)));
        let d = &app.config.desktop_settings;
        assert_eq!(d["k1"].camera, Some(true));
        assert_eq!(d["k1"].timeout_secs, Some(600), "clamped to the largest");
        assert_eq!(d["k2"].clipboard_mode, Some(clipboard::TO_DESKTOP));
        assert_eq!(d["k2"].camera, None);
    }

    #[test]
    fn other_app_settings_are_kept() {
        let mut app = app();
        settings(&mut app, SettingsMessage::AppTheme(1));
        settings(&mut app, SettingsMessage::AppBackground(true));
        settings(&mut app, SettingsMessage::AppLogSize(u32::MAX));
        assert_eq!(app.config.app_theme, Some(1));
        assert!(app.config.background);
        assert_eq!(app.config.log_max_mib, Some(crate::log::MAX_SIZE_MIB.1));
        let _ = app.update(Message::App(AppMessage::ToggleFavorite("k1".into())));
        assert!(app.config.favorites.contains("k1"));
    }

    /// Also without a settings store (it couldn't be opened), a logon
    /// choice holds for this run.
    #[test]
    fn a_logon_choice_is_remembered_without_a_settings_store() {
        let mut app = app();
        app.remember_logon("k1", LogonSetting::EmptyDomain);
        assert_eq!(
            app.config.logon_settings.get("k1"),
            Some(&LogonSetting::EmptyDomain)
        );
    }
}
