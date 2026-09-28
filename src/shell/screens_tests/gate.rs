//! The gate of `docs/ax.md` phases 1 and 2: every native screen state the
//! console draws runs through the door's audit, Tab walk included, and any
//! error-severity violation fails. Nothing is excused.
use super::*;
use crate::Secret;
use crate::ax::audit::{self, Violation};
use crate::ui::{Account, Phrase, Recover, Unlock};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

pub(super) type Build = Box<dyn Fn() -> Ducktape>;

fn booted(stage: Stage, key: bool) -> Ducktape {
    let (mut state, _) = Ducktape::boot();
    // boot reads this machine's recent nodes; the screen states set their own
    state.recent_endpoints.clear();
    state.stage = stage;
    if key {
        state.signer_key = "ab".into();
    }
    state
}

pub(super) fn desk() -> Ducktape {
    let mut state = booted(Stage::Desk, true);
    state.connected = true;
    state.network = "testkit".into();
    state.connected_rpc = "http://127.0.0.1:1".into();
    state.recent_endpoints = vec![crate::backend::RecentEndpoint {
        url: "http://127.0.0.1:1".into(),
        network: "testkit".into(),
        founded: 1,
        other_chain: false,
    }];
    state
}

fn on_desk(overlay: crate::Overlay) -> Build {
    Box::new(move || {
        let mut state = desk();
        state.overlay = Some(overlay);
        state
    })
}

/// Every screen state, with whether it is a launcher screen (AX-018).
pub(super) fn matrix() -> Vec<(&'static str, bool, Build)> {
    let words = || Secret::from(String::from("canoe pond forest"));
    let passkey = || crate::ui::task::Task::<()>::none().abortable().1;
    vec![
        ("connect", true, Box::new(|| booted(Stage::Connect, false))),
        (
            "connect-error-and-recent",
            true,
            Box::new(|| {
                let mut state = booted(Stage::Connect, false);
                state.endpoint = "127.0.0.1:9000".into();
                state.endpoint_error = "no route to host".into();
                state.recent_endpoints = vec![crate::backend::RecentEndpoint {
                    url: "http://127.0.0.1:9000".into(),
                    network: "testkit".into(),
                    ..Default::default()
                }];
                state
            }),
        ),
        (
            "connect-connecting",
            true,
            Box::new(|| {
                let mut state = booted(Stage::Connect, false);
                state.endpoint = "127.0.0.1:9000".into();
                state.connecting = true;
                state.status = "Connecting to 127.0.0.1:9000…".into();
                state
            }),
        ),
        (
            "sign-in",
            true,
            Box::new(|| booted(Stage::Unlock(Unlock::default()), false)),
        ),
        (
            "sign-in-old-password-error",
            true,
            Box::new(|| {
                let mut state = booted(Stage::Unlock(Unlock::default()), false);
                state.key_exists = true;
                state.sign_in.unlock_error = "wrong password".into();
                state
            }),
        ),
        (
            "sign-in-awaiting",
            true,
            Box::new(|| {
                booted(
                    Stage::Unlock(Unlock {
                        awaiting: true,
                        ..Default::default()
                    }),
                    true,
                )
            }),
        ),
        (
            "recovery",
            true,
            Box::new(move || {
                booted(
                    Stage::Phrase(Phrase {
                        words: words(),
                        ..Default::default()
                    }),
                    true,
                )
            }),
        ),
        (
            "recovery-check",
            true,
            Box::new(move || {
                booted(
                    Stage::Phrase(Phrase {
                        words: words(),
                        quiz: Some([0, 1, 2]),
                        ..Default::default()
                    }),
                    true,
                )
            }),
        ),
        (
            "recover",
            true,
            Box::new(|| booted(Stage::Recover(Recover::default()), true)),
        ),
        (
            "recover-adding",
            true,
            Box::new(|| {
                let mut state = booted(Stage::Recover(Recover::default()), true);
                state.sign_in.unlock_busy = true;
                state
            }),
        ),
        (
            "account-step",
            true,
            Box::new(|| booted(Stage::Account(Account::default()), true)),
        ),
        (
            "account-creating",
            true,
            Box::new(|| {
                let mut state = booted(Stage::Account(Account::default()), true);
                state.sign_in.unlock_busy = true;
                state
            }),
        ),
        (
            "account-passkey-waiting",
            true,
            Box::new(move || {
                booted(
                    Stage::Account(Account {
                        passkey_task: Some(passkey()),
                        ..Default::default()
                    }),
                    true,
                )
            }),
        ),
        (
            "account-passkey-qr",
            true,
            Box::new(move || {
                let step = Account {
                    passkey_task: Some(passkey()),
                    passkey_qr: "https://example.test/passkey".into(),
                    ..Default::default()
                };
                step.passkey_phone
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                booted(Stage::Account(step), true)
            }),
        ),
        (
            "link-waiting",
            true,
            Box::new(|| {
                booted(
                    Stage::Account(Account {
                        link_code: "ABCD-EFGH".into(),
                        ..Default::default()
                    }),
                    true,
                )
            }),
        ),
        ("desk-empty", false, Box::new(desk)),
        (
            "desk-toast",
            false,
            Box::new(|| {
                let mut state = desk();
                state.toast = "Copied".into();
                state
            }),
        ),
        (
            "desk-reconnecting",
            false,
            Box::new(|| {
                let mut state = desk();
                state.status_misses = crate::ui::LOST_AFTER;
                state
            }),
        ),
        ("spotlight", false, on_desk(crate::Overlay::Spotlight)),
        ("approve-code", false, on_desk(crate::Overlay::Approve)),
        (
            "approve-confirm",
            false,
            Box::new(|| {
                let mut state = desk();
                state.overlay = Some(crate::Overlay::Approve);
                state.sign_in.approve_found = Some(crate::backend::join::Request {
                    network: "testkit".into(),
                    key: vec![7; 32],
                });
                state
            }),
        ),
        (
            "settings-appearance",
            false,
            on_desk(crate::Overlay::Settings),
        ),
        (
            "settings-notifications",
            false,
            Box::new(|| {
                let mut state = desk();
                state.overlay = Some(crate::Overlay::Settings);
                state.settings_page = crate::ui::SettingsPage::Notifications;
                state
            }),
        ),
        (
            "settings-networks",
            false,
            Box::new(|| {
                let mut state = desk();
                state.overlay = Some(crate::Overlay::Settings);
                state.settings_page = crate::ui::SettingsPage::Networks;
                state
            }),
        ),
        (
            "settings-about",
            false,
            Box::new(|| {
                let mut state = desk();
                state.overlay = Some(crate::Overlay::Settings);
                state.settings_page = crate::ui::SettingsPage::About;
                state
            }),
        ),
        ("network-menu", false, on_desk(crate::Overlay::Network)),
        (
            "node-menu",
            false,
            on_desk(crate::Overlay::Menu(crate::Popover::Node)),
        ),
        (
            "account-menu",
            false,
            on_desk(crate::Overlay::Menu(crate::Popover::Account)),
        ),
        (
            "notifications-menu",
            false,
            on_desk(crate::Overlay::Menu(crate::Popover::Notifications)),
        ),
    ]
}

fn snap(window: &mut Window, cx: &mut gpui_kit::App) -> Vec<crate::ax::AxNode> {
    window.activate_a11y();
    window.render_frame(cx);
    window.render_frame(cx);
    crate::ax::snapshot("shell", window, true)
}

/// The audit of the screen `native` shows now, Tab walk included: each
/// error-severity violation as a line.
pub(super) fn errors(native: &mut VisualTestContext, screen: &str, launcher: bool) -> Vec<String> {
    native.update(snap);
    let report = native.update(|window, cx| {
        let reading = audit::observe(window, cx, true, |_| true, snap);
        audit::audit(&reading, launcher)
    });
    report
        .errors()
        .map(
            |Violation {
                 rule,
                 id,
                 role,
                 name,
                 message,
                 ..
             }| format!("{screen}: {rule} {id} ({role} {name:?}): {message}"),
        )
        .collect()
}

/// A screen state a test built by hand passes the audit too.
pub(super) fn passes(native: &mut VisualTestContext, screen: &str, launcher: bool) {
    let failures = errors(native, screen, launcher);
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

#[gpui_kit::test]
fn every_native_screen_state_passes_the_phase_1_audit(cx: &mut TestAppContext) {
    let _turn = notices();
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let mut failures: Vec<String> = Vec::new();
    for (screen, launcher, build) in matrix() {
        let (_view, mut native) = open(build(), cx);
        failures.extend(errors(&mut native, screen, launcher));
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
