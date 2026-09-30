//! The shell's tab lists and radio groups are one Tab stop each, WAI-ARIA
//! APG's roving tabindex (`a11y::roving`): Tab lands on the chosen item
//! once, the arrows along the composite move the keys to the next item
//! (wrapping), Home and End to the first and the last. Settings' pages and
//! its radios open or pick what the arrows reach, as a view's do; the
//! Programs rail only moves the keys, and Return opens the program.
use super::*;
use crate::Overlay;

fn keyed(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
}

/// The focused node's door id, and whether it is selected or checked.
fn focus(native: &mut VisualTestContext) -> (String, bool) {
    let nodes = native.update(draw);
    let has = |node: &serde_json::Value, state: &str| {
        node["state"]
            .as_array()
            .is_some_and(|states| states.iter().any(|it| it == state))
    };
    let node = nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| has(node, "focused"))
        .unwrap_or_else(|| panic!("nothing focused in {nodes}"));
    let picked = has(node, "selected") || has(node, "checked");
    (node["id"].as_str().unwrap().to_owned(), picked)
}

/// Every stop Tab lands on, once round from where the keys are.
fn tab_round(native: &mut VisualTestContext) -> Vec<String> {
    let mut stops: Vec<String> = Vec::new();
    for _ in 0..60 {
        native.simulate_keystrokes("tab");
        let (id, _) = focus(native);
        if stops.contains(&id) {
            return stops;
        }
        stops.push(id);
    }
    panic!("Tab never came back round: {stops:?}");
}

/// Tab until the keys are on `id`.
fn tab_to(native: &mut VisualTestContext, id: &str) {
    for _ in 0..60 {
        native.simulate_keystrokes("tab");
        if focus(native).0 == id {
            return;
        }
    }
    panic!("Tab never reached {id}");
}

/// Each key, and the item it leaves the keys on, picked or not.
fn arrows(native: &mut VisualTestContext, steps: &[(&str, &str, bool)]) {
    for (key, id, picked) in steps {
        native.simulate_keystrokes(key);
        assert_eq!(focus(native), ((*id).to_owned(), *picked), "after {key}");
    }
}

/// The stops of `stops` whose id starts `shell:<prefix>`.
fn under<'a>(stops: &'a [String], prefix: &str) -> Vec<&'a str> {
    stops
        .iter()
        .map(String::as_str)
        .filter(|id| id.starts_with(&format!("shell:{prefix}")))
        .collect()
}

/// Settings' pages, a column: one Tab stop on the page shown; up and down
/// open the next page and take the keys along, wrapping at the ends; Home
/// and End open the first and the last.
#[gpui_kit::test]
fn settings_pages_are_one_tab_stop_whose_arrows_open_the_next(cx: &mut TestAppContext) {
    keyed(cx);
    let mut state = gate::desk();
    state.overlay = Some(Overlay::Settings);
    let (_view, mut native) = open(state, cx);
    native.update(draw);
    let stops = tab_round(&mut native);
    assert_eq!(
        under(&stops, "settings/"),
        ["shell:settings/appearance"],
        "{stops:?}"
    );
    tab_to(&mut native, "shell:settings/appearance");
    arrows(
        &mut native,
        &[
            ("down", "shell:settings/notifications", true),
            ("down", "shell:settings/networks", true),
            ("up", "shell:settings/notifications", true),
            ("end", "shell:settings/about", true),
            ("down", "shell:settings/appearance", true),
            ("up", "shell:settings/about", true),
            ("home", "shell:settings/appearance", true),
        ],
    );
}

/// A radio group of Settings, a row: one Tab stop on the chosen choice;
/// right and left pick the next choice and take the keys along, wrapping
/// at the ends; Home and End pick the first and the last. The Burst limit
/// and each view's group, on the Notifications page, are one stop each too.
#[gpui_kit::test]
fn a_settings_radio_group_is_one_tab_stop_whose_arrows_pick(cx: &mut TestAppContext) {
    keyed(cx);
    let mut state = gate::desk();
    state.overlay = Some(Overlay::Settings);
    state.appearance = crate::Appearance::Dark;
    state.roster = crate::runtime::Roster::listing(&["gate-a", "gate-b"]);
    let (view, mut native) = open(state, cx);
    native.update(draw);
    let stops = tab_round(&mut native);
    assert_eq!(under(&stops, "theme/"), ["shell:theme/Dark"], "{stops:?}");
    tab_to(&mut native, "shell:theme/Dark");
    arrows(
        &mut native,
        &[
            ("right", "shell:theme/System", true),
            ("right", "shell:theme/Light", true),
            ("left", "shell:theme/System", true),
            ("home", "shell:theme/Light", true),
            ("end", "shell:theme/System", true),
        ],
    );
    let appearance = view.read_with(&native, |view, cx| view.model.read(cx).state.appearance);
    assert_eq!(
        appearance,
        crate::Appearance::System,
        "the arrows picked it"
    );
    // the Notifications page: the burst limit and one group per view
    tab_to(&mut native, "shell:settings/appearance");
    arrows(
        &mut native,
        &[("down", "shell:settings/notifications", true)],
    );
    let stops = tab_round(&mut native);
    for group in [
        "notify/burst/",
        "notify/view/gate-a/",
        "notify/view/gate-b/",
    ] {
        assert_eq!(under(&stops, group).len(), 1, "{group}: {stops:?}");
    }
}

/// The Programs rail, a row: one Tab stop, on the front window's program
/// or else the first; right and left move the keys to the next program,
/// wrapping at the ends, Home and End to the first and the last, and open
/// nothing. Tab back into the rail lands on its stop again, not on the
/// tab the arrows left.
#[gpui_kit::test]
fn the_programs_rail_is_one_tab_stop_whose_arrows_move_the_keys(cx: &mut TestAppContext) {
    keyed(cx);
    let mut state = gate::desk();
    state.roster = crate::runtime::Roster::listing(&["gate-a", "gate-b", "gate-c"]);
    let (view, mut native) = open(state, cx);
    // gpui reports focus moves only in the active window: the rail hears
    // the keys leave it from one
    super::activate(&mut native);
    native.update(draw);
    let stops = tab_round(&mut native);
    assert_eq!(under(&stops, "rail/"), ["shell:rail/gate-a"], "{stops:?}");
    tab_to(&mut native, "shell:rail/gate-a");
    let panes = |native: &mut VisualTestContext| {
        view.read_with(native, |view, cx| {
            let layout = view.layout(cx);
            layout
                .panes
                .iter()
                .map(|pane| pane.module)
                .collect::<Vec<_>>()
        })
    };
    let before = panes(&mut native);
    arrows(
        &mut native,
        &[
            ("right", "shell:rail/gate-b", false),
            ("end", "shell:rail/gate-c", false),
            ("right", "shell:rail/gate-a", false),
            ("left", "shell:rail/gate-c", false),
            ("home", "shell:rail/gate-a", false),
            ("right", "shell:rail/gate-b", false),
        ],
    );
    assert_eq!(panes(&mut native), before, "the arrows opened nothing");
    native.simulate_keystrokes("tab");
    native.simulate_keystrokes("shift-tab");
    assert_eq!(focus(&mut native).0, "shell:rail/gate-a");
}
