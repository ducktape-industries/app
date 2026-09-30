//! `Account`'s flows, as the reducer's tests had them: every way between
//! the screens as one table, the key and account steps' guards, and what
//! the desktop does on the account's events (a lock's toast, a new
//! account's Help, a network left).
use super::super::{Desktop, WindowKey};
use super::tests::session;
use super::{Account, AccountStep, Chain, Entities, Screen, Session, Slice};
use crate::backend;
use crate::ui::layout::{EMPTY, HELP, Layout};
use crate::{AppMessage as Message, Ducktape};
use gpui_kit::{AppContext as _, Entity, TestAppContext};

/// The node the account is on: none, so it asks nothing. (A socket's
/// answer would wake the test from the runtime's thread, which the test
/// scheduler refuses as non-deterministic.)
const NODE: &str = "";

fn step(screen: Screen) -> &'static str {
    match screen {
        Screen::Connect => "Connect",
        Screen::Unlock { awaiting: false } => "Unlock",
        Screen::Unlock { awaiting: true } => "Unlock/awaiting",
        Screen::Phrase { quiz: None } => "Phrase",
        Screen::Phrase { quiz: Some(_) } => "Phrase/quiz",
        Screen::Recover => "Recover",
        Screen::Account {
            step: AccountStep::Name,
        } => "Account",
        Screen::Account {
            step: AccountStep::Link,
        } => "Account/link",
        Screen::Account {
            step: AccountStep::Passkey,
        } => "Account/passkey",
        Screen::Desk => "Desk",
    }
}

/// An account with its screen, on no network.
fn fresh(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
    let session = cx.update(session);
    let (_, account, screen) = session.read_with(cx, |session, _| session.parts());
    (account, screen)
}

fn shown(screen: &Entity<Slice<Screen>>, cx: &mut TestAppContext) -> &'static str {
    step(screen.read_with(cx, |screen, _| *screen.get()))
}

fn state(account: &Entity<Account>, cx: &mut TestAppContext) -> super::AccountState {
    account.read_with(cx, |account, _| account.get().clone())
}

/// The key step on the test network. Its keyring is blank: this device's
/// key is neither opened nor minted for it (`open_device_key` asks
/// nothing without one).
fn unlock(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
    let (account, screen) = fresh(cx);
    account.update(cx, |account, _| account.seed_network(NODE, "testkit", ""));
    screen.update(cx, |screen, cx| {
        screen.set(Screen::Unlock { awaiting: false }, cx);
    });
    (account, screen)
}

fn awaiting(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
    let (account, screen) = unlock(cx);
    account.update(cx, |account, cx| account.unlocked("ab".into(), cx));
    (account, screen)
}

fn resolved(account: &Entity<Account>, found: Option<(u64, String)>, cx: &mut TestAppContext) {
    account.update(cx, |account, cx| account.resolved(NODE, "ab", found, cx));
}

fn account_step(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
    let (account, screen) = awaiting(cx);
    resolved(&account, None, cx);
    (account, screen)
}

fn desk(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
    let (account, screen) = awaiting(cx);
    resolved(&account, Some((7, "ada".into())), cx);
    (account, screen)
}

/// Every way between the screens: from a screen reached the way the app
/// reaches it, one call (or a network taken up, which is what a node
/// answering does to the account), and the screen it lands on. (Leaving
/// and switching networks are `Session`'s, in session_tests.rs.)
#[gpui_kit::test]
fn every_onboarding_transition(cx: &mut TestAppContext) {
    let _seat = backend::seat_serial();
    type Build = fn(&mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>);
    type Call = fn(&mut Account, &mut gpui_kit::Context<Account>);
    fn locked(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
        let (account, screen) = desk(cx);
        account.update(cx, |account, cx| account.lock(cx));
        (account, screen)
    }
    fn linking(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
        let (account, screen) = account_step(cx);
        account.update(cx, |account, cx| account.link_start(cx));
        (account, screen)
    }
    fn recover(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
        let (account, screen) = account_step(cx);
        account.update(cx, |account, cx| account.recover_show(cx));
        (account, screen)
    }
    fn browsing(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
        let (account, screen) = unlock(cx);
        account.update(cx, |account, cx| account.browse_without_key(cx));
        (account, screen)
    }
    fn phrase(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
        let (account, screen) = desk(cx);
        account.update(cx, |account, cx| account.recovery_key_start(cx));
        (account, screen)
    }
    fn quiz(cx: &mut TestAppContext) -> (Entity<Account>, Entity<Slice<Screen>>) {
        let (account, screen) = phrase(cx);
        account.update(cx, |account, cx| account.phrase_written_down(cx));
        (account, screen)
    }
    fn testkit() -> backend::Keyring {
        backend::Keyring {
            dir: "testkit".into(),
            other_chain: false,
        }
    }
    let table: Vec<(&str, Build, Call, &str)> = vec![
        // the fresh flow
        (
            "node reached",
            fresh,
            |a, cx| {
                a.take_up(testkit(), NODE.into(), "testkit".into(), cx);
            },
            "Unlock",
        ),
        (
            "key opened",
            unlock,
            |a, cx| a.device_key_answered(Ok(Some("ab".into())), cx),
            "Unlock/awaiting",
        ),
        (
            "old key's password",
            unlock,
            |a, cx| a.device_key_answered(Ok(None), cx),
            "Unlock",
        ),
        (
            "key refused",
            unlock,
            |a, cx| a.device_key_answered(Err("no".into()), cx),
            "Unlock",
        ),
        (
            "password wrong",
            unlock,
            |a, cx| a.unlock_failed("no".into(), cx),
            "Unlock",
        ),
        (
            "password right",
            unlock,
            |a, cx| a.unlocked("ab".into(), cx),
            "Unlock/awaiting",
        ),
        (
            "no account",
            awaiting,
            |a, cx| a.resolved(NODE, "ab", None, cx),
            "Account",
        ),
        (
            "an account",
            awaiting,
            |a, cx| a.resolved(NODE, "ab", Some((7, "ada".into())), cx),
            "Desk",
        ),
        (
            "another key's answer",
            awaiting,
            |a, cx| a.resolved(NODE, "cd", None, cx),
            "Unlock/awaiting",
        ),
        (
            "an old node's answer",
            awaiting,
            |a, cx| a.resolved("http://old", "ab", None, cx),
            "Unlock/awaiting",
        ),
        (
            "reading while the answer comes",
            awaiting,
            |a, cx| a.browse_without_key(cx),
            "Unlock/awaiting",
        ),
        // read-only browsing
        (
            "read without a key",
            unlock,
            |a, cx| a.browse_without_key(cx),
            "Desk",
        ),
        (
            "sign in from the bar",
            browsing,
            |a, cx| a.sign_in(cx),
            "Unlock",
        ),
        (
            "the key opens while reading",
            browsing,
            |a, cx| a.device_key_answered(Ok(Some("ab".into())), cx),
            "Unlock/awaiting",
        ),
        (
            "no account to make without a key",
            browsing,
            |a, cx| a.create_account(cx),
            "Desk",
        ),
        // the account step
        ("not now", account_step, |a, cx| a.create_later(cx), "Desk"),
        (
            "an empty name",
            account_step,
            |a, cx| a.create_submit("  ".into(), cx),
            "Account",
        ),
        (
            "created",
            account_step,
            |a, cx| a.account_created(Ok((9, "ada".into())), cx),
            "Desk",
        ),
        (
            "not created",
            account_step,
            |a, cx| a.account_created(Err("refused".into()), cx),
            "Account",
        ),
        (
            "passkey unnamed",
            account_step,
            |a, cx| a.passkey_create(" ".into(), cx),
            "Account",
        ),
        (
            "passkey create",
            account_step,
            |a, cx| a.passkey_create("ada".into(), cx),
            "Account/passkey",
        ),
        (
            "passkey done",
            account_step,
            |a, cx| {
                a.passkey_create("ada".into(), cx);
                a.passkey_done("ab".into(), cx)
            },
            "Desk",
        ),
        (
            "passkey for another key",
            account_step,
            |a, cx| a.passkey_done("cd".into(), cx),
            "Account",
        ),
        (
            "passkey failed",
            account_step,
            |a, cx| {
                a.passkey_create("ada".into(), cx);
                a.passkey_failed("no".into(), cx)
            },
            "Account",
        ),
        (
            "passkey cancelled",
            account_step,
            |a, cx| {
                a.passkey_create("ada".into(), cx);
                a.passkey_cancel(cx)
            },
            "Account",
        ),
        (
            "later answers stay",
            account_step,
            |a, cx| a.resolved(NODE, "ab", None, cx),
            "Account",
        ),
        // add-device / link
        (
            "from another device",
            account_step,
            |a, cx| a.link_start(cx),
            "Account/link",
        ),
        (
            "link cancelled",
            linking,
            |a, cx| a.link_cancel(cx),
            "Account",
        ),
        ("approved", linking, |a, cx| a.joined(Ok(()), cx), "Desk"),
        (
            "link expired",
            linking,
            |a, cx| a.joined(Err("gone".into()), cx),
            "Account",
        ),
        // the recovery key, typed
        (
            "use a recovery key",
            account_step,
            |a, cx| a.recover_show(cx),
            "Recover",
        ),
        ("back", recover, |a, cx| a.recover_cancel(cx), "Account"),
        (
            "not words",
            recover,
            |a, cx| a.recover_submit("not a phrase".into(), cx),
            "Recover",
        ),
        ("recovered", recover, |a, cx| a.joined(Ok(()), cx), "Desk"),
        (
            "not recovered",
            recover,
            |a, cx| a.joined(Err("no".into()), cx),
            "Recover",
        ),
        // a new recovery phrase, and its quiz
        (
            "recovery key from the menu",
            desk,
            |a, cx| a.recovery_key_start(cx),
            "Phrase",
        ),
        (
            "written down",
            phrase,
            |a, cx| a.phrase_written_down(cx),
            "Phrase/quiz",
        ),
        ("not now", phrase, |a, cx| a.phrase_cancel(cx), "Desk"),
        (
            "see them again",
            quiz,
            |a, cx| a.phrase_show_again(cx),
            "Phrase",
        ),
        (
            "wrong words",
            quiz,
            |a, cx| a.phrase_check(["x".into(), "y".into(), "z".into()], cx),
            "Phrase/quiz",
        ),
        (
            "added",
            quiz,
            |a, cx| a.recovery_key_added(Ok(()), cx),
            "Desk",
        ),
        (
            "not added",
            quiz,
            |a, cx| a.recovery_key_added(Err("no".into()), cx),
            "Phrase/quiz",
        ),
        // lock and unlock
        ("lock", desk, |a, cx| a.lock(cx), "Unlock"),
        (
            "unlock",
            locked,
            |a, cx| a.unlock(String::new(), cx),
            "Unlock",
        ),
        (
            "unlocked",
            locked,
            |a, cx| a.unlocked("ab".into(), cx),
            "Unlock/awaiting",
        ),
        (
            "read while locked",
            locked,
            |a, cx| a.browse_without_key(cx),
            "Desk",
        ),
        // the desk
        (
            "create account from the rail",
            desk,
            |a, cx| a.create_account(cx),
            "Account",
        ),
        ("approve a device", desk, |a, cx| a.approve_open(cx), "Desk"),
        (
            "a later block's answer",
            desk,
            |a, cx| a.resolved(NODE, "ab", None, cx),
            "Desk",
        ),
    ];
    for (name, build, call, then) in table {
        let (account, screen) = build(cx);
        account.update(cx, call);
        assert_eq!(shown(&screen, cx), then, "{name}");
    }
}

#[gpui_kit::test]
fn an_empty_password_or_name_is_not_sent(cx: &mut TestAppContext) {
    let (account, _) = unlock(cx);
    account.update(cx, |account, cx| {
        let mut state = account.get().clone();
        state.key_exists = true;
        account.seed(state, None, None, cx);
        account.unlock(String::new(), cx);
    });
    let key = state(&account, cx);
    assert!(!key.busy, "an empty password is not sent");
    assert!(!key.error.is_empty());

    let (account, screen) = account_step(cx);
    account.update(cx, |account, cx| account.create_submit("  ".into(), cx));
    let name = state(&account, cx);
    assert!(!name.busy, "an empty name is not sent");
    assert!(!name.error.is_empty());
    assert_eq!(shown(&screen, cx), "Account");
}

#[gpui_kit::test]
fn a_lock_stays_locked_until_unlock_and_approving_needs_a_found_request(cx: &mut TestAppContext) {
    let _seat = backend::seat_serial();
    let (account, _) = awaiting(cx);
    account.update(cx, |account, cx| account.lock(cx));
    let locked = state(&account, cx);
    assert!(locked.locked && locked.signer_key.is_empty());
    assert!(!locked.seating, "a lock reopened the key on its own");
    account.update(cx, |account, cx| {
        account.open_device_key(cx);
        account.approve_open(cx);
        account.approve_confirm(cx);
    });
    let state = state(&account, cx);
    assert!(!state.seating, "the key reopened while locked");
    assert!(!state.busy, "approved with nothing found");
}

#[gpui_kit::test]
fn a_sign_in_without_an_account_opens_the_account_step_once(cx: &mut TestAppContext) {
    type Call = fn(&mut Account, &mut gpui_kit::Context<Account>);
    let signed_in: [Call; 2] = [
        |a, cx| a.unlocked("ab".into(), cx),
        |a, cx| a.device_key_answered(Ok(Some("ab".into())), cx),
    ];
    for call in signed_in {
        let (account, screen) = unlock(cx);
        account.update(cx, call);
        assert_eq!(
            shown(&screen, cx),
            "Unlock/awaiting",
            "shown before the node answered"
        );
        resolved(&account, None, cx);
        assert_eq!(shown(&screen, cx), "Account");
        account.update(cx, |account, cx| account.create_later(cx));
        resolved(&account, None, cx);
        assert_eq!(shown(&screen, cx), "Desk", "a later block reopened it");
    }
}

/// The key step to what follows it, one call at a time: the desk never
/// shows before the node's answer, so a key with no account never
/// flashes the console on its way to the account step (#292).
#[gpui_kit::test]
fn the_account_step_is_skipped_for_a_key_with_an_account_or_a_passkey(cx: &mut TestAppContext) {
    let (account, screen) = awaiting(cx);
    resolved(&account, Some((7, "ada".into())), cx);
    assert_eq!(shown(&screen, cx), "Desk");

    let (account, screen) = account_step(cx);
    account.update(cx, |account, cx| account.passkey_done("ab".into(), cx));
    resolved(&account, None, cx);
    assert_eq!(shown(&screen, cx), "Desk", "the passkey made the account");

    // another key's answer neither opens nor disarms it
    let (account, screen) = awaiting(cx);
    account.update(cx, |account, cx| account.resolved(NODE, "cd", None, cx));
    assert_eq!(shown(&screen, cx), "Unlock/awaiting");
    resolved(&account, None, cx);
    assert_eq!(shown(&screen, cx), "Account");
}

#[gpui_kit::test]
fn the_rail_reopens_the_step_and_a_created_account_closes_it(cx: &mut TestAppContext) {
    let (account, screen) = desk(cx);
    account.update(cx, |account, cx| account.create_account(cx));
    assert_eq!(shown(&screen, cx), "Account");
    account.update(cx, |account, cx| {
        account.account_created(Err("refused".into()), cx)
    });
    assert_eq!(shown(&screen, cx), "Account");
    assert!(state(&account, cx).error.contains("refused"));
    account.update(cx, |account, cx| {
        account.account_created(
            Err("error sending request for url (http://127.0.0.1:1/)".into()),
            cx,
        )
    });
    assert!(
        !state(&account, cx).error.contains("url"),
        "a transport string"
    );
    account.update(cx, |account, cx| {
        account.account_created(Ok((9, "ada".into())), cx)
    });
    assert_eq!(shown(&screen, cx), "Desk");
    assert_eq!(state(&account, cx).account, Some(Some((9, "ada".into()))));
}

/// "Add a device…" opened or closed forgets what the last one found and
/// its failure.
#[gpui_kit::test]
fn the_approve_dialog_forgets_what_it_found_when_it_opens_or_closes(cx: &mut TestAppContext) {
    let (account, _) = desk(cx);
    let found = || {
        let key = vec![7; 32];
        let mut state = super::AccountState {
            approve: Some(backend::join::fingerprint(&key)),
            error: "no".into(),
            ..Default::default()
        };
        state.signer_key = "ab".into();
        (
            state,
            backend::join::Request {
                network: "testkit".into(),
                key,
            },
        )
    };
    for close in [Account::approve_open, Account::approve_closed] {
        let (state, request) = found();
        account.update(cx, |account, cx| {
            account.seed(state, None, Some(request), cx);
            close(account, cx);
            account.approve_confirm(cx);
        });
        let state = self::state(&account, cx);
        assert_eq!(state.approve, None);
        assert!(state.error.is_empty());
        assert!(!state.busy, "confirmed what was forgotten");
    }
}

// ---------- what the desktop does on the account's events ----------

/// A desktop over `session`'s entities, its console window's desk laid
/// out; nothing is drawn.
fn desktop(cx: &mut TestAppContext) -> (Entity<Desktop>, WindowKey, Entities) {
    let mut state = Ducktape::boot();
    state.roster = Default::default();
    state.center = Default::default();
    let key = WindowKey::unique();
    let mut layout = Layout::default();
    layout.split(EMPTY);
    layout.measure((1280., 764.));
    layout.settle();
    layout.initialized = true;
    state.console_win = Some(key);
    state.layouts.insert(key, layout);
    let mut made = None;
    let model = cx.new(|cx| {
        let entities = Entities::for_test(&state, cx);
        made = Some(entities.clone());
        Desktop::new(state, crate::tray::init(cx).0, entities, cx)
    });
    (model, key, made.unwrap())
}

fn modules(model: &Entity<Desktop>, key: WindowKey, cx: &mut TestAppContext) -> Vec<&'static str> {
    model.read_with(cx, |model, _| modules_of(&model.state, key))
}

/// Locking says so: the account's toast reaches the reducer's notice.
#[gpui_kit::test]
fn a_locked_toast_still_shows(cx: &mut TestAppContext) {
    let _seat = backend::seat_serial();
    let (model, _, entities) = desktop(cx);
    entities.account.update(cx, |account, cx| account.lock(cx));
    cx.run_until_parked();
    let toast = model.read_with(cx, |model, _| model.state.toast.clone());
    assert_eq!(toast, "Locked");
}

/// A new account lands on the desk with Help open in it; one that skips
/// the step, or a key that already has an account, does not.
#[gpui_kit::test]
fn a_new_account_opens_on_help(cx: &mut TestAppContext) {
    let (model, key, entities) = desktop(cx);
    let account = &entities.account;
    let signing_in = |entities: &Entities, cx: &mut TestAppContext| {
        entities.screen.update(cx, |screen, cx| {
            screen.set(Screen::Unlock { awaiting: false }, cx);
        });
        entities.account.update(cx, |account, cx| {
            account.seed_network(NODE, "testkit", "");
            account.unlocked("ab".into(), cx);
        });
    };
    let welcome =
        |cx: &mut TestAppContext| account.read_with(cx, |account, _| account.get().welcome);
    signing_in(&entities, cx);
    resolved(account, None, cx);
    assert_eq!(shown(&entities.screen, cx), "Account");
    account.update(cx, |account, cx| {
        account.account_created(Ok((7, "ada".into())), cx)
    });
    assert_eq!(shown(&entities.screen, cx), "Desk");
    assert_eq!(modules(&model, key, cx), [HELP]);
    assert_eq!(
        model.read_with(cx, |model, _| model.state.active),
        None,
        "help is no program"
    );
    assert!(welcome(cx), "a new account is greeted");
    model.update(cx, |model, cx| model.dispatch(Message::OpenHelp, cx));
    assert_eq!(modules(&model, key, cx), [HELP], "not twice");
    assert!(!welcome(cx), "asked for, help is just help");

    let (model, key, entities) = desktop(cx);
    signing_in(&entities, cx);
    entities.account.update(cx, |account, cx| {
        account.resolved(NODE, "ab", None, cx);
        account.create_later(cx);
    });
    assert_eq!(
        modules(&model, key, cx),
        [EMPTY],
        "not now: the desk as it was"
    );

    let (model, key, entities) = desktop(cx);
    signing_in(&entities, cx);
    entities.account.update(cx, |account, cx| {
        account.resolved(NODE, "ab", Some((7, "ada".into())), cx);
    });
    assert_eq!(shown(&entities.screen, cx), "Desk");
    assert_eq!(
        modules(&model, key, cx),
        [EMPTY],
        "a known account is not greeted"
    );
}

/// Leaving the network (disconnect, or a switch that landed on another
/// chain): every desk's panes go, and the badges and the active program
/// with them; the chain's node too.
#[gpui_kit::test]
fn leaving_the_network_clears_the_desks_and_badges(cx: &mut TestAppContext) {
    let _seat = backend::seat_serial();
    let (model, key, entities) = desktop(cx);
    model.update(cx, |model, _| {
        model.state.active = Some("chat");
        model.state.badges.insert("chat", 3);
    });
    entities.chain.update(cx, |chain, cx| {
        chain.set(
            Chain {
                node: Some(crate::ui::test_support::status(7)),
                height: 7,
                ..Chain::default()
            },
            cx,
        );
    });
    assert_eq!(modules(&model, key, cx), [EMPTY]);
    entities.session.update(cx, Session::disconnect);
    cx.run_until_parked();
    model.read_with(cx, |model, _| {
        assert!(
            modules_of(&model.state, key).is_empty(),
            "the desk kept its panes"
        );
        assert_eq!(model.state.active, None);
        assert!(model.state.badges.is_empty());
        assert!(
            model.state.layouts.get(&key).unwrap().desk.is_some(),
            "the desk lost its measure"
        );
    });
    assert_eq!(
        entities.chain.read_with(cx, |chain, _| chain.node.clone()),
        None
    );
    assert_eq!(shown(&entities.screen, cx), "Connect");
}

fn modules_of(state: &Ducktape, key: WindowKey) -> Vec<&'static str> {
    state.layouts.get(&key).map_or(Vec::new(), |layout| {
        layout.panes.iter().map(|pane| pane.module).collect()
    })
}
