//! One passing and one failing case per rule, over hand-built nodes.
use super::*;
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
        scope: "w".to_owned(),
        bounds: None,
        node: NodeId(0),
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
        modal: false,
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
    let report = one(vec![
        button("ok", "Save"),
        button("bare", ""),
        warned,
        node("h", "Heading", ""),
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
