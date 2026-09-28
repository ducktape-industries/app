//! The key step, the quiz, and the account step (sign_in.rs).

use super::sign_in::{normalize_phrase, quiz_matches, quiz_positions};
use super::test_support::{resolved, signing_in};
use super::{AppMessage as Message, Ducktape, Stage};

#[test]
fn normalize_phrase_folds_whitespace_and_case() {
    assert_eq!(
        normalize_phrase("  Canoe\n Pond\tFOREST  "),
        "canoe pond forest"
    );
}

#[test]
fn the_quiz_takes_the_asked_words_in_any_case() {
    let phrase = (1..=24)
        .map(|n| format!("w{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    let answers = |a: &str, b: &str, c: &str| [a.to_string(), b.to_string(), c.to_string()];
    assert!(quiz_matches(
        &phrase,
        [4, 11, 19],
        &answers("w5", " W12 ", "w20")
    ));
    assert!(!quiz_matches(
        &phrase,
        [4, 11, 19],
        &answers("w5", "w12", "w21")
    ));
    assert!(!quiz_matches(&phrase, [4, 11, 19], &answers("", "", "")));
    let asked = quiz_positions(24);
    assert!(asked[0] < asked[1] && asked[1] < asked[2] && asked[2] < 24);
}

fn password(state: &Ducktape) -> &str {
    match &state.stage {
        Stage::Unlock(step) => step.password.as_str(),
        _ => "",
    }
}

#[test]
fn a_failed_unlock_keeps_the_typed_password_and_success_wipes_it() {
    let mut state = signing_in();
    state.key_exists = true;
    let _ = state.update(Message::UnlockSubmit);
    assert!(!state.sign_in.unlock_busy, "an empty password is not sent");
    let _ = state.update(Message::PasswordTyped("hunter22".into()));
    let _ = state.update(Message::UnlockSubmit);
    assert_eq!(password(&state), "hunter22");
    let _ = state.update(Message::UnlockFailed("wrong".into()));
    assert_eq!(
        password(&state),
        "hunter22",
        "a retry must send what the field shows"
    );
    let _ = state.update(Message::Unlocked("ab".into()));
    assert_eq!(state.stage.step(), "Unlock/awaiting");
    assert!(password(&state).is_empty());
}

#[test]
fn a_lock_stays_locked_until_unlock_and_approving_needs_a_found_request() {
    let mut state = signing_in();
    let _ = state.update(Message::Unlocked("ab".into()));
    let _ = state.update(Message::Lock);
    assert!(state.sign_in.locked && state.signer_key.is_empty());
    assert!(!state.sign_in.seating, "a lock reopened the key on its own");
    let _ = state.update(Message::ApproveOpen);
    let _ = state.update(Message::ApproveConfirm);
    assert!(!state.sign_in.unlock_busy, "approved with nothing found");
}

/// A new account lands on the desk with Help open in it; one that
/// skips the step, or a key that already has an account, does not.
#[test]
fn a_new_account_opens_on_help() {
    let modules = |state: &Ducktape| -> Vec<&'static str> {
        let console = state.console_win.unwrap();
        state.layouts.get(&console).map_or(Vec::new(), |layout| {
            layout.panes.iter().map(|pane| pane.module).collect()
        })
    };
    let mut state = signing_in();
    state.console_win = Some(crate::shell::WindowKey::unique());
    let _ = state.update(Message::Unlocked("ab".into()));
    resolved(&mut state, None);
    assert_eq!(state.stage.step(), "Account");
    let _ = state.update(Message::AccountCreated(Ok((7, "ada".into()))));
    assert_eq!(state.stage.step(), "Desk");
    assert_eq!(modules(&state), [crate::ui::layout::HELP]);
    assert_eq!(state.active, None, "help is no program");
    assert!(state.welcome, "a new account is greeted");
    let _ = state.update(Message::OpenHelp);
    assert_eq!(modules(&state), [crate::ui::layout::HELP], "not twice");
    assert!(!state.welcome, "asked for, help is just help");

    let mut later = signing_in();
    later.console_win = Some(crate::shell::WindowKey::unique());
    let _ = later.update(Message::Unlocked("ab".into()));
    resolved(&mut later, None);
    let _ = later.update(Message::CreateAccountLater);
    assert!(modules(&later).is_empty());

    let mut known = signing_in();
    known.console_win = Some(crate::shell::WindowKey::unique());
    let _ = known.update(Message::Unlocked("ab".into()));
    resolved(&mut known, Some((7, "ada".into())));
    assert_eq!(known.stage.step(), "Desk");
    assert!(modules(&known).is_empty());
}

#[test]
fn a_sign_in_without_an_account_opens_the_account_step_once() {
    for signed_in in [
        Message::Unlocked("ab".into()),
        Message::DeviceKey(Ok(Some("ab".into()))),
    ] {
        let mut state = signing_in();
        let _ = state.update(signed_in);
        assert_eq!(
            state.stage.step(),
            "Unlock/awaiting",
            "shown before the node answered"
        );
        resolved(&mut state, None);
        assert_eq!(state.stage.step(), "Account");
        let _ = state.update(Message::CreateAccountLater);
        resolved(&mut state, None);
        assert_eq!(state.stage.step(), "Desk", "a later block reopened it");
    }
}

/// The key step to what follows it, one message at a time: the desk
/// (and its window size) never shows before the node's answer, so a
/// key with no account never flashes the console on its way to the
/// account step (#292).
#[test]
fn the_desk_waits_for_the_answer_after_the_key_step() {
    for (account, then) in [(None, "Account"), (Some((7, "ada".into())), "Desk")] {
        let mut state = signing_in();
        assert_eq!(state.stage.step(), "Unlock");
        let _ = state.update(Message::DeviceKey(Ok(Some("ab".into()))));
        assert_eq!(
            state.stage.step(),
            "Unlock/awaiting",
            "a stage before the answer"
        );
        assert!(state.in_launcher());
        resolved(&mut state, account);
        assert_eq!(state.stage.step(), then);
    }
}

#[test]
fn the_account_step_is_skipped_for_a_key_with_an_account_or_a_passkey() {
    let mut state = signing_in();
    let _ = state.update(Message::Unlocked("ab".into()));
    resolved(&mut state, Some((7, "ada".into())));
    assert_eq!(state.stage.step(), "Desk");
    let mut state = signing_in();
    state.signer_key = "ab".into();
    state.stage = Stage::Account(Default::default());
    let _ = state.update(Message::PasskeyDone("ab".into()));
    resolved(&mut state, None);
    assert_eq!(state.stage.step(), "Desk", "the passkey made the account");
    // another key's answer neither opens nor disarms it
    let mut state = signing_in();
    let _ = state.update(Message::Unlocked("ab".into()));
    let _ = state.update(Message::AccountResolved {
        node: state.connected_rpc.clone(),
        key: "cd".into(),
        account: None,
    });
    assert_eq!(state.stage.step(), "Unlock/awaiting");
    resolved(&mut state, None);
    assert_eq!(state.stage.step(), "Account");
}

#[test]
fn the_rail_reopens_the_step_and_a_created_account_closes_it() {
    let mut state = signing_in();
    state.signer_key = "ab".into();
    state.stage = Stage::Desk;
    let _ = state.update(Message::ShowCreateAccount);
    assert_eq!(state.stage.step(), "Account");
    let _ = state.update(Message::CreateAccountSubmit);
    assert!(!state.sign_in.unlock_busy, "an empty name is not sent");
    assert!(!state.sign_in.unlock_error.is_empty());
    let _ = state.update(Message::AccountCreated(Err("refused".into())));
    assert_eq!(state.stage.step(), "Account");
    assert!(state.sign_in.unlock_error.contains("refused"));
    let _ = state.update(Message::AccountCreated(Err(
        "error sending request for url (http://127.0.0.1:1/)".into(),
    )));
    assert!(
        !state.sign_in.unlock_error.contains("url"),
        "a transport string"
    );
    let _ = state.update(Message::AccountCreated(Ok((9, "ada".into()))));
    assert_eq!(state.stage.step(), "Desk");
    assert_eq!(state.account, Some(Some((9, "ada".into()))));
}
