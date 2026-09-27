//! Every way between the screens, as one table (sign_in.rs, connect.rs).

use super::test_support::resolved;
use super::{AppMessage as Message, Ducktape, Stage};
use crate::backend;

/// Every way between the screens: from a stage built the way the app
/// reaches it, one message (or a network taken up, which is what
/// `Connected` does to the stage), and the stage it lands on.
#[test]
fn every_onboarding_transition() {
    use super::Unlock;
    type Build = fn() -> Ducktape;
    type Event = fn(&mut Ducktape);
    fn connect() -> Ducktape {
        Ducktape::boot().0
    }
    fn unlock() -> Ducktape {
        let mut state = connect();
        state.connected = true;
        state.connected_rpc = "http://a".into();
        state.network = "testkit".into();
        state.keyring = "testkit".into();
        state.stage = Stage::Unlock(Unlock::default());
        state
    }
    fn locked() -> Ducktape {
        let mut state = desk();
        let _ = state.update(Message::Lock);
        state
    }
    fn awaiting() -> Ducktape {
        let mut state = unlock();
        let _ = state.update(Message::Unlocked("ab".into()));
        state
    }
    fn account() -> Ducktape {
        let mut state = awaiting();
        resolved(&mut state, None);
        state
    }
    fn linking() -> Ducktape {
        let mut state = account();
        let _ = state.update(Message::LinkStart);
        state
    }
    fn recover() -> Ducktape {
        let mut state = account();
        let _ = state.update(Message::RecoverShow);
        state
    }
    fn desk() -> Ducktape {
        let mut state = awaiting();
        resolved(&mut state, Some((7, "ada".into())));
        state
    }
    fn browsing() -> Ducktape {
        let mut state = unlock();
        let _ = state.update(Message::BrowseWithoutKey);
        state
    }
    fn phrase() -> Ducktape {
        let mut state = desk();
        let _ = state.update(Message::RecoveryKeyStart);
        state
    }
    fn quiz() -> Ducktape {
        let mut state = phrase();
        let _ = state.update(Message::PhraseWrittenDown);
        state
    }
    let table: Vec<(&str, Build, Event, &str)> = vec![
        // the fresh flow
        (
            "node reached",
            connect,
            |s| {
                drop(s.take_up(backend::Keyring {
                    dir: "testkit".into(),
                    other_chain: false,
                }))
            },
            "Unlock",
        ),
        (
            "key opened",
            unlock,
            |s| drop(s.update(Message::DeviceKey(Ok(Some("ab".into()))))),
            "Unlock/awaiting",
        ),
        (
            "old key's password",
            unlock,
            |s| drop(s.update(Message::DeviceKey(Ok(None)))),
            "Unlock",
        ),
        (
            "key refused",
            unlock,
            |s| drop(s.update(Message::DeviceKey(Err("no".into())))),
            "Unlock",
        ),
        (
            "password wrong",
            unlock,
            |s| drop(s.update(Message::UnlockFailed("no".into()))),
            "Unlock",
        ),
        (
            "password right",
            unlock,
            |s| drop(s.update(Message::Unlocked("ab".into()))),
            "Unlock/awaiting",
        ),
        ("no account", awaiting, |s| resolved(s, None), "Account"),
        (
            "an account",
            awaiting,
            |s| resolved(s, Some((7, "ada".into()))),
            "Desk",
        ),
        (
            "another key's answer",
            awaiting,
            |s| {
                drop(s.update(Message::AccountResolved {
                    node: "http://a".into(),
                    key: "cd".into(),
                    account: None,
                }))
            },
            "Unlock/awaiting",
        ),
        (
            "an old node's answer",
            awaiting,
            |s| {
                drop(s.update(Message::AccountResolved {
                    node: "http://old".into(),
                    key: "ab".into(),
                    account: None,
                }))
            },
            "Unlock/awaiting",
        ),
        (
            "reading while the answer comes",
            awaiting,
            |s| drop(s.update(Message::BrowseWithoutKey)),
            "Unlock/awaiting",
        ),
        (
            "other networks",
            unlock,
            |s| drop(s.update(Message::Disconnect)),
            "Connect",
        ),
        // read-only browsing
        (
            "read without a key",
            unlock,
            |s| drop(s.update(Message::BrowseWithoutKey)),
            "Desk",
        ),
        (
            "sign in from the bar",
            browsing,
            |s| drop(s.update(Message::SignIn)),
            "Unlock",
        ),
        (
            "the key opens while reading",
            browsing,
            |s| drop(s.update(Message::DeviceKey(Ok(Some("ab".into()))))),
            "Unlock/awaiting",
        ),
        (
            "no account to make without a key",
            browsing,
            |s| drop(s.update(Message::ShowCreateAccount)),
            "Desk",
        ),
        // the account step
        (
            "not now",
            account,
            |s| drop(s.update(Message::CreateAccountLater)),
            "Desk",
        ),
        (
            "an empty name",
            account,
            |s| drop(s.update(Message::CreateAccountSubmit)),
            "Account",
        ),
        (
            "created",
            account,
            |s| drop(s.update(Message::AccountCreated(Ok((9, "ada".into()))))),
            "Desk",
        ),
        (
            "not created",
            account,
            |s| drop(s.update(Message::AccountCreated(Err("refused".into())))),
            "Account",
        ),
        (
            "passkey unnamed",
            account,
            |s| drop(s.update(Message::PasskeyCreateSubmit)),
            "Account",
        ),
        (
            "passkey create",
            account,
            |s| {
                let _ = s.update(Message::AccountNameTyped("ada".into()));
                drop(s.update(Message::PasskeyCreateSubmit))
            },
            "Account/passkey",
        ),
        (
            "passkey sign-in",
            account,
            |s| drop(s.update(Message::PasskeySignInSubmit)),
            "Account/passkey",
        ),
        (
            "passkey done",
            account,
            |s| {
                let _ = s.update(Message::PasskeySignInSubmit);
                drop(s.update(Message::PasskeyDone("ab".into())))
            },
            "Desk",
        ),
        (
            "passkey for another key",
            account,
            |s| drop(s.update(Message::PasskeyDone("cd".into()))),
            "Account",
        ),
        (
            "passkey failed",
            account,
            |s| {
                let _ = s.update(Message::PasskeySignInSubmit);
                drop(s.update(Message::PasskeyFailed("no".into())))
            },
            "Account",
        ),
        (
            "passkey cancelled",
            account,
            |s| {
                let _ = s.update(Message::PasskeySignInSubmit);
                drop(s.update(Message::PasskeyCancel))
            },
            "Account",
        ),
        (
            "later answers stay",
            account,
            |s| resolved(s, None),
            "Account",
        ),
        // add-device / link
        (
            "from another device",
            account,
            |s| drop(s.update(Message::LinkStart)),
            "Account/link",
        ),
        (
            "link cancelled",
            linking,
            |s| drop(s.update(Message::LinkCancel)),
            "Account",
        ),
        (
            "approved",
            linking,
            |s| drop(s.update(Message::Joined(Ok(())))),
            "Desk",
        ),
        (
            "link expired",
            linking,
            |s| drop(s.update(Message::Joined(Err("gone".into())))),
            "Account",
        ),
        // the recovery key, typed
        (
            "use a recovery key",
            account,
            |s| drop(s.update(Message::RecoverShow)),
            "Recover",
        ),
        (
            "back",
            recover,
            |s| drop(s.update(Message::RecoverCancel)),
            "Account",
        ),
        (
            "not words",
            recover,
            |s| {
                let _ = s.update(Message::RestorePhraseTyped("not a phrase".into()));
                drop(s.update(Message::RecoverSubmit))
            },
            "Recover",
        ),
        (
            "recovered",
            recover,
            |s| drop(s.update(Message::Joined(Ok(())))),
            "Desk",
        ),
        (
            "not recovered",
            recover,
            |s| drop(s.update(Message::Joined(Err("no".into())))),
            "Recover",
        ),
        // a new recovery phrase, and its quiz
        (
            "recovery key from the menu",
            desk,
            |s| drop(s.update(Message::RecoveryKeyStart)),
            "Phrase",
        ),
        (
            "written down",
            phrase,
            |s| drop(s.update(Message::PhraseWrittenDown)),
            "Phrase/quiz",
        ),
        (
            "not now",
            phrase,
            |s| drop(s.update(Message::PhraseCancel)),
            "Desk",
        ),
        (
            "see them again",
            quiz,
            |s| drop(s.update(Message::PhraseShowAgain)),
            "Phrase",
        ),
        (
            "wrong words",
            quiz,
            |s| drop(s.update(Message::PhraseCheckSubmit)),
            "Phrase/quiz",
        ),
        (
            "added",
            quiz,
            |s| drop(s.update(Message::RecoveryKeyAdded(Ok(())))),
            "Desk",
        ),
        (
            "not added",
            quiz,
            |s| drop(s.update(Message::RecoveryKeyAdded(Err("no".into())))),
            "Phrase/quiz",
        ),
        // lock and unlock
        ("lock", desk, |s| drop(s.update(Message::Lock)), "Unlock"),
        (
            "unlock",
            locked,
            |s| drop(s.update(Message::UnlockSubmit)),
            "Unlock",
        ),
        (
            "unlocked",
            locked,
            |s| drop(s.update(Message::Unlocked("ab".into()))),
            "Unlock/awaiting",
        ),
        (
            "read while locked",
            locked,
            |s| drop(s.update(Message::BrowseWithoutKey)),
            "Desk",
        ),
        // the desk
        (
            "create account from the rail",
            desk,
            |s| drop(s.update(Message::ShowCreateAccount)),
            "Account",
        ),
        (
            "approve a device",
            desk,
            |s| drop(s.update(Message::ApproveOpen)),
            "Desk",
        ),
        (
            "a later block's answer",
            desk,
            |s| resolved(s, None),
            "Desk",
        ),
        // switching network
        (
            "switch reaching",
            desk,
            |s| drop(s.update(Message::SwitchNetwork("http://b".into()))),
            "Desk",
        ),
        (
            "switch failed",
            desk,
            |s| {
                let _ = s.update(Message::SwitchNetwork("http://b".into()));
                drop(s.update(Message::ConnectFailed {
                    generation: s.connect_generation,
                    error: "no".into(),
                }))
            },
            "Desk",
        ),
        (
            "switch to the same network",
            desk,
            |s| {
                drop(s.take_up(backend::Keyring {
                    dir: "testkit".into(),
                    other_chain: false,
                }))
            },
            "Desk",
        ),
        (
            "switch to another chain",
            desk,
            |s| {
                drop(s.take_up(backend::Keyring {
                    dir: "testkit+2".into(),
                    other_chain: true,
                }))
            },
            "Unlock",
        ),
        (
            "add a network",
            desk,
            |s| drop(s.update(Message::Disconnect)),
            "Connect",
        ),
        // off every network
        (
            "a key after leaving",
            connect,
            |s| drop(s.update(Message::Unlocked("ab".into()))),
            "Connect",
        ),
        (
            "not reached",
            connect,
            |s| {
                drop(s.update(Message::ConnectFailed {
                    generation: s.connect_generation,
                    error: "no".into(),
                }))
            },
            "Connect",
        ),
    ];
    for (what, from, event, to) in table {
        let mut state = from();
        let before = state.stage.step();
        event(&mut state);
        assert_eq!(state.stage.step(), to, "{before} --{what}--> ");
        assert_eq!(state.in_launcher(), to != "Desk", "{what}");
    }
}
