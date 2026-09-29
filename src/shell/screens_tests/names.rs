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
    let mut state = gate::desk();
    state.roster = crate::runtime::Roster::listing(&["names-test"]);
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

/// A bar button says what its press opens (AX-113), and a control Help
/// lists with a chord reports the chord (AX-114): the bar's Search and the
/// empty desk's three ways out.
#[gpui_kit::test]
fn a_button_says_what_it_opens_and_its_chord(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    // a program to open, so the empty desk offers its buttons
    let mut state = gate::desk();
    state.roster = crate::runtime::Roster::listing(&["chords-test"]);
    let (_view, mut native) = open(state, cx);
    let nodes = native.update(draw);
    for (id, popup) in [
        ("shell:network-switcher", "menu"),
        ("shell:rail-search", "dialog"),
        ("shell:rail-notifications", "dialog"),
        ("shell:rail-connection", "dialog"),
        ("shell:rail-account", "dialog"),
        ("shell:settings", "dialog"),
    ] {
        assert_eq!(node(&nodes, id)["has_popup"], popup, "{id}");
    }
    for (id, key) in [
        ("shell:rail-search", "K"),
        ("shell:empty-desk/new", "N"),
        ("shell:empty-desk/search", "K"),
        ("shell:empty-desk/help", "/"),
    ] {
        assert_eq!(
            node(&nodes, id)["keyboard_shortcut"],
            chord_label(key),
            "{id}"
        );
    }
    // and the audit, told the chords, finds none of either missing
    let report = native.update(|window, cx| {
        let snap = |window: &mut Window, cx: &mut gpui_kit::App| {
            draw(window, cx);
            crate::ax::snapshot("shell", window, false)
        };
        let mut reading = crate::ax::audit::observe(window, cx, false, |_| true, snap);
        reading.chords = crate::shell::chords();
        crate::ax::audit::audit(&reading, false)
    });
    let missing: Vec<_> = report
        .violations
        .iter()
        .filter(|violation| matches!(violation.rule, "AX-113" | "AX-114"))
        .collect();
    assert!(missing.is_empty(), "{missing:#?}");
    assert!(report.applicable["AX-114"] >= 4, "{:?}", report.applicable);
}
