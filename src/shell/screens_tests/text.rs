//! The shell's own words in the tree (docs/ax.md §4 gap 2): headlines,
//! leads, captions, Help, About, the node's status, the account header and
//! the Approve instruction are Labels a screen reader reads, each launcher
//! screen has one Heading of level 1, and a mapped list of lines gets one
//! node a line.
use super::*;
use gpui_kit::Role;

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
            "[01 / 04] Network",
            "Any node on it will do",
            "Node address",
        ],
    );

    let mut seed = gate::desk();
    seed.active = Some(layout::HELP);
    let (view, mut native) = open(seed, cx);
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
    assert_eq!(
        labels(&nodes)
            .iter()
            .any(|line| line.contains("Tab switches")),
        layers::CHAT_READY
    );
    let app = entities(&view, &mut native);
    app.prefs.update(&mut native, |prefs, cx| {
        prefs.set_layout(crate::backend::Layout::Sidebar, cx)
    });
    let nodes = native.update(draw);
    reads(
        &nodes,
        &[
            "The column on the left lists the programs",
            "Click a program's name in the column",
            "THE SIDEBAR",
            "Top to bottom: the network's name",
        ],
    );
    let lines = labels(&nodes);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("account menu, above your name")),
        "{lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("The bar across the top")),
        "the bar's words stayed"
    );

    let about = Overlay::Settings(SettingsPage::About);
    let (_view, mut native) = open((gate::desk(), about), cx);
    let nodes = native.update(draw);
    let version = crate::update::version();
    reads(&nodes, &["About", "Ducktape", "Version", &version]);

    let updates = Overlay::Settings(SettingsPage::Updates);
    let (_view, mut native) = open((gate::desk(), updates), cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &[
            "Updates",
            "Version",
            "Status",
            "Off: this copy is not an installed one",
        ],
    );

    let mut seed = gate::desk();
    seed.chain.node = Some(crate::backend::NodeStatus {
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
        roles: crate::backend::test_roles(),
    });
    let (_view, mut native) = open((seed, Overlay::Menu(Popover::Node)), cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &["In sync", "testkit · ", "Height", "6,230", "Epoch"],
    );

    let mut seed = gate::desk();
    seed.account.account = Some(Some((7, "Ada Lovelace".into())));
    let (_view, mut native) = open((seed, Overlay::Menu(Popover::Account)), cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &["Ada Lovelace", "account 7 · testkit", "This device's key"],
    );

    let (_view, mut native) = open((gate::desk(), Overlay::Approve), cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &["Add a device", "On the new device, choose", "Code"],
    );
}

#[gpui_kit::test]
fn settings_updates_reads_the_channel_the_status_and_the_restart(cx: &mut TestAppContext) {
    crate::update::show_here(Some(crate::update::Updates::Here(crate::update::Shown {
        channel: crate::update::Channel::Nightly,
        automatic: true,
        status: "0.1.0+abc1234 is ready: it takes over at the next start".into(),
        restart: true,
        update_now: false,
        checked: Some("checked at 14:05".into()),
        checking: false,
    })));
    let updates = Overlay::Settings(SettingsPage::Updates);
    let (_view, mut native) = open((gate::desk(), updates), cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &[
            "Version",
            "Channel",
            "Stable follows",
            "Update automatically",
            "Off still checks",
            "Status",
            "0.1.0+abc1234 is ready: it takes over at the next start · checked at 14:05",
        ],
    );
    let controls: Vec<(String, String)> = nodes
        .as_array()
        .unwrap()
        .iter()
        .map(|node| {
            let field = |key: &str| node[key].as_str().unwrap_or_default().to_owned();
            (field("role"), field("name"))
        })
        .collect();
    for (role, name) in [
        ("RadioButton", "Stable"),
        ("RadioButton", "Nightly"),
        ("Switch", "Update automatically"),
        ("Button", "Check now"),
        ("Button", "Restart to update"),
    ] {
        assert!(
            controls.contains(&(role.into(), name.into())),
            "no {role} {name:?} in {controls:?}"
        );
    }
    gate::passes(&mut native, "settings-updates-ready", false);
    crate::update::show_here(None);
}

#[gpui_kit::test]
fn settings_updates_off_names_what_is_newer_and_offers_update_now(cx: &mut TestAppContext) {
    crate::update::show_here(Some(crate::update::Updates::Here(crate::update::Shown {
        channel: crate::update::Channel::Stable,
        automatic: false,
        status: "0.1.0+abc1234 is available: it is not installed".into(),
        restart: false,
        update_now: true,
        checked: None,
        checking: false,
    })));
    let updates = Overlay::Settings(SettingsPage::Updates);
    let (_view, mut native) = open((gate::desk(), updates), cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &[
            "Update automatically",
            "0.1.0+abc1234 is available: it is not installed",
        ],
    );
    let controls: Vec<(String, String)> = nodes
        .as_array()
        .unwrap()
        .iter()
        .map(|node| {
            let field = |key: &str| node[key].as_str().unwrap_or_default().to_owned();
            (field("role"), field("name"))
        })
        .collect();
    for (role, name) in [("Switch", "Update automatically"), ("Button", "Update now")] {
        assert!(
            controls.contains(&(role.into(), name.into())),
            "no {role} {name:?} in {controls:?}"
        );
    }
    assert!(
        !controls.iter().any(|(_, name)| name == "Restart to update"),
        "{controls:?}"
    );
    gate::passes(&mut native, "settings-updates-off", false);
    crate::update::show_here(None);
}

#[gpui_kit::test]
fn settings_updates_says_a_check_runs(cx: &mut TestAppContext) {
    crate::update::show_here(Some(crate::update::Updates::Here(crate::update::Shown {
        channel: crate::update::Channel::Stable,
        automatic: true,
        status: "Up to date".into(),
        restart: false,
        update_now: false,
        checked: None,
        checking: true,
    })));
    let updates = Overlay::Settings(SettingsPage::Updates);
    let (_view, mut native) = open((gate::desk(), updates), cx);
    let nodes = native.update(draw);
    let check = nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "shell:updates/check")
        .unwrap_or_else(|| panic!("no Check now: {nodes}"));
    assert_eq!(check["name"], "Checking…");
    let states: Vec<_> = check["state"].as_array().into_iter().flatten().collect();
    assert!(
        ["disabled", "busy"]
            .iter()
            .all(|state| states.iter().any(|it| *it == state)),
        "{check}"
    );
    gate::passes(&mut native, "settings-updates-checking", false);
    crate::update::show_here(None);
}

#[gpui_kit::test]
fn settings_updates_in_a_second_process_says_another_one_updates(cx: &mut TestAppContext) {
    crate::update::show_here(Some(crate::update::Updates::Elsewhere));
    let updates = Overlay::Settings(SettingsPage::Updates);
    let (_view, mut native) = open((gate::desk(), updates), cx);
    let nodes = native.update(draw);
    reads(
        &nodes,
        &["Status", "Another running Ducktape updates this install"],
    );
    crate::update::show_here(None);
}
