//! Spotlight and an empty window pick from their rows with ↑↓ while the
//! keys stay in the field: to assistive technology each is one combo box,
//! the field, whose active row is the picked one (contract §6, AX-107).
use super::*;
use serde_json::json;

fn by_id<'a>(nodes: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == id)
        .unwrap_or_else(|| panic!("missing {id} in {nodes}"))
}

#[gpui_kit::test]
fn spotlight_is_a_combo_box_whose_active_row_is_the_picked_one(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let mut state = gate::desk();
    state.overlay = Some(crate::Overlay::Spotlight);
    let (_view, mut native) = open(state, cx);
    let nodes = native.update(draw);
    let combo = find(&nodes, "EditableComboBox", "Search");
    assert_eq!(combo["state"], json!(["expanded"]));
    assert_eq!(combo["active_descendant"], "shell:spotlight/0");
    assert_eq!(find(&nodes, "ListBox", "Results")["parent"], combo["id"]);
    let first = by_id(&nodes, "shell:spotlight/0");
    assert_eq!(first["role"], "ListBoxOption");
    assert_eq!(first["state"], json!(["focused", "selected"]));
    assert_eq!(
        by_id(&nodes, "shell:spotlight/1")["state"],
        json!(["unselected"])
    );
    native.simulate_keystrokes("down");
    let nodes = native.update(draw);
    let combo = find(&nodes, "EditableComboBox", "Search");
    assert_eq!(combo["active_descendant"], "shell:spotlight/1");
    // the field still takes what is typed, and with no rows the focus is
    // the field's own
    native.update(|window, cx| type_into("spotlight/search", "zzz", window, cx));
    let nodes = native.update(draw);
    let combo = find(&nodes, "EditableComboBox", "Search");
    assert_eq!(combo["value"], "zzz");
    assert_eq!(combo["state"], json!(["focused", "collapsed"]));
}

#[gpui_kit::test]
fn an_empty_window_is_a_combo_box_whose_active_row_is_the_picked_one(cx: &mut TestAppContext) {
    crate::runtime::list_for_test("combotest-alpha");
    crate::runtime::list_for_test("combotest-beta");
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (_view, mut native) = open(gate::desk(), cx);
    native.update(|window, cx| {
        draw(window, cx);
        window.dispatch_action(Box::new(keys::NewWindow), cx);
    });
    native.update(draw);
    native.simulate_input("combotest");
    let nodes = native.update(draw);
    let combo = find(&nodes, "EditableComboBox", "Open a program");
    assert_eq!(combo["value"], "combotest");
    assert_eq!(combo["active_descendant"], "shell:empty/combotest-alpha");
    let rows = find(&nodes, "ListBox", "Programs");
    assert_eq!(rows["parent"], combo["id"]);
    assert_eq!(
        by_id(&nodes, "shell:empty/combotest-beta")["state"],
        json!(["unselected"])
    );
    native.simulate_keystrokes("down");
    let nodes = native.update(draw);
    let combo = find(&nodes, "EditableComboBox", "Open a program");
    assert_eq!(combo["active_descendant"], "shell:empty/combotest-beta");
}
