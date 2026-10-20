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

/// Listed, and signed out.
fn signed_out(app: &AppModel, id: &str) -> bool {
    app.account(id)
        .is_some_and(|a| matches!(a.state, AccountState::NeedsSignIn { .. }))
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
    assert_eq!(
        app.sign_out_asking,
        Some(("acct-3a".to_owned(), Leaving::SignOut))
    );
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
    // Owner's decision (2026-10-20): signed out, it stays listed.
    assert!(signed_out(&app, "acct-3a"));
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
    assert!(signed_out(&app, "acct-so"));
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
        assert_eq!(s.clipboard, Some(clipboard::BOTH));
        // A position past the options unsets it.
        edit(&mut app, &Level::App, Edit::Choice(c, app_choice(c, 99)));
        assert_eq!(app.config.app_settings.clipboard, None);
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
        assert_eq!(d["k2"].clipboard, Some(clipboard::TO_DESKTOP));
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

/// The sidebar's last entry: "Sign in" with no account (after signing out
/// of the only one), "Add account" once there is one.
#[test]
fn the_sidebar_offers_sign_in_until_there_is_an_account() {
    let labels = |app: &AppModel| -> Vec<String> {
        app.nav
            .iter()
            .filter_map(|id| app.nav.text(id).map(str::to_owned))
            .collect()
    };
    let mut app = app();
    app.rebuild_nav(&Page::Add);
    assert_eq!(labels(&app), [fl!("sign-in"), fl!("add-connection")]);
    add_account(
        &mut app,
        "acct-n",
        "00000000-0000-0000-0000-0000000000a9",
        "k1",
    );
    app.rebuild_nav(&Page::Add);
    assert_eq!(
        labels(&app),
        [
            "anna@example.com".to_owned(),
            fl!("add-account"),
            fl!("add-connection")
        ]
    );
    // Signed out, it stays (owner's decision, 2026-10-20); removed, it's
    // back to "Sign in".
    let _ = app.update(Message::Account(AccountMessage::SignOut("acct-n".into())));
    assert_eq!(
        labels(&app),
        [
            "anna@example.com".to_owned(),
            fl!("add-account"),
            fl!("add-connection")
        ]
    );
    let _ = app.update(Message::Account(AccountMessage::Remove("acct-n".into())));
    assert_eq!(labels(&app), [fl!("sign-in"), fl!("add-connection")]);
}

mod signed_out_accounts {
    use super::*;

    #[test]
    fn signing_out_keeps_the_account_listed_and_remembered() {
        let mut app = app();
        add_account(
            &mut app,
            "acct-k",
            "00000000-0000-0000-0000-0000000000c1",
            "k1",
        );
        // As a sign-in would have remembered it.
        app.remember_account("acct-k", &app.account("acct-k").unwrap().identity.clone());
        let _ = app.update(Message::Account(AccountMessage::SignOut("acct-k".into())));
        assert!(signed_out(&app, "acct-k"));
        assert!(app.config.accounts.contains_key("acct-k"));
        let a = app.account("acct-k").unwrap();
        assert!(a.cached.is_none() && a.stored.is_none() && a.workspaces().is_empty());
    }

    #[test]
    fn removing_takes_it_off_the_list_with_its_settings() {
        let mut app = app();
        add_account(
            &mut app,
            "acct-r",
            "00000000-0000-0000-0000-0000000000c2",
            "k1",
        );
        app.remember_account("acct-r", &app.account("acct-r").unwrap().identity.clone());
        app.config.account_settings.insert(
            "acct-r".into(),
            crate::settings::ConnectionSettings {
                sound: Some(false),
                ..Default::default()
            },
        );
        let _ = app.update(Message::Account(AccountMessage::Remove("acct-r".into())));
        assert!(app.account("acct-r").is_none());
        assert!(!app.config.accounts.contains_key("acct-r"));
        assert!(!app.config.account_settings.contains_key("acct-r"));
        assert!(
            app.signing_out.contains("acct-r"),
            "cleaned up like a sign-out"
        );
    }

    #[test]
    fn removing_with_a_desktop_open_asks_first() {
        let mut app = app();
        add_account(
            &mut app,
            "acct-ra",
            "00000000-0000-0000-0000-0000000000c3",
            "k1",
        );
        launching(&mut app, "acct-ra", "k1");
        let _ = app.update(Message::Account(AccountMessage::Remove("acct-ra".into())));
        assert_eq!(
            app.sign_out_asking,
            Some(("acct-ra".to_owned(), Leaving::Remove))
        );
        assert!(app.dialog().is_some());
        let _ = app.update(Message::Account(AccountMessage::SignOutConfirm));
        assert!(app.account("acct-ra").is_none());
        assert!(app.desks.launching("k1").is_none_or(|l| l.stop));
    }

    /// Spec §6: late results from before a sign-out never bring the account
    /// back, now that it stays listed.
    #[test]
    fn work_from_before_a_sign_out_never_brings_it_back() {
        let mut app = app();
        let epoch = add_account(
            &mut app,
            "acct-l",
            "00000000-0000-0000-0000-0000000000c4",
            "k1",
        );
        let generation = app.next();
        app.account_mut("acct-l").unwrap().generation = generation;
        let _ = app.update(Message::Account(AccountMessage::SignOut("acct-l".into())));
        let me = signed_in("00000000-0000-0000-0000-0000000000c4");
        let late = [
            AccountMessage::Restored {
                account: "acct-l".into(),
                generation,
                outcome: Ok((me.clone(), None)),
            },
            AccountMessage::SignInFinished {
                account: Some("acct-l".into()),
                generation,
                profile: PathBuf::from("/nonexistent/acct-l"),
                result: Ok(me.clone()),
            },
            AccountMessage::Discovered {
                account: "acct-l".into(),
                epoch,
                generation,
                result: Ok((Vec::new(), me.clone())),
            },
        ];
        for m in late {
            let _ = app.update(Message::Account(m));
            assert!(signed_out(&app, "acct-l"));
        }
    }

    #[test]
    fn sign_in_waits_until_the_sign_out_has_finished() {
        let mut app = app();
        add_account(
            &mut app,
            "acct-w",
            "00000000-0000-0000-0000-0000000000c5",
            "k1",
        );
        let _ = app.update(Message::Account(AccountMessage::SignOut("acct-w".into())));
        let _ = app.update(Message::Account(AccountMessage::SignInAccount(
            "acct-w".into(),
        )));
        let a = app.account("acct-w").unwrap();
        assert!(
            matches!(&a.state, AccountState::NeedsSignIn { error: Some(e) } if *e == fl!("error-still-signing-out")),
            "{:?}",
            matches!(a.state, AccountState::SigningIn { .. })
        );
    }

    #[test]
    fn at_start_signed_out_accounts_are_listed_and_saved_ones_remembered() {
        let mut app = app();
        let away = signed_in("00000000-0000-0000-0000-0000000000c6");
        app.config
            .accounts
            .insert("acct-away".into(), (&away.identity).into());
        let saved = signed_in("00000000-0000-0000-0000-0000000000c7");
        let saved_id = crate::accounts::account_id(&saved.identity);
        let stored = crate::secrets::Stored {
            identity: saved.identity.clone(),
            refresh_token: crate::auth::token::Secret::new("rt".into()),
        };
        let _ = app.update(Message::Account(AccountMessage::SavedLoaded(Ok(vec![
            stored,
        ]))));
        assert!(matches!(
            app.account(&saved_id).unwrap().state,
            AccountState::Restoring
        ));
        assert!(app.config.accounts.contains_key(&saved_id), "remembered");
        assert!(signed_out(&app, "acct-away"));
    }
}

/// Clicking "Add account" (or "Sign in") in the sidebar starts the sign-in,
/// also when its page is already the one shown. (Here the helper isn't
/// installed next to the test, so it ends as "helper missing", without
/// making a profile folder.)
#[test]
fn the_add_entry_starts_the_sign_in_when_clicked() {
    let mut app = app();
    app.rebuild_nav(&Page::Add);
    assert!(app.adding.is_none(), "selecting it alone starts nothing");
    let add = app
        .nav
        .iter()
        .find(|&id| app.nav.data::<Page>(id) == Some(&Page::Add))
        .unwrap();
    let _ = app.on_nav_select(add);
    assert!(app.adding.is_some(), "the sign-in was started");
}

/// Saved plain RDP connections (ADR 0029): the form, the list, the sidebar
/// and removing one. Connecting is tested with the launcher.
mod saved_connections {
    use super::*;

    fn rdp(app: &mut AppModel, m: RdpMessage) {
        let _ = app.update(Message::Rdp(m));
    }

    fn fill(app: &mut AppModel, name: &str, server: &str, user: &str, domain: &str) {
        for (field, text) in [
            (FormField::Name, name),
            (FormField::Server, server),
            (FormField::Username, user),
            (FormField::Domain, domain),
        ] {
            rdp(app, RdpMessage::Input(field, text.into()));
        }
    }

    fn labels(app: &AppModel) -> Vec<String> {
        app.nav
            .iter()
            .filter_map(|id| app.nav.text(id).map(str::to_owned))
            .collect()
    }

    /// Adds one through the form; returns its ID.
    fn add(app: &mut AppModel, name: &str, server: &str) -> String {
        let before: HashSet<String> = app.config.connections.keys().cloned().collect();
        fill(app, name, server, "anna", "");
        rdp(app, RdpMessage::Save);
        let mut new = app
            .config
            .connections
            .keys()
            .filter(|k| !before.contains(*k));
        let id = new.next().expect("saved").clone();
        assert!(new.next().is_none());
        id
    }

    #[test]
    fn added_through_the_form_it_is_saved_listed_and_shown() {
        let mut app = app();
        app.rebuild_nav(&Page::AddConnection);
        let id = add(&mut app, "Office PC", "pc1.example.lan:3390");
        assert!(id.starts_with("rdp-"), "a random ID, never the host");
        let c = &app.config.connections[&id];
        assert_eq!((c.host.as_str(), c.port), ("pc1.example.lan", 3390));
        assert_eq!(c.username, "anna");
        assert_eq!(c.logon, LogonSetting::Automatic);
        let rows: Vec<_> = app.connection_rows.iter().map(|r| &r.title).collect();
        assert_eq!(rows, ["Office PC"]);
        assert_eq!(app.selected_page(), Some(Page::Connections));
        assert_eq!(app.connection_form.server, "", "the form is cleared");
    }

    #[test]
    fn a_bad_server_is_explained_and_nothing_saved() {
        let mut app = app();
        for (server, error) in [
            ("", "error-connection-no-host"),
            ("pc 1", "error-connection-host"),
            ("pc1:0", "error-connection-port"),
            ("anna@pc1", "error-connection-host"),
        ] {
            fill(&mut app, "", server, "anna", "");
            rdp(&mut app, RdpMessage::Save);
            assert!(app.config.connections.is_empty(), "{server:?} not saved");
            let expected = match error {
                "error-connection-no-host" => fl!("error-connection-no-host"),
                "error-connection-host" => fl!("error-connection-host"),
                _ => fl!("error-connection-port"),
            };
            assert_eq!(app.connection_form.error.as_deref(), Some(&*expected));
            assert_eq!(app.connection_form.server, server, "what was typed stays");
        }
    }

    #[test]
    fn editing_prefills_and_keeps_the_id() {
        let mut app = app();
        let id = add(&mut app, "Office PC", "pc1.example.lan");
        let _ = app.update(Message::Settings(SettingsMessage::OpenDesktopSettings(
            id.clone(),
        )));
        let f = &app.connection_form;
        assert_eq!(f.editing.as_deref(), Some(id.as_str()));
        assert_eq!(
            (f.name.as_str(), f.server.as_str(), f.username.as_str()),
            ("Office PC", "pc1.example.lan", "anna")
        );
        assert_eq!(app.selected_page(), Some(Page::AddConnection));
        rdp(
            &mut app,
            RdpMessage::Input(FormField::Domain, "CORP".into()),
        );
        rdp(&mut app, RdpMessage::Save);
        assert_eq!(app.config.connections.len(), 1, "changed, not added");
        assert_eq!(
            app.config.connections[&id].logon,
            LogonSetting::CustomDomain("CORP".into())
        );
    }

    #[test]
    fn the_sidebar_shows_connections_only_when_some_are_saved() {
        let mut app = app();
        app.rebuild_nav(&Page::AddConnection);
        assert!(!labels(&app).contains(&fl!("connections")));
        let id = add(&mut app, "", "pc1");
        assert_eq!(
            labels(&app),
            [fl!("connections"), fl!("sign-in"), fl!("add-connection")]
        );
        let _ = app.update(Message::Settings(SettingsMessage::OpenDesktopSettings(id)));
        rdp(&mut app, RdpMessage::Remove);
        assert_eq!(labels(&app), [fl!("sign-in"), fl!("add-connection")]);
    }

    #[test]
    fn removing_takes_everything_about_it() {
        let mut app = app();
        let keep = add(&mut app, "Other", "pc2");
        let id = add(&mut app, "Office PC", "pc1");
        for key in [&id, &keep] {
            app.config.certificate_pins.insert(key.clone(), "AA".into());
            app.config
                .desktop_settings
                .insert(key.clone(), crate::settings::ConnectionSettings::default());
            app.config
                .logon_settings
                .insert(key.clone(), LogonSetting::Automatic);
            app.config.favorites.insert(key.clone());
            app.config.saved_passwords.insert(key.clone());
        }
        let _ = app.update(Message::Settings(SettingsMessage::OpenDesktopSettings(
            id.clone(),
        )));
        rdp(&mut app, RdpMessage::Remove);
        let c = &app.config;
        assert!(!c.connections.contains_key(&id));
        assert!(!c.certificate_pins.contains_key(&id));
        assert!(!c.desktop_settings.contains_key(&id));
        assert!(!c.logon_settings.contains_key(&id));
        assert!(!c.favorites.contains(&id));
        assert!(!c.saved_passwords.contains(&id));
        // The other one is untouched.
        assert!(c.connections.contains_key(&keep) && c.certificate_pins.contains_key(&keep));
        assert!(c.favorites.contains(&keep) && c.saved_passwords.contains(&keep));
        let rows: Vec<_> = app.connection_rows.iter().map(|r| &r.title).collect();
        assert_eq!(rows, ["Other"]);
    }

    #[test]
    fn a_password_given_is_marked_remembered_never_saved_in_the_settings() {
        let mut app = app();
        fill(&mut app, "", "pc1", "anna", "");
        rdp(
            &mut app,
            RdpMessage::PasswordInput(PasswordText("s3cret!".into())),
        );
        rdp(&mut app, RdpMessage::Save);
        let id = app.config.connections.keys().next().unwrap().clone();
        assert!(app.config.saved_passwords.contains(&id));
        assert_eq!(app.connection_form.password, "");
        let settings = format!("{:?}", app.config);
        assert!(!settings.contains("s3cret!"));
    }

    #[test]
    fn a_connection_removed_elsewhere_closes_its_form() {
        let mut app = app();
        let id = add(&mut app, "", "pc1");
        let _ = app.update(Message::Settings(SettingsMessage::OpenDesktopSettings(id)));
        let mut config = app.config.clone();
        config.connections.clear();
        let _ = app.update(Message::App(AppMessage::UpdateConfig(Box::new(config))));
        assert!(app.connection_form.editing.is_none());
        assert!(app.connection_rows.is_empty());
        rdp(&mut app, RdpMessage::Save);
        assert!(app.config.connections.is_empty(), "not brought back");
    }
}
