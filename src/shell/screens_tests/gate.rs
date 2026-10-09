//! The gate of `docs/ax.md` phases 1 and 2: every native screen state the
//! console draws runs through the door's audit, Tab walk included, and any
//! error-severity violation fails. Nothing is excused.
use super::*;
use crate::ax::audit::{self, Violation};
use crate::shell::entities::{AccountStep, Overlay, Popover, Screen, Secret, SettingsPage};
use crate::shell::layers::tests::Seed;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

pub(super) type Build = Box<dyn Fn() -> Scene>;

/// A screen state with nothing open over it.
fn plain(build: impl Fn() -> Seed + 'static) -> Build {
    Box::new(move || build().into())
}

/// A screen state with `overlay` open over its desk.
fn over(overlay: Overlay, build: impl Fn() -> Seed + 'static) -> Build {
    Box::new(move || (build(), overlay).into())
}

fn booted(screen: Screen, key: bool) -> Seed {
    let mut seed = Seed::boot();
    // boot reads this machine's recent nodes; the screen states set their own
    seed.session.recent_endpoints.clear();
    // and hands over the app's notification centre and roster, which every
    // test shares: a screen state gets its own
    seed.center = Default::default();
    seed.roster = Default::default();
    seed.screen = screen;
    if key {
        seed.account.signer_key = "ab".into();
    }
    seed
}

const ACCOUNT: Screen = Screen::Account {
    step: AccountStep::Name,
};

/// The layout step, asked before the desk on a device with none picked.
pub(super) fn layout_step() -> Seed {
    booted(Screen::Layout, true)
}

pub(super) fn desk() -> Seed {
    let mut seed = booted(Screen::Desk, true);
    seed.session.connected = true;
    seed.session.network = "testkit".into();
    seed.session.connected_rpc = "http://127.0.0.1:1".into();
    seed.session.recent_endpoints = vec![crate::backend::RecentEndpoint {
        url: "http://127.0.0.1:1".into(),
        network: "testkit".into(),
        founded: 1,
        other_chain: false,
    }];
    seed
}

fn on_desk(overlay: Overlay) -> Build {
    over(overlay, desk)
}

/// A desk that lists two programs: the bar's rail is a tab list the
/// arrows move along, and Settings' Notifications page has a radio group
/// for each.
fn two_programs() -> Seed {
    let mut seed = desk();
    seed.roster = crate::runtime::Roster::listing(&["gate-a", "gate-b"]);
    seed
}

/// `build`'s state in the sidebar layout.
fn in_sidebar(build: fn() -> Seed) -> impl Fn() -> Seed {
    move || {
        let mut seed = build();
        seed.prefs.layout = Some(crate::backend::Layout::Sidebar);
        seed
    }
}

/// A sidebar listing a titled window under its program, a program standing
/// for its one window, Help and an empty window after the hairline. (Two
/// windows of one program still loading would share their loading body's
/// ids, AX-015, in either layout.)
fn sidebar_windows() -> Seed {
    use crate::ui::layout::{EMPTY, HELP};
    super::sidebar::sidebar(
        &["gate-a", "gate-b"],
        &[
            ("gate-a", Some("# general")),
            ("gate-b", None),
            (HELP, None),
            (EMPTY, None),
        ],
    )
}

/// A desk with a window on it, which Search opens over: its rows for the window
/// (Fill, Move or size) are there, each reporting its chord (AX-114).
pub(super) fn spotlight_over_window() -> Seed {
    let mut seed = desk();
    let mut layout = crate::ui::layout::Layout::default();
    layout.split(crate::ui::layout::EMPTY);
    layout.measure((1280., 764.));
    layout.settle();
    layout.initialized = true;
    seed.layout = Some(layout);
    seed
}

/// Every screen state, with whether it is a launcher screen (AX-018).
pub(super) fn matrix() -> Vec<(&'static str, bool, Build)> {
    let words = || Secret::from(String::from("canoe pond forest"));
    vec![
        ("connect", true, plain(|| booted(Screen::Connect, false))),
        (
            "connect-error-and-recent",
            true,
            plain(|| {
                let mut seed = booted(Screen::Connect, false);
                seed.session.endpoint = "127.0.0.1:9000".into();
                seed.session.endpoint_error = "no route to host".into();
                seed.session.recent_endpoints = vec![crate::backend::RecentEndpoint {
                    url: "http://127.0.0.1:9000".into(),
                    network: "testkit".into(),
                    ..Default::default()
                }];
                seed
            }),
        ),
        (
            "connect-connecting",
            true,
            plain(|| {
                let mut seed = booted(Screen::Connect, false);
                seed.session.endpoint = "127.0.0.1:9000".into();
                seed.session.connecting = true;
                seed.session.reaching = "127.0.0.1:9000".into();
                seed
            }),
        ),
        (
            "sign-in",
            true,
            plain(|| booted(Screen::Unlock { awaiting: false }, false)),
        ),
        (
            "sign-in-old-password-error",
            true,
            plain(|| {
                let mut seed = booted(Screen::Unlock { awaiting: false }, false);
                seed.account.key_exists = true;
                seed.account.error = "wrong password".into();
                seed
            }),
        ),
        (
            "sign-in-awaiting",
            true,
            plain(|| booted(Screen::Unlock { awaiting: true }, true)),
        ),
        (
            "recovery",
            true,
            plain(move || {
                let mut seed = booted(Screen::Phrase { quiz: None }, true);
                seed.phrase = Some(words());
                seed
            }),
        ),
        (
            "recovery-check",
            true,
            plain(move || {
                let mut seed = booted(
                    Screen::Phrase {
                        quiz: Some([0, 1, 2]),
                    },
                    true,
                );
                seed.phrase = Some(words());
                seed
            }),
        ),
        ("recover", true, plain(|| booted(Screen::Recover, true))),
        (
            "recover-adding",
            true,
            plain(|| {
                let mut seed = booted(Screen::Recover, true);
                seed.account.busy = true;
                seed
            }),
        ),
        ("account-step", true, plain(|| booted(ACCOUNT, true))),
        (
            "account-creating",
            true,
            plain(|| {
                let mut seed = booted(ACCOUNT, true);
                seed.account.busy = true;
                seed
            }),
        ),
        (
            "account-passkey-waiting",
            true,
            plain(move || {
                let mut seed = booted(
                    Screen::Account {
                        step: AccountStep::Passkey,
                    },
                    true,
                );
                seed.account.passkey_waiting = true;
                seed
            }),
        ),
        (
            "account-passkey-qr",
            true,
            plain(move || {
                let mut seed = booted(
                    Screen::Account {
                        step: AccountStep::Passkey,
                    },
                    true,
                );
                seed.account.passkey_waiting = true;
                seed.account.passkey_qr = Some("https://example.test/passkey".into());
                seed
            }),
        ),
        (
            "link-waiting",
            true,
            plain(|| {
                let mut seed = booted(
                    Screen::Account {
                        step: AccountStep::Link,
                    },
                    true,
                );
                seed.account.link_code = "ABCD-EFGH".into();
                seed
            }),
        ),
        ("layout-step", true, plain(layout_step)),
        ("desk-empty", false, plain(desk)),
        (
            "desk-empty-a-program",
            false,
            plain(|| {
                let mut seed = desk();
                seed.roster = crate::runtime::Roster::listing(&["gate-program"]);
                seed
            }),
        ),
        ("desk-two-programs", false, plain(two_programs)),
        (
            "desk-toast",
            false,
            plain(|| {
                let mut seed = desk();
                seed.toast = "Copied".into();
                seed
            }),
        ),
        (
            "desk-reconnecting",
            false,
            plain(|| {
                let mut seed = desk();
                seed.session.reconnecting = true;
                seed
            }),
        ),
        ("spotlight", false, on_desk(Overlay::Spotlight)),
        (
            "spotlight-over-window",
            false,
            over(Overlay::Spotlight, spotlight_over_window),
        ),
        ("approve-code", false, on_desk(Overlay::Approve)),
        (
            "approve-confirm",
            false,
            over(Overlay::Approve, || {
                let mut seed = desk();
                let key = vec![7; 32];
                seed.account.approve = Some(crate::backend::join::fingerprint(&key));
                seed.found = Some(crate::backend::join::Request {
                    network: "testkit".into(),
                    key,
                });
                seed
            }),
        ),
        (
            "settings-appearance",
            false,
            on_desk(Overlay::Settings(SettingsPage::Appearance)),
        ),
        (
            "settings-notifications",
            false,
            over(Overlay::Settings(SettingsPage::Notifications), two_programs),
        ),
        (
            "settings-networks",
            false,
            on_desk(Overlay::Settings(SettingsPage::Networks)),
        ),
        (
            "settings-about",
            false,
            on_desk(Overlay::Settings(SettingsPage::About)),
        ),
        ("network-menu", false, on_desk(Overlay::Network)),
        ("node-menu", false, on_desk(Overlay::Menu(Popover::Node))),
        (
            "account-menu",
            false,
            on_desk(Overlay::Menu(Popover::Account)),
        ),
        (
            "notifications-menu",
            false,
            on_desk(Overlay::Menu(Popover::Notifications)),
        ),
        (
            "desk-two-programs-sidebar",
            false,
            plain(in_sidebar(two_programs)),
        ),
        ("desk-windows-sidebar", false, plain(sidebar_windows)),
        (
            "settings-appearance-sidebar",
            false,
            over(
                Overlay::Settings(SettingsPage::Appearance),
                in_sidebar(desk),
            ),
        ),
        (
            "settings-notifications-sidebar",
            false,
            over(
                Overlay::Settings(SettingsPage::Notifications),
                in_sidebar(two_programs),
            ),
        ),
        (
            "settings-networks-sidebar",
            false,
            over(Overlay::Settings(SettingsPage::Networks), in_sidebar(desk)),
        ),
        (
            "settings-about-sidebar",
            false,
            over(Overlay::Settings(SettingsPage::About), in_sidebar(desk)),
        ),
        (
            "network-menu-sidebar",
            false,
            over(Overlay::Network, in_sidebar(desk)),
        ),
        (
            "node-menu-sidebar",
            false,
            over(Overlay::Menu(Popover::Node), in_sidebar(desk)),
        ),
        (
            "account-menu-sidebar",
            false,
            over(Overlay::Menu(Popover::Account), in_sidebar(desk)),
        ),
        (
            "notifications-menu-sidebar",
            false,
            over(Overlay::Menu(Popover::Notifications), in_sidebar(desk)),
        ),
        (
            "desk-asking",
            false,
            plain(|| {
                let mut seed = desk();
                seed.center.lock().ask_for_test("gate-asking");
                seed.active = Some("gate-asking");
                seed
            }),
        ),
    ]
}

fn snap(window: &mut Window, cx: &mut gpui_kit::App) -> Vec<crate::ax::AxNode> {
    window.activate_a11y();
    window.render_frame(cx);
    // the frame's callbacks (the desk's size, its seed) run before the next
    window.simulate_next_frame(cx);
    window.render_frame(cx);
    crate::ax::snapshot("shell", window, true)
}

/// The audit of the screen `native` shows now, Tab walk included: each
/// error-severity violation as a line. The walk's arrows pick Settings'
/// choices, and a pick saves: the prefs file goes back as it was once the
/// walk ends, as the door's does (`Kept`), so the next state boots on it
/// as this one did. (Only the file: the screen keeps the prefs it was
/// drawn with, which a seed may hold without the file.)
pub(super) fn errors(native: &mut VisualTestContext, screen: &str, launcher: bool) -> Vec<String> {
    native.update(snap);
    let prefs = crate::backend::read_prefs().expect("a test's prefs");
    let report = native.update(|window, cx| {
        let mut reading = audit::observe(window, cx, "shell", true, |_| true, snap);
        reading.chords = crate::shell::chords();
        audit::audit(&reading, launcher)
    });
    crate::backend::edit_prefs(|now| *now = prefs);
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
    let mut failures: Vec<String> = Vec::new();
    for (screen, launcher, build) in matrix() {
        let (_view, mut native) = open(build(), cx);
        failures.extend(errors(&mut native, screen, launcher));
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
    // every walk put back the picks it saved: each state booted on the
    // prefs this test's thread started with
    assert_eq!(crate::backend::read_prefs().unwrap(), serde_json::json!({}));
}

/// The walk probes the shell's tab lists and radio groups as a view's
/// (AX-107): the layout step's cards, the bar's rail and the sidebar's
/// list, Settings' pages and each of its radio groups, once each, and each
/// passes.
#[gpui_kit::test]
fn the_walk_probes_every_tab_list_and_radio_group_of_the_shell(cx: &mut TestAppContext) {
    let wanted: [(&str, &[&str]); 6] = [
        ("layout-step", &["shell:layout-cards"]),
        ("desk-two-programs", &["shell:rail-rows"]),
        ("desk-two-programs-sidebar", &["shell:rail-rows"]),
        (
            "desk-windows-sidebar",
            &["shell:empty-window/rows", "shell:rail-rows"],
        ),
        (
            "settings-appearance",
            &["shell:settings-nav", "shell:theme", "shell:layout"],
        ),
        (
            "settings-notifications",
            &[
                "shell:settings-nav",
                "shell:notify/front",
                "shell:notify/burst",
                "shell:notify/view/gate-a",
                "shell:notify/view/gate-b",
            ],
        ),
    ];
    let matrix = matrix();
    for (screen, composites) in wanted {
        let (_, _, build) = matrix.iter().find(|(name, ..)| *name == screen).unwrap();
        let (_view, mut native) = open(build(), cx);
        native.update(snap);
        let reading =
            native.update(|window, cx| audit::observe(window, cx, "shell", true, |_| true, snap));
        let mut probed: Vec<_> = reading
            .arrows
            .iter()
            .map(|arrows| arrows.composite.id.as_str())
            .collect();
        probed.sort_unstable();
        let mut composites = composites.to_vec();
        composites.sort_unstable();
        assert_eq!(probed, composites, "{screen}");
        let report = audit::audit(&reading, false);
        assert_eq!(report.applicable["AX-107"], composites.len(), "{screen}");
        assert!(
            report.violations.iter().all(|v| v.rule != "AX-107"),
            "{screen}: {:#?}",
            report.violations
        );
    }
}

/// The rail's tab list and its chosen tab both offer focus and are one Tab
/// stop, so the desk has more nodes offering focus than stops: the walk
/// still closes the first time the focus is back on the first stop
/// (docs/ax.md §1.1), one press past the last stop, and never passes the
/// first stop again.
#[gpui_kit::test]
fn the_walk_closes_the_first_time_the_focus_is_back_on_the_first_stop(cx: &mut TestAppContext) {
    let (_view, mut native) = open(plain(two_programs)(), cx);
    native.update(snap);
    let reading =
        native.update(|window, cx| audit::observe(window, cx, "shell", true, |_| true, snap));
    let walked: Vec<String> = reading.snapshots[1..]
        .iter()
        .map(|nodes| {
            let ids: Vec<&str> = nodes
                .iter()
                .filter(|node| node.state.contains(&"focused"))
                .map(|node| node.id.as_str())
                .collect();
            ids.join(",")
        })
        .collect();
    let offering = reading.snapshots[0]
        .iter()
        .filter(|node| node.actions.contains(&"focus"))
        .count();
    let stops: std::collections::BTreeSet<&String> = walked.iter().collect();
    assert!(offering > stops.len(), "{offering} offer focus: {walked:?}");
    let first = &walked[0];
    assert_eq!(
        walked.last(),
        Some(first),
        "closes on the first stop: {walked:?}"
    );
    assert_eq!(walked.len(), stops.len() + 1, "one lap: {walked:?}");
}

/// The walk's arrows pick Settings' choices, and a pick saves; the door
/// puts back what they changed. From no prefs, after a walk of the
/// Notifications page no view is answered, the one asking still asks, and
/// the prefs are still none.
#[gpui_kit::test]
async fn a_door_walk_leaves_the_prefs_as_it_found_them(cx: &mut TestAppContext) {
    let matrix = matrix();
    let (_, _, build) = matrix
        .iter()
        .find(|(name, ..)| *name == "settings-notifications")
        .unwrap();
    let scene = build();
    scene.0.center.lock().ask_for_test("gate-a");
    let center = scene.0.center.clone();
    let prefs = crate::backend::read_prefs().unwrap();
    assert_eq!(prefs, serde_json::json!({}));
    let (_view, native) = open(scene, cx);
    let window = gpui_kit::VisualContext::window_handle(&native);
    let report = crate::ax::audit::tests::door_audit(window, None, cx).await;
    // the four radio groups and the page tabs were probed
    assert!(
        report["applicable"]["AX-107"].as_u64() >= Some(5),
        "{}",
        report["applicable"]
    );
    assert_eq!(crate::backend::read_prefs().unwrap(), prefs);
    assert!(crate::runtime::notify::Settings::load().views.is_empty());
    assert!(center.lock().asking("gate-a"));
}

/// Search over a window offers Fill and Move or size for it, each saying
/// the chord Help lists (AX-114), and the audit told the chords passes.
#[gpui_kit::test]
fn search_over_a_window_reports_the_chords_of_its_window_rows(cx: &mut TestAppContext) {
    // narrowed to the window rows: programs other tests list share the list
    let (_view, mut native) = open((spotlight_over_window(), Overlay::Spotlight), cx);
    native.update(|window, cx| type_into("spotlight/search", "window", window, cx));
    let nodes = native.update(draw);
    for (name, key) in [("Fill window", "⇧↩"), ("Move or size window", "⇧M")] {
        assert_eq!(
            find(&nodes, "ListBoxOption", name)["keyboard_shortcut"],
            chord_label(key),
            "{name}"
        );
    }
    let report = native.update(|window, cx| {
        let mut reading = audit::observe(window, cx, "shell", true, |_| true, snap);
        reading.chords = crate::shell::chords();
        audit::audit(&reading, false)
    });
    assert!(
        report.errors().next().is_none() && report.violations.iter().all(|v| v.rule != "AX-114"),
        "{:#?}",
        report.violations
    );
    assert!(report.applicable["AX-114"] >= 2, "{:?}", report.applicable);
}

/// The walk leaves a stop Tab did not move off by Esc, Tab only outside a
/// modal (docs/ax.md §1.1): Search, whose one Tab stop Tab comes back to,
/// stays open the whole walk instead of closing on the Esc.
#[gpui_kit::test]
fn the_walk_presses_no_escape_under_a_modal(cx: &mut TestAppContext) {
    let (_view, mut native) = open(on_desk(Overlay::Spotlight)(), cx);
    native.update(snap);
    let reading =
        native.update(|window, cx| audit::observe(window, cx, "shell", true, |_| true, snap));
    assert!(reading.modal && reading.snapshots.len() > 2);
    for (n, nodes) in reading.snapshots.iter().enumerate() {
        assert!(
            nodes
                .iter()
                .any(|node| node.role == "Dialog" && node.name == "Search"),
            "step {n}: Search is gone"
        );
    }
}
