//! Names that say which one (docs/ax.md §4 gap 11): a program tab carries
//! its unread count, a notification row its time and source, a window's
//! buttons its program, the bar's account button what it shows, and the
//! node button holds still while blocks land.
use super::*;

fn node<'a>(nodes: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {nodes}"))
}

#[gpui_kit::test]
fn names_say_which_one(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    crate::runtime::list_for_test("names-test");
    let mut state = gate::desk();
    state.active = Some(crate::runtime::intern("names-test"));
    state.badges.insert(crate::runtime::intern("names-test"), 3);
    state.height = 6230;
    let (_view, mut native) = open(state, cx);
    let nodes = native.update(draw);

    let tab = &node(&nodes, "shell:rail/names-test")["name"];
    assert!(
        tab.as_str().unwrap().ends_with(", 3 unread"),
        "tab named {tab}"
    );
    let breath = node(&nodes, "shell:rail-connection");
    assert_eq!(breath["name"], "Node: in sync");
    assert_eq!(breath["description"], "Block 6230");
    for (button, name) in [
        ("split", "Open another names-test window"),
        ("popout", "Open names-test in a new window"),
        ("close", "Close names-test window"),
    ] {
        assert_eq!(
            node(&nodes, &format!("shell:pane/0/{button}"))["name"],
            name
        );
    }

    // a key, and no account on this network yet
    let mut state = gate::desk();
    state.account = Some(None);
    let (_view, mut native) = open(state, cx);
    let nodes = native.update(draw);
    find(&nodes, "Button", "Create account");
}
