# Cleanup to-do: over-engineering and AI-written patterns

A review of the whole codebase, done 2026-10-20 at the owner's request, without changing any code. Four reviewers each read one area:
- the app's screens and logic (`src/app/`);
- sign-in, accounts, network and storage;
- FreeRDP sessions, settings and the COSMIC integrations;
- the sign-in helper, the small crates, the tooling and the docs.

Their findings were checked before going on this list. Items marked **✓ checked** were confirmed again by hand. Two findings were dropped as wrong:
- "the external review isn't in the repo": it is, at `CODE_REVIEW.md`;
- "the roxmltree comment names the wrong version": it names both.

Each item gives where, what is wrong, the simpler alternative, effort (S/M/L) and risk.

**Overall impression.** The core logic is careful and mostly correct:
- URL and redirect policy, ID-token checks, feed parsing, connection-file rules and FreeRDP argument handling.
- Six FreeRDP source citations in the comments were spot-checked, and all hold.
- The unsafe/libc code is needed: pty, pipe fds and the death signal.

The AI fingerprint is **accretion**. Each fix added a flag, wrapper, global map, comment or test instead of reshaping the code. It shows most in:
- copy-pasted blocks;
- parallel lists that must be kept in step by hand;
- comments that cite review IDs and dates;
- process documents in the repo.

## 1. Bugs found along the way (fix first)

- [ ] **An unidentified test failed once** (2026-10-20, one run of the full suite during the keyring clean-up; 21 runs afterwards, also under CPU load, all passed). Its name wasn't captured; the gate now keeps the full test output, so the next failure names it.
- [x] **A crashed dependency check counts as "everything present"** (fixed 2026-10-20: `required_ok` needs every required dependency checked and OK; a crashed check reports each one as unusable) ✓ checked. `src/app/mod.rs:890-892`: `spawn_blocking(deps::check_all)…unwrap_or_default()` gives an empty list on a panic, and `deps::required_ok(&[])` is `true`. *Fix:* treat a failed check as failed, with its own message. Effort S, risk low.
- [x] **The fuzz setup no longer compiles** (fixed 2026-10-20: the target uses `to_freerdp_text_with`, clipboard off and on; compiles, not run, since the nightly toolchain is gone; removing `fuzz/` stays in §6 for the owner) ✓ checked. `fuzz/fuzz_targets/rdpfile.rs:16` calls `to_freerdp_text`, which became test-only in the clipboard fix (`9cd5e5c`). CI doesn't build `fuzz/`, so nothing noticed. *Fix:* see §6, remove `fuzz/`. Effort S, risk low.
- [x] **Dependency-check output over ~64 KiB would hang until the timeout.** (Fixed 2026-10-20, with a test that failed before.) `src/deps.rs:183-209`: stdout is read only after the child exits, but the limit (`MAX_OUTPUT`) is 256 KiB. Harmless for FreeRDP's `/version`. *Fix:* read on a thread, or cap below the pipe size. Effort S, risk low.

## 2. Structure: biggest wins

- [ ] **Message routing ceremony** (`src/app/mod.rs` `route`, the four handlers' `misrouted` fallbacks, `src/app/routing_check.rs`).
  - About 95 messages are listed twice: once in `route` and once in each handler, plus a run-time "wrong handler" fallback.
  - The gap caused the Quit bug, and a 155-line source-parsing test was added to cover it. That test depends on exact formatting.
  - *Simpler:* nested message enums (`Message::Account(…)`, `Connection(…)`, `Settings(…)`, `Quit(…)`), so the compiler enforces the routing. `route`, `misrouted` and `routing_check.rs` then go.
  - Effort M–L (about 280 `Message::` sites), risk med.
- [ ] **The app's state can't be built in a test** (`AppModel` holds libcosmic's `Core`). So flows such as cancelling a desktop before it starts and the sign-out question stay manual tests (3a, 3d), although only the app's own logic decides them. *Simpler:* keep the state (desks, accounts, dialogs, config) in a struct without COSMIC types that `AppModel` owns, with the update logic as methods on it; tests then drive messages through it. Goes well with the routing item above. Effort L, risk med.
- [ ] **Account state in six global maps** ✓ checked (`src/accounts.rs:184-438`: `ACCOUNT_LOCKS`, `KEYRING`, `SIGN_OUTS`, `LATEST`, `FORGETS`, `SAVED_AFTER`).
  - Each has its own lock, plus a `PasswordTicket` generation scheme.
  - Because the state is global, tests need unique account IDs so they don't interfere.
  - *Simpler:* one `Mutex<HashMap<account, AccountState>>`, ideally a value owned by the app.
  - Effort M, risk med (sign-out race fixes live here; keep their tests).
- [ ] **Twin functions for testing in `src/accounts.rs`.**
  - Pairs: `save`/`save_locked`, `delete`/`delete_in`, `save_password`/`_in`, `forget_password`/`_in`, `load_saved`/`_in`.
  - Triples and more: `restore`/`restore_in`/`restore_at`, `renew_if_needed`/`renew_now`/`renew_in`/`renew_at`.
  - A `Store` trait with one real implementation.
  - *Simpler:* pass the environment from the caller (or make it a struct with methods), and move the keyring lock into `secrets`.
  - Effort M, risk low.
- [ ] **Restore and renew repeat one pipeline** (`src/accounts.rs:333-789`).
  - Both do lock, sign-out check, refresh, error mapping (AADSTS 70043 handled twice, with different texts), identity check, save and note.
  - `Restore` and `RenewError` have the same shape.
  - *Simpler:* one `renew_tokens(…)`, with restore adding "delete on invalid_grant".
  - Effort M, risk med.
- [ ] **`src/accounts.rs` (1,409 lines) mixes concerns.**
  - It holds profile folders, concurrency bookkeeping, keyring orchestration, token renewal, and user-facing text (`fl!` returned as `Err(String)`).
  - *Simpler:* split into `accounts/{profiles,state,renew}.rs`, return typed errors and let `app/errors.rs` word them.
  - Effort M, risk low.
- [ ] **Settings listed in ~7 places** (`src/settings.rs`).
  - Each setting appears in: `ConnectionSettings` fields, `Effective` fields, `DEFAULTS`, `toggle`/`set_toggle`, `Effective::toggle`, `with()` and `ALL`.
  - The Toggle/Number/Choice enums themselves are justified (the settings pages are built from them).
  - *Simpler:* one `macro_rules!` that declares each setting once.
  - Smaller items:
    - 19 redundant field-level `#[serde(default)]`;
    - the clipboard as a `u32` with a constants module instead of an enum;
    - a doc comment (line 37) naming a `ClipboardMode` type that doesn't exist ✓ checked.
  - Effort M, risk med (the saved format must stay the same).
- [ ] **The helper/app protocol is defined twice.**
  - `Reply` is in both `src/auth/signin.rs:148-165` and `signin/src/protocol.rs`.
  - The request is ad-hoc `json!` in the app, but typed in the helper.
  - Environment-variable names and `JAR_DIR` are duplicated (`PROXY_FILE_ENV`, `…_SIGNIN_PROFILE`, `…_SIGNIN_TITLE` as literals).
  - The wrong-account marker is kept equal only by a test.
  - *Simpler:* one shared crate (turn `lines/` into a common crate holding bounded lines, the protocol types and the constants).
  - Effort M, risk low.
- [x] **Password dialogs as parallel state with generics for one type.** (Done 2026-10-20: `DialogQueue` with `offer`, `withdraw`, `close`; still generic, so its test can use stand-ins.)
  - Where: `dialog` + `waiting_dialogs` (`src/app/mod.rs`), and `offer_dialog<T>`/`withdraw_dialogs<T>` (`src/app/decide.rs:236-262`).
  - `withdraw_dialogs(|_| false)` really means "show the next one".
  - *Simpler:* a small `DialogQueue` with `offer`, `withdraw` and `advance`.
  - Effort S–M, risk low.
- [x] **How a session ended is decided in two places.** (Done 2026-10-20: `Log::end`, tested without threads; `classify_exit` and its tests unchanged.) `supervise`'s guard chain (`src/sessions/launcher/process.rs:325-337`) and `classify_exit` (`output.rs`); "Replaced" is derived in both. *Simpler:* one `Log::end(status)` holding the whole order, testable without threads. Effort S, risk low–med (order matters).

## 3. Duplication and needless pieces (cheap)

- [x] **Wayland setup copy-pasted 5 times** (Done 2026-10-20: `connect()`, `no_wayland`; `with_window` folded into `focus`) (`src/workspace.rs:128-152, 228-250, 327-347` and tests); `map_err(NoWayland)` about 10 times. *Simpler:* `fn connect()` + `From` for the error. Effort S, risk low.
- [x] **`Running` and `Launching` share 6 fields** (Done 2026-10-20: both hold a `Start`) (`src/app/model.rs:124-160`). *Simpler:* `Running { start: Launching, session }`. Effort S, risk low.
- [ ] **"Renew, try, renew again on 401" written twice** (`src/app/account_flow.rs:552-575`, `src/app/connect.rs:~236-255`). *Simpler:* one helper taking a closure. Effort M, risk med.
- [x] **About 24 `spawn_blocking` blocks with four different fallbacks** (Done 2026-10-20: all 26 go through `off_ui`, which logs a crash and returns an explicit fallback) (`"task"`, `error-generic`, `Err(())`, `unwrap_or_default`). The §1 bug is one of them. *Simpler:* one helper with an explicit error. Effort S–M, risk low.
- [ ] **Settings plumbing** (`src/app/settings_ui.rs`).
  - Nine near-identical setting messages (App/Account/Desktop × Toggle/Number/Choice).
  - `account_settings` is an alias of `inherited`.
  - `view()` only wraps `view_with`.
  - The "`config_handler` Some → save, None → assign" match is repeated ~8 times, and two copies skip the in-memory update.
  - *Simpler:* one `EditSetting { level, key, edit }` message and one `save_config` helper.
  - Effort M, risk low.
- [x] **Enums written as structs of Options.** (Done 2026-10-20: `Adding` is `SigningIn | Failed`; `quit_chosen` is an argument of `request_quit`.) `Adding { cancel: Option, error: Option }` is really an enum. `quit_chosen` is a field used to pass an argument to a recursive call; use `request_quit(chosen)`. Effort S, risk low.
- [x] **Connection-state knowledge spread around.** (Done 2026-10-20: `Connection::progress_label` and `is_starting`; the row-note rule is a different question and stays.) The status label is mapped twice (`views.rs`, `background.rs`), and "is starting" is listed three times. *Simpler:* `Connection::label()` and `is_starting()`. Effort S, risk low.
- [ ] **Positional tuple messages** (for example `Launched(String, u64, String, u64, Result)`; account ID and resource key are both bare `String`). *Simpler:* struct variants, or a shared `Origin { account, epoch, key }`. Effort M, risk low.
- [x] **Label lists cached in the model** (Kept, 2026-10-20: every settings row shares them and the view is rebuilt often; built once, they are borrowed, not copied per row.) (`logon_labels`, `follow_app_on/off`, `follow_account_on/off`); they can be built in `view`. Effort S, risk low.
- [x] **Small duplicate helpers.** (Done 2026-10-20: `is_guid`, the private folder and the title rule are one each. Kept on purpose: `BearerToken` and `Secret` stay two types, so a password can never be passed where a bearer token is expected; `account_id` (hex) and `hash_key` (base64) produce keys already saved in the settings, so merging them would need a migration for no gain.)
  - `BearerToken` and `Secret` are the same redacting newtype.
  - `is_guid` is defined twice ✓ checked (`log.rs:223`, `id_token.rs:213`).
  - `account_id` repeats `hash_key`.
  - The "private folder 0700" code appears three times (`log.rs`, `cache.rs`, `accounts.rs`).
  - Window-title cleanup appears three times in the helper and once in the launcher.
  - Effort S, risk low.
- [x] **Keyring code copy-paste** (Done 2026-10-20, after adding tests on a fake keyring) (`src/secrets.rs:303-420`): `save`/`save_password` repeat CreateItem; `read`/`read_secret` both call GetSecret; three delete loops are the same. *Simpler:* `create_item` and `delete_matching`. Effort S, risk low.
- [x] **A round trip through a URL** (Kept, 2026-10-20: it sends the helper's answer through exactly the same checks as any callback — state, single use, length, duplicate parameters; one validation path is safer than two.) (`src/auth/signin.rs:319-359`): the helper's JSON reply is turned back into a URL only so `Attempt::complete` can parse it again. Effort S, risk low.
- [x] **Errors as magic strings.** (Done 2026-10-20: `HelperProblem`; getrandom failure is `SignInError::NoRandom`.) `HelperFailed(String)` with `"random"`, `"proxy"`, `"no reply"`… is matched by string in `app/errors.rs`. *Simpler:* own variants. Effort S, risk low.
- [x] **Hand-written string conversion for the cache's resource kind** (Done 2026-10-20; same file format, tested) (`cache.rs`, `model.rs`). *Simpler:* serde `rename_all = "lowercase"` on the enum. Effort S, risk low.
- [x] **The signing-keys address is hard-coded** (Done 2026-10-20) ✓ checked (`id_token.rs:21` `JWKS_URL`), unlike the other endpoints, which come from `EndpointPolicy`. Two tests pass only because the test policy refuses that address. Effort S, risk low.
- [x] **Tray events mirror tray actions one to one** (Kept, 2026-10-20: a Show comes from the icon and from the menu and must carry COSMIC's activation token either way, so the event needs its own Show; the two menu walkers number lines in the same depth-first order, and a test pins the numbering, including submenus and disabled lines.) (`src/tray.rs` `TrayEvent` vs `TrayAction`, plus `Menu::clicked`). Two walkers must number menu IDs identically. *Simpler:* `TrayEvent::Action(TrayAction)` and one flat list built in `set_menu`. Effort S–M, risk low.
- [ ] **Polling instead of glib fd watches** in the helper.
  - `freerdp.rs:214-253` uses a reader thread, a channel and a 50 ms poll; `main.rs:138-156` uses a thread, a flag and a 200 ms poll.
  - `Arc<Mutex<File>>` is used although everything runs on one loop.
  - *Simpler:* `glib::unix_fd_add_local`.
  - Effort M, risk med (re-run the hostile-input tests).
- [ ] **Sign-in cancellation via raw PIDs** (`src/auth/signin.rs:65-130, 216-303`): Condvar, PID in a Mutex, `waitid(WNOWAIT)` and unsafe `kill`. Keeping the `Child` in the shared mutex may remove the unsafe code. Uncertain; try it. Effort M, risk med.
- [x] **Small wrappers.** (Done 2026-10-20: `open_sessions` and `now_label` inlined, `clock_label` reuses `date_time_label`, `Account::new`. Kept: `retain_from_tray(|_, _| false)` (two uses, clear enough); `with_window` goes with the workspace item.)
  - `open_sessions` and `diagnostics::now_label`;
  - `retain_from_tray(|_,_| false)` used three times (should be `clear_from_tray`);
  - `clock_label` re-implements `date_time_label`;
  - the 13-field `Account { … }` literal is written twice;
  - `with_window` in `workspace.rs` has one caller.
  - Effort S, risk low.

## 4. Things that never or barely do anything

- [ ] **The `client_info` account check never runs** (ADR 0026 addendum; you chose to keep it for now).
  - It spans `signin/src/account.rs`, `vet`/`AfterSilent::PickAccount`/`WRONG_ACCOUNT_LINE` in `freerdp.rs`, the `account` field in `protocol.rs`, and on the app side `ACCOUNT_ENV`, the marker, `SessionEnd::WrongAccount`, a message and tests.
  - Every desktop sign-in logs "account not confirmed (no client_info)". It is the helper's only reason to depend on `base64`.
  - *If removed:* keep `login_hint`, drop the rest.
  - Effort M, risk low. Your decision.
- [x] **Password "wipe" that only clears one copy** (Done 2026-10-20: removed with its `unsafe`; neither the spec nor the review asks for wiping memory.) ✓ checked. `process.rs:261-263` zeroes one buffer while the same password stays in `args: Vec<String>` and in the `Secret`. A write just before a drop can also be optimised away. *Either* drop it, *or* clear every copy properly. Effort S, risk low.
- [x] **A test-only retry loop in production code** (Kept, 2026-10-20: the suggested rename doesn't close the race, since a process another thread forks still inherits the open file until it execs; the retry also helps while a package update replaces FreeRDP.) ✓ checked. `spawn_retrying` (`process.rs:225-236`) retries "file busy" (ETXTBSY) 20 times, which only happens with freshly written test scripts. *Simpler:* the test writes the script and renames it into place. Effort S, risk low.
- [ ] **Legacy resource-key migration** (`legacy_key`, `migrate_resource_keys`, `adopt_legacy_profile`). Added on the project's first day, before any release, so it only migrates your own development settings; it still runs on every refresh. Effort S, risk low (at worst, settings from before 2026-10-17 are lost).
- [ ] **Legacy `clipboard: Option<bool>` and its migration** (`src/settings.rs:33-38`, `choice`/`set_choice`): the field existed for about a day before any release. Check your own config first. Effort S, risk low.
- [x] **Small dead pieces.** (Done 2026-10-20: the helper no longer parses FreeRDP's title; the test-only helpers moved into the tests. Kept: `dropped`, which the tests use to check exactly which properties were dropped; and the single "bad_request", since only Deskfeed sends requests, so any refusal is a Deskfeed bug.)
  - The helper parses and cleans `title`, then ignores it ✓ checked (`freerdp.rs:336` `title: _`).
  - `ConnectionFile::dropped` is counted but only read by tests.
  - `main.rs:132` throws away `RequestError`'s three variants with `.ok()`.
  - `#[cfg(test)]` helpers sit in production code (`protocol.rs:16, 132`).
  - Effort S, risk low.

## 5. Comments

- [x] **Review IDs and dates in code comments.** (Done 2026-10-20: provenance removed, reasons kept; dates kept only where they date an observation of an external system.)
  - 42 references to CODE_REVIEW.md items in `.rs` files ✓ checked (for example "(review F09)", "(test audit 2026-10-20)").
  - About 30 diary lines ("found 2026-10-20", "was untested", "Before, each fact had its own map… Now…").
  - *Fix:* keep the *why*, drop the provenance, or point to an ADR.
  - Effort S–M, risk none.
- [x] **Stale or stacked doc comments.** (Done 2026-10-20.)
  - `model.rs:465-470`: three docs on `to_cached`, two of which describe `resource_key` and contradict each other ✓ checked.
  - Message variants with an old doc above the new one: `SignOutFinished`, `PasswordSaved`, `WorkspaceMoved`.
  - `mod.rs:151-152`: leftover from a deleted constant.
  - Handler docs say "routed by `update`" (it's `route`).
  - `main.rs:32`: says quitting is in `connect.rs` (it's `quit.rs`).
  - `signin/src/view.rs:64-72` ✓ checked: `private_dir`'s doc sits on `APP_ID`, and it contradicts itself ("Creates `dir`" / "Never creates one").
  - `discovery.rs:55-58`: two merged paragraphs.
  - `attempt.rs:155`: "the web view uses this" (it doesn't).
  - `secrets.rs:260`: "stderr" (it's the app log).
  - `output.rs:13-15`: two stacked summaries.
  - `launcher/mod.rs:46`: "version 3.32.2 either way" (it's a minimum).
  - `session_jar.rs:25`: points to the wrong file for `JAR_DIR`.
  - Effort S, risk none.

## 6. Tests, tooling and docs

- [ ] **Remove `fuzz/`** (broken, see §1; nightly-only; ADR 0023 is titled "random-input tests *instead of* cargo-fuzz"). Also drop `just fuzz`, its CONTRIBUTING and .gitignore lines, and the ADR addendum. For a deep run, raise `DESKFEED_RANDOM_ROUNDS`. Effort S, risk low.
- [ ] **Launcher test boilerplate** (`src/sessions/launcher/tests.rs:523-830`).
  - Six tests repeat the same setup: temp dir, `#!/bin/sh` fake, chmod, channel, launch, wait, clean up. Temp dirs leak when a test fails.
  - The `/proc` zombie check is written twice.
  - `start_without_gateway` re-implements `launch_with` step by step and can drift.
  - *Simpler:* `fake_freerdp(script)`, `launch_fake(spec, script)`, `is_gone(pid)`.
  - Effort M, risk low.
- [ ] **Four hand-written fake HTTP servers and ~14 temp-dir naming schemes.** *Simpler:* one configurable test server in `test_util`, and `tempfile` (already in Cargo.lock) as a dev-dependency (an ADR line per the minimal-dependencies rule). Effort S, risk low.
- [ ] **Two tiny workspace crates** (`lines/`, `testkit/`): sharing code with the helper is a real need, but one crate is enough. Merge them into the common crate from §2, with the random-input tool behind a feature. Effort S, risk low.
- [x] **`justfile`.** (Done 2026-10-20: `just ci`; `check` as CI; comments on the right recipes; `check-json` gone; `tag` bumps only the app and the helper, tags `v<version>` as the AUR package expects, no interactive amend.)
  - A doc comment sits on the wrong recipe.
  - `just check` doesn't match CI.
  - There's no single command that runs CI's gates.
  - Template leftovers: `check-json`, and `tag`, which uses an interactive `--amend` and would also bump the helper crates' versions.
  - *Fix:* `just ci` mirroring `ci.yml`, and drop the leftovers.
  - Effort S, risk low.
- [ ] **The rust-analyzer step in CI** (ADR 0025): a pinned download kept current by hand, on top of rustc and strict clippy. Its one known catch (the `#[path]` include) would go with the shared crate. Uncertain value. Effort S, risk low.
- [ ] **Documentation sprawl.** *Partly done 2026-10-20:* the roadmap lists only open items; `milestone-1.md` is replaced by `compatibility.md` (the matrix and findings); ADR 0004 and 0005 are marked Accepted in the index, ADR 0006 Historical. Still open: addenda that reverse a decision (0023, 0026) and `CODE_REVIEW.md` in the repo root.
  - 26 ADRs written in 4 days, several with addenda that reverse the decision (0023, 0026).
  - `docs/milestone-1.md` is a timestamped lab diary.
  - `docs/roadmap.md` is mostly struck-through "done" items that repeat `CHANGELOG.md`.
  - `CODE_REVIEW.md` (43 KB) is a one-time report sitting in the repo root.
  - *Simpler:*
    - the roadmap lists only open items;
    - fold milestone-1's findings into the ADRs;
    - supersede ADRs instead of stacking addenda;
    - move the review to `docs/reviews/` as historical.
  - Effort M, risk low.

## Areas found clean

Nothing to do here:
- `display.rs` (XSETTINGS parser);
- `single.rs`;
- `awake.rs`;
- the connection-file rule table;
- the death-signal spawner thread;
- `attempt.rs`, `id_token.rs`, `feed.rs` and `endpoint.rs`;
- the helper's URL policy, redirect capture and cookie jar;
- `proxy.rs`, the `PKGBUILD` and `deny.toml`.

The log redaction code (`log.rs`, about 170 lines) is defensible: it guards FreeRDP's output and is well tested. If it's ever simplified, start with the IPv6 heuristics. `tray.rs` hand-writes the tray protocol, which is justified by the minimal-dependencies rule.
