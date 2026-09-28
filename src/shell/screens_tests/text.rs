//! The shell's own words in the tree (docs/ax.md §4 gap 2): headlines,
//! leads, captions, Help, About, the node's status, the account header and
//! the Approve instruction are Labels a screen reader reads, each launcher
//! screen has one Heading of level 1, and a mapped list of lines gets one
//! node a line.
use super::*;
use gpui_kit::Role;

/// The Heading nodes of `window`'s last tree: their name and level.
fn headings(window: &Window) -> Vec<(String, Option<usize>)> {
    window
        .a11y_tree()
        .map(|update| {
            update
                .nodes
                .iter()
                .filter(|(_, node)| node.role() == Role::Heading)
                .map(|(_, node)| (node.label().unwrap_or_default().to_owned(), node.level()))
                .collect()
        })
        .unwrap_or_default()
}

/// The words of every Label the door shows.
fn labels(nodes: &serde_json::Value) -> Vec<String> {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["role"] == "Label")
        .map(|node| node["name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[track_caller]
fn reads(nodes: &serde_json::Value, words: &[&str]) {
    let labels = labels(nodes);
    for words in words {
        assert!(
            labels.iter().any(|label| label.starts_with(words)),
            "no Label {words:?} in {labels:?}"
        );
    }
}

#[gpui_kit::test]
fn the_shells_own_words_are_in_the_tree(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    // every launcher screen: one Heading, level 1
    for (screen, launcher, build) in gate::matrix() {
        if !launcher {
            continue;
        }
        let (_view, mut native) = open(build(), cx);
        let heard = native.update(|window, cx| {
            draw(window, cx);
            headings(window)
        });
        assert!(
            heard.len() == 1 && heard[0].1 == Some(1) && !heard[0].0.is_empty(),
            "{screen}: {heard:?}"
        );
    }

    let (_view, mut native) = open(gate::matrix().remove(0).2(), cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &[
            "[01 / 03] Network",
            "Any node on it will do",
            "Node address",
        ],
    );

    // Help: its lead, a section, every paragraph, the keys table
    let mut state = gate::desk();
    state.active = Some(layout::HELP);
    let (_view, mut native) = open(state, cx);
    // tall enough that the door shows the whole page, unscrolled
    native.simulate_resize(size(px(1280.), px(2400.)));
    let nodes = native.update(draw);
    let heard = native.update(|window, _| headings(window));
    assert!(
        heard.contains(&("Ducktape help".to_owned(), Some(1))),
        "{heard:?}"
    );
    reads(
        &nodes,
        &[
            "The bar across the top lists",
            "OPEN A PROGRAM",
            "Click a program's name",
            "In a title bar, +",
            "Lock puts this device's key away",
            "This help",
            "In a text editor, move on (Tab alone indents)",
        ],
    );
    // no word of a Tab that only moves focus (AX-022)
    assert_eq!(
        labels(&nodes)
            .iter()
            .any(|line| line.contains("Tab switches")),
        command::CHAT_READY
    );

    // Settings › About
    let mut state = gate::desk();
    state.overlay = Some(crate::Overlay::Settings);
    state.settings_page = crate::ui::SettingsPage::About;
    let (_view, mut native) = open(state, cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &["About", "Ducktape", "Version", env!("CARGO_PKG_VERSION")],
    );

    // the node's status, in its menu
    let mut state = gate::desk();
    state.overlay = Some(crate::Overlay::Menu(crate::Popover::Node));
    state.node = Some(crate::backend::NodeStatus {
        network: "testkit".into(),
        time: 0,
        block_time_ms: 1000,
        epoch_length: 10,
        height: 6230,
        tip: [1; 32],
        root: abi::Root([2; 32]),
        epoch: 623,
        identity: vec![3; 32],
        contract: 1,
        genesis: [0; 32],
    });
    let (_view, mut native) = open(state, cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &["In sync", "testkit · ", "Height", "6,230", "Epoch"],
    );

    // the account menu's header
    let mut state = gate::desk();
    state.overlay = Some(crate::Overlay::Menu(crate::Popover::Account));
    state.account = Some(Some((7, "Ada Lovelace".into())));
    let (_view, mut native) = open(state, cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &["Ada Lovelace", "account 7 · testkit", "This device's key"],
    );

    // Approve's instruction
    let mut state = gate::desk();
    state.overlay = Some(crate::Overlay::Approve);
    let (_view, mut native) = open(state, cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &["Add a device", "On the new device, choose", "Code"],
    );
}
