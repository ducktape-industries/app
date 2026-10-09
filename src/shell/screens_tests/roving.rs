//! The shell's tab lists and radio groups are one Tab stop each, WAI-ARIA
//! APG's roving tabindex (`a11y::roving`): Tab lands on the chosen item
//! once, the arrows along the composite move the keys to the next item
//! (wrapping), Home and End to the first and the last. Settings' pages and
//! its radios open or pick what the arrows reach, as a view's do; the
//! Programs rail only moves the keys, and Return opens the program.
use super::*;

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
    let settings = Overlay::Settings(SettingsPage::Appearance);
    let (_view, mut native) = open((gate::desk(), settings), cx);
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
    let mut seed = gate::desk();
    seed.prefs.appearance = crate::backend::Appearance::Dark;
    seed.roster = crate::runtime::Roster::listing(&["gate-a", "gate-b"]);
    let settings = Overlay::Settings(SettingsPage::Appearance);
    let (view, mut native) = open((seed, settings), cx);
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
    let appearance = view.read_with(&native, |view, cx| view.app.prefs.read(cx).get().appearance);
    assert_eq!(
        appearance,
        crate::backend::Appearance::System,
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

/// Settings' Layout row shows Menu bar chosen while none is picked, and a
/// click on Sidebar saves it.
#[gpui_kit::test]
fn a_click_on_the_settings_layout_row_saves_it(cx: &mut TestAppContext) {
    use crate::backend::{Layout, load_layout};
    let settings = Overlay::Settings(SettingsPage::Appearance);
    let (view, mut native) = open((gate::desk(), settings), cx);
    let nodes = native.update(|window, cx| {
        draw(window, cx);
        serde_json::to_value(crate::ax::snapshot("shell", window, true)).unwrap()
    });
    let checked = |name| {
        find(&nodes, "RadioButton", name)["state"]
            .as_array()
            .is_some_and(|states| states.iter().any(|it| it == "checked"))
    };
    assert_eq!((checked("Menu bar"), checked("Sidebar")), (true, false));
    let sidebar = find(&nodes, "RadioButton", "Sidebar");
    let at = |n: usize| sidebar["bounds"][n].as_f64().unwrap() as f32;
    native.simulate_click(
        gpui_kit::point(px((at(0) + at(2)) / 2.), px((at(1) + at(3)) / 2.)),
        gpui_kit::Modifiers::none(),
    );
    let app = entities(&view, &mut native);
    let layout = native.update(|_, cx| app.prefs.read(cx).get().layout);
    assert_eq!(layout, Some(Layout::Sidebar));
    assert_eq!(
        load_layout(),
        Some(Layout::Sidebar),
        "never reached the file"
    );
}

/// The layout step's cards, a row: one Tab stop, on the card picked (Menu
/// bar before any pick); right and left pick the other and take the keys
/// along, wrapping, Home and End the first and the last, and none of it
/// saves. Enter on the group continues: the pick saved, the desk.
#[gpui_kit::test]
fn the_layout_cards_are_one_tab_stop_whose_arrows_pick_and_enter_continues(
    cx: &mut TestAppContext,
) {
    use crate::backend::{Layout, load_layout};
    let (view, mut native) = open(gate::layout_step(), cx);
    native.update(draw);
    let stops = tab_round(&mut native);
    assert_eq!(
        under(&stops, "layout-cards/"),
        ["shell:layout-cards/Menu bar"],
        "{stops:?}"
    );
    tab_to(&mut native, "shell:layout-cards/Menu bar");
    arrows(
        &mut native,
        &[
            ("right", "shell:layout-cards/Sidebar", true),
            ("right", "shell:layout-cards/Menu bar", true),
            ("left", "shell:layout-cards/Sidebar", true),
            ("home", "shell:layout-cards/Menu bar", true),
            ("end", "shell:layout-cards/Sidebar", true),
        ],
    );
    assert_eq!(load_layout(), None, "an arrow saved the pick");
    native.simulate_keystrokes("enter");
    let app = entities(&view, &mut native);
    let now = native.update(|_, cx| (*app.screen.read(cx).get(), app.prefs.read(cx).get().layout));
    assert_eq!(now, (Screen::Desk, Some(Layout::Sidebar)));
    assert_eq!(load_layout(), Some(Layout::Sidebar));
}

/// The Programs rail, a row: one Tab stop, on the front window's program
/// or else the first; right and left move the keys to the next program,
/// wrapping at the ends, Home and End to the first and the last, and open
/// nothing. Tab back into the rail lands on its stop again, not on the
/// tab the arrows left.
#[gpui_kit::test]
fn the_programs_rail_is_one_tab_stop_whose_arrows_move_the_keys(cx: &mut TestAppContext) {
    let mut seed = gate::desk();
    seed.roster = crate::runtime::Roster::listing(&["gate-a", "gate-b", "gate-c"]);
    let (view, mut native) = open(seed, cx);
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

/// The sidebar's list, a column: one Tab stop, on the window in front's
/// row; up and down move the keys along every row, programs and windows,
/// wrapping at the ends, Home and End to the first and the last, and open
/// nothing. Return on a window's row brings that window to the front; on a
/// program's row, it opens the program.
#[gpui_kit::test]
fn the_sidebar_is_one_tab_stop_whose_up_and_down_move_the_keys(cx: &mut TestAppContext) {
    use crate::ui::layout::HELP;
    let seed = super::sidebar::sidebar(
        &["rove-a", "rove-b"],
        &[("rove-a", Some("one")), ("rove-a", None), (HELP, None)],
    );
    let (view, mut native) = open(seed, cx);
    super::activate(&mut native);
    native.update(draw);
    let at: Vec<u64> = view.read_with(&native, |view, cx| {
        view.layout(cx)
            .panes
            .iter()
            .map(|pane| pane.instance)
            .collect()
    });
    let (one, two, help) = (
        format!("shell:rail/rove-a/{}", at[0]),
        format!("shell:rail/rove-a/{}", at[1]),
        format!("shell:rail-help/{}", at[2]),
    );
    let stops = tab_round(&mut native);
    let listed: Vec<&String> = stops
        .iter()
        .filter(|id| id.starts_with("shell:rail/") || id.starts_with("shell:rail-help/"))
        .collect();
    assert_eq!(listed, [&help], "{stops:?}");
    tab_to(&mut native, &help);
    let panes = |native: &mut VisualTestContext| {
        view.read_with(native, |view, cx| {
            let layout = view.layout(cx);
            (layout.panes.len(), layout.focused)
        })
    };
    let before = panes(&mut native);
    arrows(
        &mut native,
        &[
            ("up", "shell:rail/rove-b", false),
            ("up", &two, false),
            ("up", &one, false),
            ("home", "shell:rail/rove-a", false),
            ("up", &help, true),
            ("down", "shell:rail/rove-a", false),
            ("end", &help, true),
            ("home", "shell:rail/rove-a", false),
            ("down", &one, false),
        ],
    );
    assert_eq!(panes(&mut native), before, "the arrows opened nothing");
    // a whole press of Return: a click is its key-up
    let enter = |native: &mut VisualTestContext| {
        let keystroke = gpui_kit::Keystroke::parse("enter").unwrap();
        native.simulate_event(gpui_kit::KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        native.simulate_event(gpui_kit::KeyUpEvent { keystroke });
    };
    enter(&mut native);
    assert_eq!(
        panes(&mut native),
        (3, 0),
        "Return brought the window forward"
    );
    // the keys went with the window: Tab back to the list, on its row now
    tab_to(&mut native, &one);
    arrows(
        &mut native,
        &[("down", &two, false), ("down", "shell:rail/rove-b", false)],
    );
    enter(&mut native);
    let modules: Vec<&str> = view.read_with(&native, |view, cx| {
        view.layout(cx)
            .panes
            .iter()
            .map(|pane| pane.module)
            .collect()
    });
    assert!(modules.contains(&"rove-b"), "Return opened it: {modules:?}");
}
