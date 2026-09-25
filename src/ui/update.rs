//! The reducer: one message in, the state moved, a task out. Each domain
//! has its own sub-reducer; this match only routes.

use super::{AppMessage as Message, Ducktape};
use view_wire::Subscription;
use view_wire::Task;

/// How often the node's status is asked for while connected.
pub(super) const STATUS_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

impl Ducktape {
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        use Message as M;
        match message {
            m @ (M::EndpointTyped(_)
            | M::ConnectSubmit
            | M::ConnectTo(_)
            | M::Connected { .. }
            | M::ConnectFailed { .. }
            | M::StatusPushed(_)
            | M::StatusMissed
            | M::AccountResolved { .. }
            | M::Disconnect
            | M::SwitchNetwork(_)
            | M::ForgetEndpoint(_)
            | M::Tick) => self.on_connect(m),
            m @ (M::ToggleNetworkMenu
            | M::TogglePopover(_)
            | M::CloseOverlay(_)
            | M::OpenSpotlight
            | M::SpotlightTyped(_)
            | M::SpotlightMove { .. }
            | M::SpotlightSubmit
            | M::Spot(_)
            | M::OpenSettings
            | M::ShowSettingsPage(_)) => self.on_overlay(m),
            m @ (M::NotifyOpen(_)
            | M::NotifyMarkAllRead
            | M::NotifyClearRead
            | M::NotifySettings
            | M::NotifyPermission(..)
            | M::NotifyNotNow(_)
            | M::SetNotifyBanners(_)
            | M::SetNotifyInFront(_)
            | M::SetNotifyBurst(_)) => self.on_notify(m),
            m @ (M::PasswordTyped(_)
            | M::UnlockSubmit
            | M::DeviceKey(_)
            | M::Unlocked(_)
            | M::UnlockFailed(_)
            | M::Lock
            | M::BrowseWithoutKey
            | M::SignIn
            | M::RecoveryKeyStart
            | M::PhraseCancel
            | M::PhraseWrittenDown
            | M::PhraseWordTyped(..)
            | M::PhraseCheckSubmit
            | M::RecoveryKeyAdded(_)
            | M::PhraseShowAgain
            | M::RestorePhraseTyped(_)
            | M::RecoverShow
            | M::RecoverCancel
            | M::RecoverSubmit
            | M::LinkStart
            | M::LinkCancel
            | M::Joined(_)
            | M::ApproveOpen
            | M::ApproveCodeTyped(_)
            | M::ApproveFind
            | M::ApproveFound(_)
            | M::ApproveConfirm
            | M::ApproveDone(_)
            | M::AccountNameTyped(_)
            | M::PasskeyCreateSubmit
            | M::PasskeySignInSubmit
            | M::PasskeyUsePhone
            | M::PasskeyQr(_)
            | M::PasskeyCancel
            | M::PasskeyFailed(_)
            | M::PasskeyDone(_)
            | M::ShowCreateAccount
            | M::CreateAccountLater
            | M::CreateAccountSubmit
            | M::AccountCreated(_)) => self.on_sign_in(m),
            m @ (M::SetAppearance(_)
            | M::SetMotion(_)
            | M::SelectView(_)
            | M::ViewShown(_)
            | M::ViewEvent(..)
            | M::OpenLink(_)
            | M::ShowToast(_)
            | M::DismissToast
            | M::ToastTick
            | M::WallTick
            | M::ConsoleOpened(_)
            | M::WindowWasClosed(_)
            | M::WindowFocused(_)
            | M::WindowUnfocused(_)
            | M::ModifierStateChanged(_)
            | M::TrayOpen
            | M::TrayQuit) => self.on_desk(m),
        }
    }

    /// What runs while the app does: the clocks, and nothing else.
    pub(crate) fn subscriptions(&self) -> Subscription<Message> {
        let mut recipes = vec![
            Subscription::run(wall_ticks),
            Subscription::run(toast_ticks),
        ];
        if self.connected {
            recipes.push(Subscription::run(status_ticks));
        }
        Subscription::batch(recipes)
    }
}

fn wall_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(std::time::Duration::from_secs(1)).map(|()| Message::WallTick)
}

fn toast_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(std::time::Duration::from_millis(300)).map(|()| Message::ToastTick)
}

fn status_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(STATUS_EVERY).map(|()| Message::Tick)
}

use futures::StreamExt as _;

#[cfg(test)]
mod tests {
    use super::super::sign_in::{normalize_phrase, quiz_matches, quiz_positions};
    use super::super::{Overlay, SeatRequest, Stage};
    use super::*;
    use crate::backend;
    use crate::runtime::Intent;

    #[test]
    fn normalize_phrase_folds_whitespace_and_case() {
        assert_eq!(
            normalize_phrase("  Canoe\n Pond\tFOREST  "),
            "canoe pond forest"
        );
    }

    #[test]
    fn a_link_on_this_chain_hands_its_view_the_route() {
        let (mut state, _) = Ducktape::boot();
        state.chain = "testkit#0a1b2c3d".into();
        crate::runtime::list_for_test("link-test-here");
        crate::runtime::list_for_test("link-test-away");
        let _ = state.update(Message::OpenLink(
            "duck://testkit-0a1b2c3d/link-test-here/tx/00ff".into(),
        ));
        assert_eq!(state.active, Some("link-test-here"));
        assert!(state.toast.is_empty(), "the view coming forward says it");
        assert_eq!(
            crate::runtime::take_route("link-test-here").as_deref(),
            Some("tx/00ff")
        );
        // another chain's link opens the seat, routes nothing, and says why
        let _ = state.update(Message::OpenLink(
            "duck://othernet-0a1b2c3d/link-test-away/tx/00ff".into(),
        ));
        assert_eq!(state.active, Some("link-test-away"));
        assert_eq!(crate::runtime::take_route("link-test-away"), None);
        assert!(state.toast.contains("othernet"));
        // a view nobody lists opens nothing
        state.toast.clear();
        let _ = state.update(Message::OpenLink(
            "duck://testkit-0a1b2c3d/link-test-nowhere/x".into(),
        ));
        assert_eq!(state.active, Some("link-test-away"));
        assert!(state.toast.contains("link-test-nowhere"));
    }

    #[test]
    fn a_view_event_sets_its_badge_and_opens_its_link() {
        let (mut state, _) = Ducktape::boot();
        let _ = state.update(Message::ViewEvent("chat", Intent::Badge(3)));
        assert_eq!(state.badges.get("chat"), Some(&3));
        let _ = state.update(Message::ViewEvent("chat", Intent::Badge(0)));
        assert!(state.badges.is_empty(), "a zero count clears the badge");
        let _ = state.update(Message::ViewEvent("chat", Intent::Badge(-2)));
        assert!(state.badges.is_empty());
        let _ = state.update(Message::ViewEvent("chat", Intent::Notified));
        assert!(state.badges.is_empty() && state.active.is_none());

        crate::runtime::list_for_test("view-event-link");
        let _ = state.update(Message::ViewEvent(
            "chat",
            Intent::OpenLink("duck://view-event-link/room/7".into()),
        ));
        assert_eq!(state.active, Some("view-event-link"));
        assert_eq!(
            state.seat_request,
            Some(SeatRequest::Open("view-event-link")),
            "a link brings its seat forward"
        );
        assert_eq!(
            crate::runtime::take_route("view-event-link").as_deref(),
            Some("room/7")
        );
    }

    #[test]
    fn two_missed_polls_read_reconnecting_and_one_answer_recovers() {
        let (mut state, _) = Ducktape::boot();
        state.connected = true;
        state.status = "Connected · block 7".into();
        let _ = state.update(Message::StatusMissed);
        assert!(!state.reconnecting(), "one miss is a hiccup");
        assert_eq!(state.status, "Connected · block 7");
        let _ = state.update(Message::StatusMissed);
        assert!(state.reconnecting());
        assert_eq!(state.status, "Reconnecting…");
        assert!(state.connected, "the poll keeps running");
        // same height: no block moved, so nothing else is asked for
        state.height = 9;
        let _ = state.update(Message::StatusPushed(backend::NodeStatus {
            network: "testkit".into(),
            time: 0,
            block_time_ms: 0,
            epoch_length: 0,
            height: 9,
            tip: [0; 32],
            root: abi::Root([0; 32]),
            epoch: 0,
            identity: Vec::new(),
            contract: backend::noded::NODE_CONTRACT,
            genesis: [0; 32],
        }));
        assert!(!state.reconnecting());
        assert_eq!(state.status, "Connected · block 9");
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

    /// On the key step, the node reached.
    fn signing_in() -> Ducktape {
        let (mut state, _) = Ducktape::boot();
        state.stage = Stage::Unlock(Default::default());
        state
    }

    #[test]
    fn a_new_try_clears_the_last_connect_failure() {
        let mut state = signing_in();
        state.error = "Can't reach http://a.".into();
        let _ = state.update(Message::EndpointTyped("not a url at all".into()));
        assert!(state.error.is_empty());
        state.error = "Can't reach http://a.".into();
        let _ = state.update(Message::ConnectSubmit);
        assert!(state.error.is_empty(), "the stale failure hid the refusal");
        assert_eq!(state.endpoint_error, backend::ENDPOINT_REFUSAL);
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

    #[test]
    fn switching_node_leaves_no_key_seated() {
        let mut state = signing_in();
        state.signer_key = "ab".into();
        state.stage = Stage::Account(super::super::Account {
            name: "ada".into(),
            link_code: "ABCD-EFGH".into(),
            ..Default::default()
        });
        state.sign_in.unlock_error = "wrong".into();
        let _ = state.update(Message::Disconnect);
        assert!(state.signer_key.is_empty());
        assert_eq!(state.stage.step(), "Connect");
        assert!(state.sign_in.unlock_error.is_empty());
    }

    fn resolved(state: &mut Ducktape, account: Option<(u64, String)>) {
        let key = state.signer_key.clone();
        let node = state.connected_rpc.clone();
        let _ = state.update(Message::AccountResolved { node, key, account });
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

    /// Every way between the screens: from a stage built the way the app
    /// reaches it, one message (or a network taken up, which is what
    /// `Connected` does to the stage), and the stage it lands on.
    #[test]
    fn every_onboarding_transition() {
        use super::super::Unlock;
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

    fn on_testkit() -> Ducktape {
        let mut state = signing_in();
        state.stage = Stage::Desk;
        state.connected = true;
        state.connected_rpc = "http://a".into();
        state.network = "testkit".into();
        state.keyring = "testkit".into();
        state.height = 7;
        state.status = "Connected · block 7".into();
        state.signer_key = "ab".into();
        state.account = Some(Some((7, "Grace Hopper".into())));
        state.active = Some("chat");
        state.badges.insert("chat", 3);
        state
    }

    #[test]
    fn a_switch_keeps_the_network_in_hand_until_the_other_answers() {
        let mut state = on_testkit();
        let _ = state.update(Message::ToggleNetworkMenu);
        assert_eq!(state.overlay, Some(Overlay::Network));
        let _ = state.update(Message::SwitchNetwork("http://b".into()));
        assert!(state.overlay.is_none() && state.connecting);
        assert_eq!(state.status, "Reaching http://b…");
        assert_eq!(state.signer_key, "ab", "still signed in while reaching");
        // A's poll lands mid-switch: the status keeps saying where it is going
        let _ = state.update(Message::StatusPushed(status(8)));
        assert_eq!(state.status, "Reaching http://b…");
        let _ = state.update(Message::ConnectFailed {
            generation: state.connect_generation,
            error: "error sending request for url (http://b/v1/status)".into(),
        });
        assert!(state.connected && !state.connecting);
        assert_eq!(state.status, "Connected · block 8");
        assert_eq!(state.endpoint, "http://a");
        assert!(state.error.is_empty() && state.toast.contains("http://b"));
        assert_eq!(state.account, Some(Some((7, "Grace Hopper".into()))));
    }

    #[test]
    fn taking_up_another_chain_resets_the_last_ones_state() {
        let mut state = on_testkit();
        let _ = state.take_up(backend::Keyring {
            dir: "testkit".into(),
            other_chain: false,
        });
        assert_eq!(
            state.signer_key, "ab",
            "a second node of one network keeps the key"
        );
        assert_eq!(state.active, Some("chat"));
        let _ = state.take_up(backend::Keyring {
            dir: "testkit+200".into(),
            other_chain: true,
        });
        assert!(
            state.signer_key.is_empty(),
            "the seat is locked on a switch"
        );
        assert_eq!(state.account, None, "the account is asked again");
        assert_eq!(state.active, None);
        assert!(state.badges.is_empty());
        assert!(state.other_chain);
        assert_eq!(state.keyring, "testkit+200");
    }

    #[test]
    fn an_account_answer_from_before_a_switch_is_dropped() {
        let mut state = on_testkit();
        state.account = None;
        let _ = state.update(Message::AccountResolved {
            node: "http://old".into(),
            key: "ab".into(),
            account: Some((1, "Stale".into())),
        });
        assert_eq!(state.account, None);
        let _ = state.update(Message::AccountResolved {
            node: "http://a".into(),
            key: "ab".into(),
            account: None,
        });
        assert_eq!(state.account, Some(None));
    }

    #[test]
    fn closing_an_overlay_lets_go_of_what_it_held() {
        use super::super::Popover;
        let mut state = on_testkit();
        let _ = state.update(Message::OpenSpotlight);
        let _ = state.update(Message::SpotlightTyped("chat".into()));
        let _ = state.update(Message::CloseOverlay(Overlay::Spotlight));
        assert!(state.overlay.is_none() && state.spotlight_query.is_empty());
        // Escape on the account menu closes it whichever menu is named
        let _ = state.update(Message::TogglePopover(Popover::Account));
        let _ = state.update(Message::CloseOverlay(Overlay::Menu(Popover::Node)));
        assert!(state.overlay.is_none());
        // one overlay's close leaves another open
        let _ = state.update(Message::OpenSettings);
        let _ = state.update(Message::CloseOverlay(Overlay::Network));
        assert_eq!(state.overlay, Some(Overlay::Settings));
        let _ = state.update(Message::ApproveOpen);
        state.sign_in.unlock_error = "no such code".into();
        let _ = state.update(Message::CloseOverlay(Overlay::Approve));
        assert!(state.overlay.is_none() && state.sign_in.unlock_error.is_empty());
    }

    fn status(height: u64) -> backend::NodeStatus {
        backend::NodeStatus {
            network: "testkit".into(),
            time: 0,
            block_time_ms: 0,
            epoch_length: 0,
            height,
            tip: [0; 32],
            root: abi::Root([0; 32]),
            epoch: 0,
            identity: Vec::new(),
            contract: backend::noded::NODE_CONTRACT,
            genesis: [0; 32],
        }
    }
}
