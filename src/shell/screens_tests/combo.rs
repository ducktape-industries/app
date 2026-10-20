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
    let (_view, mut native) = open((gate::desk(), Overlay::Spotlight), cx);
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
    assert_eq!(
        find(&nodes, "ListBoxOption", "Help")["keyboard_shortcut"],
        chord_label("/")
    );
    native.simulate_keystrokes("down");
    let nodes = native.update(draw);
    let combo = find(&nodes, "EditableComboBox", "Search");
    assert_eq!(combo["active_descendant"], "shell:spotlight/1");
    native.update(|window, cx| type_into("spotlight/search", "zzz", window, cx));
    let nodes = native.update(draw);
    let combo = find(&nodes, "EditableComboBox", "Search");
    assert_eq!(combo["value"], "zzz");
    assert_eq!(combo["state"], json!(["focused", "collapsed"]));
}

#[gpui_kit::test]
fn an_empty_window_is_a_combo_box_whose_active_row_is_the_picked_one(cx: &mut TestAppContext) {
    let (_view, mut native) = open(two_programs(), cx);
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

#[gpui_kit::test]
fn an_empty_window_without_the_keys_passes_the_audit(cx: &mut TestAppContext) {
    let (_view, mut native) = two_empty_windows(cx);
    let nodes = native.update(draw);
    let lists: Vec<&serde_json::Value> = nodes
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["role"] == "ListBox")
        .collect();
    let combo = find(&nodes, "EditableComboBox", "Open a program");
    assert_eq!(lists.len(), 2, "both windows' rows are options: {lists:?}");
    assert_eq!(lists[1]["parent"], combo["id"]);
    assert_eq!(
        by_id(&nodes, "shell:pane/0/view")["keyboard_shortcut"],
        chord_label("1")
    );
    gate::passes(&mut native, "empty-window-unfocused", false);
}

#[gpui_kit::test]
fn a_press_on_a_row_of_a_window_without_the_keys_opens_it_there(cx: &mut TestAppContext) {
    let (view, mut native) = two_empty_windows(cx);
    let layout = native.update(|_, cx| view.read(cx).layout(cx));
    assert_eq!(layout.focused, 1);
    native.update(|window, cx| press("pane/0/view", "empty/combotest-alpha", window, cx));
    let nodes = native.update(draw);
    let layout = native.update(|_, cx| view.read(cx).layout(cx));
    assert_eq!(layout.focused, 0, "the pressed window has the keys");
    assert_eq!(layout.panes[0].program, "combotest-alpha");
    assert_eq!(layout.panes[1].program, crate::ui::layout::EMPTY);
    let keys: Vec<&str> = nodes
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| {
            node["state"]
                .as_array()
                .unwrap()
                .contains(&json!("focused"))
        })
        .map(|node| node["id"].as_str().unwrap())
        .collect();
    assert!(
        keys.len() == 1 && keys[0].starts_with("shell:pane/0/"),
        "the keys are in the window it opened in: {keys:?}"
    );

    let (view, mut native) = two_empty_windows(cx);
    native.update(|window, cx| press("pane/0/view", "empty-window/program", window, cx));
    native.update(draw);
    let layout = native.update(|_, cx| view.read(cx).layout(cx));
    assert_eq!(layout.focused, 0);
    assert_eq!(layout.panes[0].program, crate::ui::layout::EMPTY);
}

fn two_empty_windows(cx: &mut TestAppContext) -> (Entity<WindowRoot>, VisualTestContext) {
    let (view, mut native) = open(two_programs(), cx);
    for _ in 0..2 {
        native.update(|window, cx| {
            draw(window, cx);
            window.dispatch_action(Box::new(keys::NewWindow), cx);
        });
    }
    (view, native)
}

fn two_programs() -> Seed {
    let mut seed = gate::desk();
    seed.roster = crate::runtime::Roster::listing(&["combotest-alpha", "combotest-beta"]);
    seed
}
