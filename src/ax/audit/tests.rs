//! One passing and one failing case per rule, over hand-built nodes.
use super::*;
use Severity::Warn;
use gpui_kit::accesskit::NodeId;

fn node(id: &str, role: &str, name: &str) -> AxNode {
    AxNode {
        id: format!("w:{id}"),
        role: role.to_owned(),
        name: name.to_owned(),
        description: None,
        value: None,
        state: Vec::new(),
        actions: Vec::new(),
        more: Default::default(),
        scope: "w".to_owned(),
        bounds: None,
        node: NodeId(0),
        synthetic: false,
    }
}

fn button(id: &str, name: &str) -> AxNode {
    let mut node = node(id, "Button", name);
    node.actions = vec!["press", "focus"];
    node.bounds = Some([0, 0, 40, 40]);
    node
}

fn field(id: &str, name: &str) -> AxNode {
    let mut node = node(id, "TextInput", name);
    node.actions = vec!["focus", "set_value", "type"];
    node.value = Some(String::new());
    node
}

fn focused(mut node: AxNode) -> AxNode {
    node.state.push("focused");
    node
}

fn reading(snapshots: Vec<Vec<AxNode>>) -> Reading {
    Reading {
        escape: vec![true; snapshots.len()],
        snapshots,
        ..Default::default()
    }
}

fn one(nodes: Vec<AxNode>) -> Report {
    audit(&reading(vec![nodes]), false)
}

fn fails(report: &Report, rule: &str) -> Vec<String> {
    report
        .violations
        .iter()
        .filter(|violation| violation.rule == rule)
        .map(|violation| violation.id.clone())
        .collect()
}

#[test]
fn ax_001_actionable_nodes_need_a_real_role() {
    let mut bare = node("bare", "GenericContainer", "x");
    bare.actions = vec!["press", "focus"];
    let report = one(vec![button("ok", "Save"), bare]);
    assert_eq!(fails(&report, "AX-001"), ["w:bare"]);
    assert_eq!(report.applicable["AX-001"], 2);
}

#[test]
fn ax_002_press_or_focus_needs_a_name() {
    let report = one(vec![button("ok", "Save"), button("none", "  ")]);
    assert_eq!(fails(&report, "AX-002"), ["w:none"]);
}

#[test]
fn ax_003_a_text_input_is_named_and_its_description_does_not_count() {
    let mut placeholder = field("ph", "");
    placeholder.description = Some("Type here".into());
    let report = one(vec![field("ok", "Node address"), placeholder]);
    assert_eq!(fails(&report, "AX-003"), ["w:ph"]);
}

#[test]
fn ax_004_a_text_input_has_a_value_key_but_a_password_need_not() {
    let mut blank = field("blank", "Phrase");
    blank.value = None;
    let mut masked = field("masked", "Phrase");
    masked.value = Some("•••".into());
    let mut password = node("pw", "PasswordInput", "Password");
    password.actions = vec!["focus", "type"];
    let report = one(vec![masked, blank, password]);
    assert_eq!(fails(&report, "AX-004"), ["w:blank"]);
    assert_eq!(report.applicable["AX-004"], 2);
}

#[test]
fn ax_005_a_name_that_is_its_placeholder_warns() {
    let mut same = field("same", "Search");
    same.description = Some("Search".into());
    let mut other = field("other", "Search");
    other.description = Some("Type a name".into());
    let report = one(vec![same, other]);
    assert_eq!(fails(&report, "AX-005"), ["w:same"]);
    assert_eq!(report.violations[0].severity, Warn);
}

#[test]
fn ax_006_a_glyph_or_the_role_is_not_a_name() {
    let report = one(vec![
        button("ok", "Save"),
        button("glyph", "→"),
        button("role", " button "),
    ]);
    assert_eq!(fails(&report, "AX-006"), ["w:glyph", "w:role"]);
}

#[test]
fn ax_007_images_are_named() {
    let report = one(vec![
        node("ok", "Image", "QR code"),
        node("bare", "Image", ""),
    ]);
    assert_eq!(fails(&report, "AX-007"), ["w:bare"]);
}

#[test]
fn ax_008_containers_that_take_a_name_have_one() {
    let report = one(vec![
        node("ok", "Dialog", "Settings"),
        node("bare", "TabList", ""),
        node("plain", "GenericContainer", ""),
    ]);
    assert_eq!(fails(&report, "AX-008"), ["w:bare"]);
    assert_eq!(report.applicable["AX-008"], 2);
}

#[test]
fn ax_009_a_toggle_says_which_way_it_is() {
    let mut on = node("on", "Switch", "Banners");
    on.state = vec!["checked"];
    let mut both = node("both", "CheckBox", "Two");
    both.state = vec!["checked", "unchecked"];
    let report = one(vec![on, node("none", "MenuItemRadio", "Row"), both]);
    assert_eq!(fails(&report, "AX-009"), ["w:both", "w:none"]);
}

#[test]
fn ax_010_a_combo_box_is_expanded_or_collapsed() {
    let mut shut = node("shut", "ComboBox", "Kind");
    shut.state = vec!["collapsed"];
    let report = one(vec![shut, node("none", "ComboBox", "Kind")]);
    assert_eq!(fails(&report, "AX-010"), ["w:none"]);
}

#[test]
fn ax_011_a_control_without_press_is_disabled() {
    let mut off = node("off", "Button", "Split");
    off.state = vec!["disabled"];
    let mut stuck = node("stuck", "Button", "Split");
    stuck.actions = vec!["focus"];
    let report = one(vec![off, stuck]);
    assert_eq!(fails(&report, "AX-011"), ["w:stuck"]);
}

#[test]
fn ax_012_press_comes_with_focus() {
    let mut mouse_only = node("row", "MenuItem", "Open chat");
    mouse_only.actions = vec!["press"];
    let report = one(vec![button("ok", "Save"), mouse_only]);
    assert_eq!(fails(&report, "AX-012"), ["w:row"]);
}

/// A rich text's clickable ranges are links no element draws, reached by
/// the arrows from the box around the text once Tab has put the keys in
/// it: under a node that offers focus they pass, and the picked one, a
/// drawn node the keys are on, passes as focused. A range under no such
/// box, and a drawn link nothing reaches, fail.
#[test]
fn ax_012_a_texts_links_are_reached_through_its_box() {
    let mut text = node("text", "Group", "Read the docs or the code");
    text.actions = vec!["focus"];
    let link = |id, name: &str, parent: Option<&str>| {
        let mut link = node(id, "Link", name);
        link.actions = vec!["press"];
        link.more.parent = parent.map(|parent| format!("w:{parent}"));
        link
    };
    let mut range = link("text.Link", "the docs", Some("text"));
    range.synthetic = true;
    let picked = focused(link("text.link", "the code", Some("text")));
    let mut loose = link("loose.Link", "the code", None);
    loose.synthetic = true;
    let drawn = link("drawn", "the docs", Some("text"));
    let report = one(vec![text, range, picked, loose, drawn]);
    assert_eq!(fails(&report, "AX-012"), ["w:drawn", "w:loose.Link"]);
}

/// A composite's rows are reached by the arrows from the composite: a row
/// with a press and no focus passes under one that takes focus, as
/// view_wire::audit's Unreachable reads it, and not under one that does not.
#[test]
fn ax_012_a_composites_rows_are_reached_through_it() {
    let row = |id, parent: &str| {
        let mut row = node(id, "ListBoxOption", id);
        row.actions = vec!["press"];
        row.more.parent = Some(format!("w:{parent}"));
        row
    };
    let mut combo = node("combo", "EditableComboBox", "Search");
    combo.actions = vec!["focus", "set_value", "type"];
    let mut active = node("list", "ListBox", "Results");
    active.more.parent = Some("w:combo".into());
    active.more.active_descendant = Some("w:a".into());
    let report = one(vec![
        combo,
        active,
        row("a", "list"),
        node("menu", "Menu", "Results"),
        row("b", "menu"),
        node("group", "Group", "Results"),
        row("c", "group"),
    ]);
    assert_eq!(fails(&report, "AX-012"), ["w:b", "w:c"]);
}

/// A desk window without the keys: its rows are reached by the chord its
/// box names, which hands that window the keys. The shell's own reading: a
/// view's row answers to view_wire::audit, a control's chord presses the
/// control, handing nothing the keys, and in the window with the keys the
/// chord hands it nothing it has not got.
#[test]
fn ax_012_a_shell_row_is_reached_by_the_chord_its_window_names() {
    let row = |id, parent: &str| {
        let mut row = node(id, "ListBoxOption", id);
        row.actions = vec!["press"];
        row.more.parent = Some(format!("w:{parent}"));
        row
    };
    let mut window = node("window", "Group", "Empty");
    window.more.keyboard_shortcut = Some("Ctrl 2".into());
    let mut keyed = node("keyed", "Group", "Empty");
    keyed.more.keyboard_shortcut = Some("Ctrl 1".into());
    let mut keys = focused(field("keys", "Open a program"));
    keys.more.parent = Some("w:keyed".into());
    let bare = node("bare", "Group", "Empty");
    let mut search = button("search", "Search");
    search.more.keyboard_shortcut = Some("Ctrl K".into());
    let mut in_view = row("v", "window");
    in_view.scope = "w/chat".into();
    let report = one(vec![
        window,
        row("a", "window"),
        in_view,
        keyed,
        keys,
        row("d", "keyed"),
        bare,
        row("b", "bare"),
        search,
        row("c", "search"),
    ]);
    assert_eq!(fails(&report, "AX-012"), ["w:b", "w:c", "w:d", "w:v"]);
}

#[test]
fn ax_013_status_and_alert_are_named() {
    let report = one(vec![
        node("ok", "Alert", "wrong password"),
        node("bare", "Status", ""),
    ]);
    assert_eq!(fails(&report, "AX-013"), ["w:bare"]);
}

#[test]
fn ax_014_heading_and_label_are_named() {
    let report = one(vec![
        node("ok", "Heading", "Connect"),
        node("bare", "Label", " "),
    ]);
    assert_eq!(fails(&report, "AX-014"), ["w:bare"]);
}

#[test]
fn ax_015_a_tilde_suffix_is_two_elements_on_one_path() {
    let mut twin = node("row~2", "Button", "Row");
    twin.id = "w:row~2".into();
    let tilde = node("a~b", "Button", "Row");
    let report = one(vec![button("ok", "Save"), twin, tilde]);
    assert_eq!(fails(&report, "AX-015"), ["w:row~2"]);
    assert_eq!(report.applicable["AX-015"], 3);
}

#[test]
fn ax_016_two_press_nodes_in_one_scope_with_one_name_warn() {
    let mut elsewhere = button("c", "Close");
    elsewhere.scope = "w/chat".into();
    let report = one(vec![
        button("a", "Close"),
        button("b", "Close"),
        elsewhere,
        button("d", "Save"),
    ]);
    assert_eq!(fails(&report, "AX-016"), ["w:a", "w:b"]);
}

#[test]
fn ax_017_a_press_target_is_24_px_each_way() {
    let mut thin = button("thin", "x");
    thin.bounds = Some([0, 0, 40, 20]);
    let mut unmeasured = button("none", "y");
    unmeasured.bounds = None;
    let report = one(vec![button("ok", "Save"), thin, unmeasured]);
    assert_eq!(fails(&report, "AX-017"), ["w:thin"]);
    assert_eq!(report.applicable["AX-017"], 2);
}

#[test]
fn ax_018_a_launcher_screen_has_one_shell_heading_when_the_caller_says_so() {
    let one_heading = vec![node("h", "Heading", "Connect"), button("ok", "Go")];
    assert!(fails(&audit(&reading(vec![one_heading.clone()]), true), "AX-018").is_empty());
    let mut view_heading = node("vh", "Heading", "Chat");
    view_heading.scope = "w/chat".into();
    let none = vec![view_heading, button("ok", "Go")];
    assert_eq!(
        fails(&audit(&reading(vec![none.clone()]), true), "AX-018"),
        ["w"]
    );
    let two = vec![node("h", "Heading", "A"), node("h2", "Heading", "B")];
    assert_eq!(fails(&audit(&reading(vec![two]), true), "AX-018"), ["w"]);
    // the desk, or a caller that said nothing: not applicable
    let report = audit(&reading(vec![none.clone()]), false);
    assert!(!report.applicable.contains_key("AX-018") && !report.launcher);
    let mut modal = reading(vec![none]);
    modal.modal = true;
    assert!(!audit(&modal, true).applicable.contains_key("AX-018"));
}

fn walk(steps: Vec<Vec<AxNode>>) -> Report {
    audit(&reading(steps), false)
}

#[test]
fn ax_020_every_tab_lands_on_exactly_one_node() {
    let a = || button("a", "A");
    let b = || button("b", "B");
    let good = walk(vec![
        vec![a(), b()],
        vec![focused(a()), b()],
        vec![a(), focused(b())],
        vec![focused(a()), b()],
    ]);
    assert!(fails(&good, "AX-020").is_empty());
    assert_eq!(good.presses, 3);
    let lost = walk(vec![
        vec![a(), b()],
        vec![focused(a()), b()],
        vec![a(), b()],
    ]);
    assert_eq!(fails(&lost, "AX-020"), ["step 2"]);
    let double = walk(vec![vec![a(), b()], vec![focused(a()), focused(b())]]);
    assert_eq!(fails(&double, "AX-020"), ["step 1"]);
}

#[test]
fn ax_021_every_focus_stop_is_reached() {
    let a = || button("a", "A");
    let b = || button("b", "B");
    let reached = walk(vec![
        vec![a(), b()],
        vec![focused(a()), b()],
        vec![a(), focused(b())],
    ]);
    assert!(fails(&reached, "AX-021").is_empty());
    let skipped = walk(vec![
        vec![a(), b()],
        vec![focused(a()), b()],
        vec![focused(a()), b()],
    ]);
    assert_eq!(fails(&skipped, "AX-021"), ["w:b"]);
}

/// A box the keys went into is reached: a rich text's box has the keys
/// while its picked link, a node offering no focus of its own, is the one
/// focused. A stop beside it or around it that Tab never lands on is not:
/// the keys are on the link's nearest ancestor that offers focus.
#[test]
fn ax_021_a_box_holding_the_keys_through_its_picked_link_is_reached() {
    let card = || {
        let mut card = node("card", "Group", "A note");
        card.actions = vec!["focus"];
        card
    };
    let text = || {
        let mut text = node("text", "Group", "Read the docs");
        text.actions = vec!["focus"];
        text.more.parent = Some("w:card".into());
        text
    };
    let link = || {
        let mut link = node("text.link", "Link", "the docs");
        link.actions = vec!["press"];
        link.more.parent = Some("w:text".into());
        link
    };
    let report = walk(vec![
        vec![card(), text(), link(), button("b", "B")],
        vec![card(), text(), focused(link()), button("b", "B")],
        vec![card(), text(), focused(link()), button("b", "B")],
    ]);
    assert_eq!(fails(&report, "AX-021"), ["w:b", "w:card"]);
}

/// A stop the state opened on and Tab never lands on again is not reached.
#[test]
fn ax_021_where_the_state_opened_does_not_count() {
    let a = || button("a", "A");
    let b = || button("b", "B");
    let report = walk(vec![
        vec![focused(a()), b()],
        vec![a(), focused(b())],
        vec![a(), focused(b())],
    ]);
    assert_eq!(fails(&report, "AX-021"), ["w:a"]);
}

/// A menu Tab leaves closes (a Dialog or a Menu hanging from the bar): the
/// control it opened on had the keys as it opened, and no Tab comes back
/// to it. One that stays open is held to the walk like anything else, and
/// so is what goes with any other box.
#[test]
fn ax_021_a_menu_the_walk_closed_had_the_keys_where_it_opened() {
    let walked = |role: &str, closes: bool| {
        let menu = || node("menu", role, "Account");
        let inside = |id, name| {
            let mut node = button(id, name);
            node.more.parent = Some("w:menu".into());
            node
        };
        let bar = || button("bar", "Search");
        let last = match closes {
            true => vec![focused(bar())],
            false => vec![
                menu(),
                inside("a", "Lock"),
                inside("b", "Log out"),
                focused(bar()),
            ],
        };
        walk(vec![
            vec![
                menu(),
                focused(inside("a", "Lock")),
                inside("b", "Log out"),
                bar(),
            ],
            vec![
                menu(),
                inside("a", "Lock"),
                focused(inside("b", "Log out")),
                bar(),
            ],
            last,
        ])
    };
    for role in ["Dialog", "Menu"] {
        assert!(fails(&walked(role, true), "AX-021").is_empty(), "{role}");
        assert_eq!(fails(&walked(role, false), "AX-021"), ["w:a"], "{role}");
    }
    assert_eq!(fails(&walked("Group", true), "AX-021"), ["w:a"]);
}

/// A combo box never shows as focused: gpui moves the tree's focus to its
/// active row. It holds the keys while it names one.
#[test]
fn ax_021_a_combo_box_holds_the_keys_while_its_row_is_active() {
    let combo = |row: Option<&str>| {
        let mut combo = node("combo", "EditableComboBox", "Search");
        combo.actions = vec!["focus", "set_value", "type"];
        combo.more.active_descendant = row.map(str::to_owned);
        combo
    };
    let a = || button("a", "A");
    let held = walk(vec![
        vec![combo(None), a()],
        vec![combo(Some("w:row")), a()],
        vec![combo(None), focused(a())],
    ]);
    assert!(fails(&held, "AX-021").is_empty());
    let skipped = walk(vec![
        vec![combo(None), a()],
        vec![combo(None), focused(a())],
    ]);
    assert_eq!(fails(&skipped, "AX-021"), ["w:combo"]);
}

#[test]
fn ax_022_a_tab_that_moves_nothing_warns() {
    let a = || button("a", "A");
    let b = || button("b", "B");
    let moved = walk(vec![
        vec![a(), b()],
        vec![focused(a()), b()],
        vec![a(), focused(b())],
    ]);
    assert!(fails(&moved, "AX-022").is_empty());
    let stuck = walk(vec![
        vec![a(), b()],
        vec![focused(a()), b()],
        vec![focused(a()), b()],
    ]);
    assert_eq!(fails(&stuck, "AX-022"), ["step 2"]);
    assert!(
        stuck
            .violations
            .iter()
            .any(|v| v.rule == "AX-022" && v.severity == Warn)
    );
}

#[test]
fn ax_023_under_a_modal_focus_never_leaves() {
    let a = || button("a", "A");
    let mut inside = reading(vec![
        vec![focused(a())],
        vec![focused(a())],
        vec![focused(a())],
    ]);
    inside.modal = true;
    assert!(fails(&audit(&inside, false), "AX-023").is_empty());
    let mut out = reading(vec![vec![focused(a())], vec![focused(a())], vec![a()]]);
    out.modal = true;
    assert_eq!(fails(&audit(&out, false), "AX-023"), ["step 2"]);
    // no modal: not applicable
    assert!(
        !walk(vec![vec![a()], vec![a()]])
            .applicable
            .contains_key("AX-023")
    );
}

#[test]
fn ax_024_a_shell_dialog_has_escape_bound() {
    let dialog = || node("dlg", "Dialog", "Settings");
    let bound = reading(vec![vec![dialog(), focused(button("a", "A"))]]);
    assert!(fails(&audit(&bound, false), "AX-024").is_empty());
    let mut unbound = reading(vec![vec![dialog(), focused(button("a", "A"))]]);
    unbound.escape = vec![false];
    assert_eq!(fails(&audit(&unbound, false), "AX-024"), ["w:dlg"]);
    // a view's dialog closes on its own key
    let mut view = dialog();
    view.scope = "w/chat".into();
    let mut viewed = reading(vec![vec![view, focused(button("a", "A"))]]);
    viewed.escape = vec![false];
    assert!(!audit(&viewed, false).applicable.contains_key("AX-024"));
}

#[test]
fn ax_025_a_dialog_shows_with_something_focused() {
    let dialog = || node("dlg", "Dialog", "Settings");
    let held = one(vec![dialog(), focused(button("a", "A"))]);
    assert!(fails(&held, "AX-025").is_empty());
    let lost = one(vec![dialog(), button("a", "A")]);
    assert_eq!(fails(&lost, "AX-025"), ["w:dlg"]);
}

#[test]
fn coverage_counts_actionable_nodes_clean_of_errors_and_rules_failed_over_applicable() {
    let mut warned = button("thin", "Go");
    warned.bounds = Some([0, 0, 10, 10]);
    let mut heading = node("h", "Heading", "");
    heading.more.level = Some(1);
    let report = one(vec![
        button("ok", "Save"),
        button("bare", ""),
        warned,
        heading,
    ]);
    assert_eq!(report.nodes, 4);
    assert_eq!(report.actionable, 3);
    // "bare" fails AX-002 (error); "thin" only warns
    assert!(
        (report.coverage.actionable - 2. / 3.).abs() < 1e-9,
        "{report:#?}"
    );
    let failed = report.violations.len();
    let applicable: usize = report.applicable.values().sum();
    assert!((report.coverage.rule - (1. - failed as f64 / applicable as f64)).abs() < 1e-9);
    assert_eq!(report.errors().count(), 2);
    assert_eq!(
        one(Vec::new()).coverage,
        Coverage {
            actionable: 1.,
            rule: 1.
        }
    );
}

#[test]
fn a_node_seen_in_every_snapshot_counts_once() {
    let bare = || button("bare", "");
    let report = walk(vec![vec![bare()], vec![focused(bare())]]);
    assert_eq!(fails(&report, "AX-002"), ["w:bare"]);
    assert_eq!(report.applicable["AX-002"], 1);
    assert_eq!(report.nodes, 1);
}

/// Six buttons, Tab order as painted: three below the window's edge, then
/// three on it; the box around them holds the keys first.
struct Stops(gpui_kit::FocusHandle);

impl gpui_kit::Render for Stops {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::*;
        let button = |id: &'static str, top: f32| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(id)
                .focusable()
                .tab_stop(true)
                .on_click(|_, _, _| {})
                .absolute()
                .left(px(0.))
                .top(px(top))
                .size(px(40.))
        };
        div()
            .id("stops")
            .track_focus(&self.0)
            .relative()
            .size_full()
            .child(button("hidden-1", 1000.))
            .child(button("hidden-2", 1100.))
            .child(button("hidden-3", 1200.))
            .child(button("shown-1", 0.))
            .child(button("shown-2", 50.))
            .child(button("shown-3", 100.))
    }
}

/// The first snapshot shows three stops of a Tab cycle of six: N + 1
/// presses end on the first shown one, and the walk goes on round.
#[gpui_kit::test]
fn the_walk_goes_round_a_cycle_longer_than_the_first_snapshot_shows(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::AppContext as _;
    use gpui_kit::test::TestWindowExt as _;
    cx.update(gpui_kit::init);
    // the kit's root is what answers Tab
    let window = cx.open_window(
        gpui_kit::size(gpui_kit::px(200.), gpui_kit::px(200.)),
        |window, cx| {
            let stops = cx.new(|cx| Stops(cx.focus_handle()));
            gpui_kit::component::Root::new(stops, window, cx)
        },
    );
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let snap = |window: &mut Window, cx: &mut App| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        crate::ax::snapshot("w", window, true)
    };
    let report = native.update(|window, cx| {
        let stops = window
            .root::<gpui_kit::component::Root>()
            .flatten()
            .unwrap();
        let held = stops.read(cx).view().clone().downcast::<Stops>().unwrap();
        held.read(cx).0.clone().focus(window, cx);
        let reading = observe(window, cx, true, |_| true, snap);
        audit(&reading, false)
    });
    assert!(fails(&report, "AX-021").is_empty(), "{report:?}");
    assert_eq!(report.presses, 7);
}

mod phase_two {
    //! One passing and one failing case per phase-2 rule, over hand-built
    //! nodes.
    use super::*;

    fn under(mut node: AxNode, parent: &str) -> AxNode {
        node.more.parent = Some(format!("w:{parent}"));
        node
    }

    #[test]
    #[should_panic(expected = "AX-999 is not in the rule table")]
    fn a_rule_missing_from_the_table_is_a_bug_not_an_error() {
        severity("AX-999");
    }

    #[test]
    fn ax_101_a_tab_an_option_or_a_tree_item_says_whether_it_is_picked() {
        let mut on = node("on", "Tab", "Appearance");
        on.state = vec!["selected"];
        let mut off = node("off", "ListBoxOption", "Help");
        off.state = vec!["unselected"];
        let report = one(vec![on, off, node("none", "TreeItem", "src")]);
        assert_eq!(fails(&report, "AX-101"), ["w:none"]);
        assert_eq!(report.applicable["AX-101"], 3);
    }

    #[test]
    fn ax_102_a_status_is_polite_an_alert_assertive_and_both_carry_a_value() {
        let live = |id, role, live: &'static str, value: Option<&str>| {
            let mut node = node(id, role, "words");
            node.more.live = Some(live);
            node.value = value.map(str::to_owned);
            node
        };
        let report = one(vec![
            live("status", "Status", "polite", Some("words")),
            live("alert", "Alert", "assertive", Some("words")),
            live("loud", "Status", "assertive", Some("words")),
            live("mute", "Alert", "assertive", None),
            node("off", "Status", "words"),
        ]);
        assert_eq!(fails(&report, "AX-102"), ["w:loud", "w:mute", "w:off"]);
    }

    #[test]
    fn ax_103_a_dialog_the_walk_never_leaves_is_modal() {
        let dialog = |modal| {
            let mut node = node("dlg", "Dialog", "Account");
            node.more.modal = modal;
            node
        };
        let a = || under(button("a", "Lock"), "dlg");
        let b = || under(button("b", "Switch node"), "dlg");
        let held = |modal| {
            walk(vec![
                vec![dialog(modal), focused(a()), b()],
                vec![dialog(modal), a(), focused(b())],
                vec![dialog(modal), focused(a()), b()],
            ])
        };
        assert!(fails(&held(true), "AX-103").is_empty());
        assert_eq!(fails(&held(false), "AX-103"), ["w:dlg"]);
        // a walk that leaves it: not modal to the keyboard either
        let left = walk(vec![
            vec![dialog(false), focused(a())],
            vec![dialog(false), a(), focused(button("bar", "Search"))],
        ]);
        assert!(!left.applicable.contains_key("AX-103"));
        // no walk, no word on it
        assert!(
            !one(vec![dialog(false), focused(a())])
                .applicable
                .contains_key("AX-103")
        );
    }

    #[test]
    fn ax_104_the_focus_is_inside_the_dialog_that_shows() {
        let dialog = || node("dlg", "Dialog", "Account");
        let inside = one(vec![dialog(), focused(under(button("a", "Lock"), "dlg"))]);
        assert!(fails(&inside, "AX-104").is_empty());
        let outside = one(vec![
            dialog(),
            under(button("a", "Lock"), "dlg"),
            focused(button("bar", "Search")),
        ]);
        assert_eq!(fails(&outside, "AX-104"), ["w:dlg"]);
        // every snapshot: a menu the walk leaves closes, one that stays
        // open with the focus outside it fails
        let left = |stays: bool| {
            let mut after = vec![focused(button("bar", "Search"))];
            if stays {
                after.extend([dialog(), under(button("a", "Lock"), "dlg")]);
            }
            walk(vec![
                vec![dialog(), focused(under(button("a", "Lock"), "dlg"))],
                after,
            ])
        };
        assert!(fails(&left(false), "AX-104").is_empty());
        assert_eq!(fails(&left(true), "AX-104"), ["w:dlg"]);
    }

    #[test]
    fn ax_105_a_row_is_in_its_container() {
        let item = |id, parent| under(node(id, "MenuItem", "Lock"), parent);
        let report = one(vec![
            node("menu", "Menu", "Account actions"),
            under(node("group", "Group", ""), "menu"),
            item("deep", "group"),
            node("dialog", "Dialog", "Account"),
            item("loose", "dialog"),
            under(node("tab", "Tab", "Chat"), "dialog"),
            node("list", "TabList", "Programs"),
            under(node("held", "Tab", "Forge"), "list"),
        ]);
        assert_eq!(fails(&report, "AX-105"), ["w:loose", "w:tab"]);
        assert_eq!(report.applicable["AX-105"], 4);
    }

    #[test]
    fn ax_106_a_heading_has_a_level_from_one_to_six() {
        let heading = |id, level| {
            let mut node = node(id, "Heading", "Connect");
            node.more.level = level;
            node
        };
        let report = one(vec![
            heading("h1", Some(1)),
            heading("none", None),
            heading("deep", Some(7)),
        ]);
        assert_eq!(fails(&report, "AX-106"), ["w:deep", "w:none"]);
    }

    fn invalid(id: &str, description: Option<&str>) -> AxNode {
        let mut node = field(id, "Node address");
        node.more.invalid = Some("true");
        node.description = description.map(str::to_owned);
        node
    }

    #[test]
    fn ax_107_the_focus_in_a_composite_is_on_a_row_the_arrows_move() {
        let menu = || node("menu", "Menu", "Networks");
        let row = |id| {
            let mut row = under(node(id, "MenuItemRadio", id), "menu");
            row.actions = vec!["press", "focus"];
            row
        };
        // on a row: the composite is the one the arrows move
        let on_row = vec![menu(), focused(row("a")), row("b")];
        assert_eq!(
            crate::ax::audit::arrowed(&on_row).map(|node| node.id.as_str()),
            Some("w:menu")
        );
        assert!(crate::ax::audit::arrowed(&[menu(), focused(row("a"))]).is_none());
        assert!(!one(on_row).applicable.contains_key("AX-107"));
        // on the composite itself, with rows in it
        let mut held = menu();
        held.state = vec!["focused"];
        assert_eq!(fails(&one(vec![held, row("a")]), "AX-107"), ["w:menu"]);
        // the probe: each press moves the active row, or it does not
        let probe = |focused: [&str; 3]| {
            let mut told = reading(vec![vec![menu()]]);
            told.arrows = vec![Arrows {
                composite: menu(),
                focused: focused.map(|id| Some(format!("w:{id}"))),
            }];
            audit(&told, false)
        };
        assert!(fails(&probe(["a", "b", "a"]), "AX-107").is_empty());
        assert_eq!(fails(&probe(["a", "a", "a"]), "AX-107"), ["w:menu"]);
    }

    #[test]
    fn ax_108_an_invalid_field_says_why() {
        let report = one(vec![
            invalid("said", Some("no route to host")),
            invalid("silent", None),
            invalid("blank", Some(" ")),
            field("fine", "Name"),
        ]);
        assert_eq!(fails(&report, "AX-108"), ["w:blank", "w:silent"]);
        assert_eq!(report.applicable["AX-108"], 3);
    }

    #[test]
    fn ax_109_a_field_refused_empty_is_required() {
        let refused = |id, required| {
            let mut node = invalid(id, Some("Type a name"));
            node.more.required = required;
            node
        };
        let mut typed = refused("typed", false);
        typed.value = Some("x".into());
        let report = one(vec![
            refused("marked", true),
            refused("unmarked", false),
            typed,
        ]);
        assert_eq!(fails(&report, "AX-109"), ["w:unmarked"]);
        assert_eq!(report.applicable["AX-109"], 2);
        assert_eq!(report.violations[0].severity, Warn);
    }

    #[test]
    fn ax_110_a_control_that_says_work_is_in_flight_is_busy() {
        let off = |id, name| {
            let mut node = node(id, "Button", name);
            node.state = vec!["disabled"];
            node
        };
        let mut creating = off("creating", "Creating…");
        creating.state.push("busy");
        let report = one(vec![
            creating,
            off("adding", "Adding…"),
            off("loading", "Loading"),
            off("opens", "Add a device…"),
            button("save", "Save"),
            node("status", "Status", "Connecting…"),
        ]);
        assert_eq!(fails(&report, "AX-110"), ["w:adding", "w:loading"]);
        assert_eq!(report.applicable["AX-110"], 3);
    }

    #[test]
    fn ax_111_a_fields_name_is_not_its_placeholder() {
        let hinted = |id, name, placeholder: &str| {
            let mut node = field(id, name);
            node.more.placeholder = Some(placeholder.to_owned());
            node
        };
        let report = one(vec![
            hinted("ok", "Search", "Type a program"),
            hinted("same", "Search", "Search"),
            field("none", "Search"),
        ]);
        assert_eq!(fails(&report, "AX-111"), ["w:same"]);
        assert_eq!(report.applicable["AX-111"], 2);
    }

    #[test]
    fn ax_112_a_view_set_that_says_where_its_rows_are_says_it_for_each() {
        let row = |id, set: Option<(usize, usize)>| {
            let mut node = under(node(id, "ListItem", "Row"), "list");
            node.scope = "w/chat".into();
            node.more.position_in_set = set.map(|(position, _)| position);
            node.more.size_of_set = set.map(|(_, size)| size);
            node
        };
        let mut half = row("half", None);
        half.more.position_in_set = Some(2);
        let report = one(vec![
            row("a", Some((1, 40))),
            row("b", Some((2, 40))),
            row("bare", None),
            half,
            row("past", Some((41, 40))),
        ]);
        assert_eq!(fails(&report, "AX-112"), ["w:bare", "w:half", "w:past"]);
        // a list that says nothing, and the shell's own rows, are not asked
        let mut native = row("native", None);
        native.scope = "w".into();
        native.more.size_of_set = Some(3);
        let report = one(vec![row("x", None), row("y", None), native]);
        assert!(!report.applicable.contains_key("AX-112"));
    }

    #[test]
    fn ax_113_a_button_that_opens_something_says_what() {
        let mut menu = button("menu", "Network: testkit");
        menu.state = vec!["collapsed"];
        menu.more.has_popup = Some("menu");
        let mut bare = button("bare", "Account");
        bare.state = vec!["expanded"];
        let report = one(vec![menu, bare, button("plain", "Save")]);
        assert_eq!(fails(&report, "AX-113"), ["w:bare"]);
        assert_eq!(report.applicable["AX-113"], 2);
    }

    #[test]
    fn ax_114_a_control_help_lists_with_a_chord_reports_it() {
        let mut said = button("said", "Search");
        said.more.keyboard_shortcut = Some("⌘K".into());
        let mut told = reading(vec![vec![
            said,
            button("help", "Help"),
            button("other", "Save"),
        ]]);
        told.chords = vec![("Search".into(), "⌘K".into()), ("Help".into(), "⌘/".into())];
        let report = audit(&told, false);
        assert_eq!(fails(&report, "AX-114"), ["w:help"]);
        assert_eq!(report.applicable["AX-114"], 2);
        // told nothing, it asks nothing
        assert!(
            !one(vec![button("help", "Help")])
                .applicable
                .contains_key("AX-114")
        );
    }

    #[test]
    fn ax_116_a_step_comes_with_a_value_and_a_fold_with_its_state() {
        let mut spin = node("spin", "SpinButton", "Volume");
        spin.actions = vec!["focus", "increment", "decrement"];
        spin.value = Some("7".into());
        let mut blind = spin.clone();
        blind.id = "w:blind".into();
        blind.value = None;
        let mut fold = node("fold", "TreeItem", "src");
        fold.actions = vec!["focus", "expand"];
        fold.state = vec!["collapsed", "unselected"];
        let mut shut = fold.clone();
        shut.id = "w:shut".into();
        shut.state = vec!["unselected"];
        let report = one(vec![spin, blind, fold, shut]);
        assert_eq!(fails(&report, "AX-116"), ["w:blind", "w:shut"]);
    }

    #[test]
    fn ax_117_a_link_is_pressed_and_named_by_words() {
        let link = |id, name, press| {
            let mut node = node(id, "Link", name);
            if press {
                node.actions = vec!["press"];
            }
            node
        };
        let report = one(vec![
            link("ok", "the docs", true),
            link("dead", "the docs", false),
            link("glyph", "→", true),
        ]);
        assert_eq!(fails(&report, "AX-117"), ["w:dead", "w:glyph"]);
    }

    #[test]
    fn ax_118_a_splitter_is_named_and_the_keyboard_reaches_it() {
        let splitter = |id, name, focus| {
            let mut node = node(id, "Splitter", name);
            if focus {
                node.actions = vec!["focus"];
            }
            node
        };
        let report = one(vec![
            splitter("ok", "Resize the sidebar", true),
            splitter("bare", "", true),
            splitter("mouse", "Resize the sidebar", false),
        ]);
        assert_eq!(fails(&report, "AX-118"), ["w:bare", "w:mouse"]);
    }

    #[test]
    fn ax_119_nothing_pressable_sits_inside_a_control() {
        let mut tab = node("tab", "Tab", "Chat");
        tab.actions = vec!["press", "focus"];
        tab.state = vec!["selected"];
        let report = one(vec![
            tab,
            under(button("close", "Close Chat"), "tab"),
            under(node("badge", "Status", "2 unread"), "tab"),
            button("beside", "Save"),
        ]);
        assert_eq!(fails(&report, "AX-119"), ["w:close"]);
        assert_eq!(report.applicable["AX-119"], 3);
    }
}
