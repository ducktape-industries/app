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
    // a dialog that is not modal may let the walk out once it has the focus
    let left = walk(vec![
        vec![dialog(), focused(under(button("a", "Lock"), "dlg"))],
        vec![
            dialog(),
            under(button("a", "Lock"), "dlg"),
            focused(button("bar", "Search")),
        ],
    ]);
    assert!(fails(&left, "AX-104").is_empty());
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
        crate::ax::audit::phase_two::arrowed(&on_row).map(|node| node.id.as_str()),
        Some("w:menu")
    );
    assert!(crate::ax::audit::phase_two::arrowed(&[menu(), focused(row("a"))]).is_none());
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
