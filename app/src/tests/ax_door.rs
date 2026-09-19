//! The test door (#114) over fixture screens: the same tree `ax_contract`
//! reads, as the door reports it.
use super::*;
use crate::ax_door::{self, AxNode, DoorFile, Reply};
use gpui_kit::{AnyWindowHandle, AppContext, HeadlessAppContext, px, size};

fn draw(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn read(cx: &mut HeadlessAppContext, window: AnyWindowHandle, name: &str) -> Vec<AxNode> {
    cx.update_window(window, |_, window, _| {
        ax_door::snapshot(name, window, false)
    })
    .unwrap()
}

/// A native screen, switched on and drawn as the door finds it.
fn open(app: Ducktape, kind: crate::shell::WindowKind) -> (HeadlessAppContext, AnyWindowHandle) {
    let mut cx = crate::frame_probe::headless_context();
    let window = cx
        .open_window(size(px(1280.), px(800.)), |window, cx| {
            let view = crate::shell::test_window(app, kind, window, cx);
            cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
        })
        .unwrap()
        .into();
    cx.update_window(window, |_, window, _| window.activate_a11y())
        .unwrap();
    draw(&mut cx, window);
    (cx, window)
}

fn console() -> Ducktape {
    let mut app = Ducktape::initial_state();
    app.connected = true;
    app.connected_rpc = "http://127.0.0.1:8844".into();
    app.network_name = "walk".into();
    app
}

fn json(nodes: &[AxNode]) -> String {
    serde_json::to_string(&ax_door::compact(nodes)).unwrap()
}

#[test]
fn door_ids_are_stable_and_named_by_element_path() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let (mut cx, window) = open(console(), crate::shell::WindowKind::Console);
    let first = read(&mut cx, window, "console");
    let (mut again, window) = open(console(), crate::shell::WindowKind::Console);
    let second = read(&mut again, window, "console");
    assert!(
        first.len() > 5,
        "the console shows a tree: {}",
        json(&first)
    );
    assert_eq!(json(&first), json(&second), "same screen, same bytes");
    let mut ids = std::collections::HashSet::new();
    for node in &first {
        assert!(node.id.starts_with("console:"), "{}", node.id);
        let entity = |segment: &str| {
            segment
                .strip_prefix("view-")
                .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit()))
        };
        assert!(
            !node.id.split(['.', ':', '/']).any(entity),
            "an entity id leaked: {}",
            node.id
        );
        assert!(ids.insert(&node.id), "duplicate id {}", node.id);
    }
    assert!(
        first
            .iter()
            .any(|node| node.id == "console:view:chat" && node.role == "Tab"),
        "a rail tab carries its call site's id: {}",
        json(&first)
    );
}

#[test]
fn a_shared_element_id_is_widened_by_its_ancestors_only() {
    let path = |segments: &[&str]| {
        (
            "w:".to_owned(),
            segments.iter().map(|segment| segment.to_string()).collect(),
        )
    };
    let ids = ax_door::door_ids(&[
        path(&["root", "left", "close"]),
        path(&["root", "right", "close"]),
        path(&["root", "save"]),
        path(&["root", "save"]),
    ]);
    assert_eq!(
        ids,
        [
            "w:left.close",
            "w:right.close",
            "w:root.save",
            "w:root.save~2"
        ]
    );
}

#[test]
fn door_offers_exactly_the_actionable_nodes() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let (mut cx, window) = open(console(), crate::shell::WindowKind::Console);
    let nodes = read(&mut cx, window, "console");
    let offers = serde_json::to_value(ax_door::offers(&nodes)).unwrap();
    let offers = offers.as_array().unwrap();
    let actionable: Vec<&AxNode> = nodes
        .iter()
        .filter(|node| !node.actions.is_empty())
        .collect();
    assert!(!actionable.is_empty());
    assert_eq!(
        offers.len(),
        actionable
            .iter()
            .map(|node| node.actions.len())
            .sum::<usize>()
    );
    for node in &nodes {
        let offered: Vec<&str> = offers
            .iter()
            .filter(|offer| offer["id"] == node.id.as_str())
            .map(|offer| offer["action"].as_str().unwrap())
            .collect();
        assert_eq!(offered, node.actions, "{}", node.id);
    }
    assert!(
        nodes
            .iter()
            .all(|node| !node.state.contains(&"disabled") || node.actions.is_empty())
    );
}

#[test]
fn door_act_answers_with_the_delta() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let (mut cx, window) = open(console(), crate::shell::WindowKind::Console);
    let before = read(&mut cx, window, "console");
    let tab = before
        .iter()
        .find(|node| {
            node.role == "Tab"
                && !node.state.contains(&"selected")
                && node.actions.contains(&"press")
        })
        .unwrap_or_else(|| panic!("an unselected rail tab: {}", json(&before)))
        .clone();
    assert!(
        ax_door::nearest(&format!("{}x", tab.id), &before).contains(&tab.id),
        "a near miss names the node it missed"
    );
    press(&mut cx, window, &tab.id);
    draw(&mut cx, window);
    let after = read(&mut cx, window, "console");
    let delta = serde_json::to_value(ax_door::delta(&before, &after)).unwrap();
    let changed = delta["changed"].as_array().unwrap();
    assert!(
        changed.iter().any(|node| node["id"] == tab.id.as_str()
            && node["state"]
                .as_array()
                .unwrap()
                .contains(&"selected".into())),
        "the pressed tab is selected now: {delta}"
    );
}

fn keys(cx: &mut HeadlessAppContext, window: AnyWindowHandle, keys: &str, text: &str) {
    cx.update_window(window, |_, window, cx| {
        ax_door::press_keys(window, cx, keys, text)
    })
    .unwrap()
    .unwrap_or_else(|error| panic!("{keys}: {error}"));
    cx.run_until_parked();
    draw(cx, window);
}

fn focused(nodes: &[AxNode]) -> Option<&AxNode> {
    nodes.iter().find(|node| node.state.contains(&"focused"))
}

/// A keyboard-only walk: Tab moves focus through the window's own key
/// dispatch, the tree reports where focus is, and Enter on a focused rail
/// tab selects it — no accessibility action involved.
#[test]
fn door_keys_move_focus_and_activate_the_focused_node() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let (mut cx, window) = open(console(), crate::shell::WindowKind::Console);
    // Tab from nowhere goes nowhere: the kit binds it in the Root context,
    // on no dispatch path until something holds focus. A real window focuses
    // its own handle when it opens (shell.rs); the fixture does not, so start
    // from a focused node.
    let first = read(&mut cx, window, "console")
        .into_iter()
        .find(|node| node.actions.contains(&"focus"))
        .expect("a focusable node");
    cx.update_window(window, |_, window, cx| {
        ax_door::perform_by_id("console", window, cx, &first.id, "focus", "")
    })
    .unwrap();
    draw(&mut cx, window);
    let mut seen = Vec::new();
    let tab = loop {
        keys(&mut cx, window, "tab", "");
        let nodes = read(&mut cx, window, "console");
        let now = focused(&nodes)
            .unwrap_or_else(|| panic!("Tab puts focus on a node: {}", json(&nodes)))
            .clone();
        if now.role == "Tab" && !now.state.contains(&"selected") {
            break now;
        }
        assert!(
            !seen.contains(&now.id) && seen.len() < 200,
            "Tab went round without reaching an unselected rail tab: {seen:?}"
        );
        seen.push(now.id);
    };
    keys(&mut cx, window, "shift-tab", "");
    let back = read(&mut cx, window, "console");
    assert_ne!(
        focused(&back).map(|node| node.id.as_str()),
        Some(tab.id.as_str()),
        "Shift-Tab moves focus back"
    );
    keys(&mut cx, window, "tab enter", "");
    let after = read(&mut cx, window, "console");
    let pressed = after.iter().find(|node| node.id == tab.id).unwrap();
    assert!(
        pressed.state.contains(&"selected"),
        "Enter on the focused tab selects it: {}",
        json(&after)
    );
    // a view's chord is a claim, not a binding: the door names it too
    crate::module_view::claim_chord("cmd-shift-alt-9", "palette").unwrap();
    let shortcuts = cx
        .update_window(window, |_, window, cx| {
            assert!(ax_door::press_keys(window, cx, "tab k-ctrl", "").is_err());
            serde_json::to_string(&ax_door::shortcuts(window, cx)).unwrap()
        })
        .unwrap();
    crate::module_view::release_chord("cmd-shift-alt-9", "palette");
    assert!(
        shortcuts.contains(r#""keys":"tab""#),
        "the bindings reachable from focus: {shortcuts}"
    );
    let command = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    assert!(
        shortcuts.contains(&format!(
            r#""keys":"{command}-shift-alt-9","action":"the palette view's cmd-shift-alt-9""#
        )),
        "a claimed chord: {shortcuts}"
    );
}

fn press(cx: &mut HeadlessAppContext, window: AnyWindowHandle, id: &str) {
    let pressed = cx
        .update_window(window, |_, window, cx| {
            ax_door::perform_by_id("console", window, cx, id, "press", "")
        })
        .unwrap();
    assert!(pressed, "{id} is showing");
    cx.run_until_parked();
}

#[test]
fn door_masks_secure_input_and_private_text() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let mut app = Ducktape::initial_state();
    app.hub_step = HubStep::Password;
    let (mut cx, window) = open(app, crate::shell::WindowKind::Onboarding);
    let nodes = read(&mut cx, window, "onboarding");
    let field = nodes
        .iter()
        .find(|node| node.role == "PasswordInput")
        .unwrap_or_else(|| panic!("a password field: {}", json(&nodes)))
        .id
        .clone();
    let secret = "correct-horse-battery";
    cx.update_window(window, |_, window, cx| {
        ax_door::perform_by_id("onboarding", window, cx, &field, "set_value", secret)
    })
    .unwrap();
    draw(&mut cx, window);
    let nodes = read(&mut cx, window, "onboarding");
    let whole = serde_json::to_string(&nodes).unwrap();
    assert!(!whole.contains(secret), "{whole}");
    // a person sees dots in a password field, so reveal refuses it
    let refused = cx
        .update_window(window, |_, window, _| {
            ax_door::reveal("onboarding", window, &field)
        })
        .unwrap();
    assert_eq!(refused.status(), 403, "{}", refused.body());
    assert!(!refused.body().contains(secret), "{}", refused.body());
    let button = nodes
        .iter()
        .find(|node| node.role == "Button")
        .unwrap_or_else(|| panic!("a button: {}", json(&nodes)))
        .id
        .clone();
    let plain = cx
        .update_window(window, |_, window, _| {
            ax_door::reveal("onboarding", window, &button)
        })
        .unwrap();
    assert_eq!(plain.status(), 400, "not private: {}", plain.body());

    // the recovery-phrase word's marker, as the phrase step sets it
    struct Phrase;
    impl gpui_kit::Render for Phrase {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::{
                InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _,
                Styled as _,
            };
            gpui_kit::div()
                .size_full()
                .child(gpui_notion::editor::ui::ax_private(
                    gpui_kit::div()
                        .id("phrase-word/1")
                        .role(gpui_kit::Role::Label)
                        .aria_value("abandon")
                        .child("abandon"),
                ))
        }
    }
    let mut cx = crate::frame_probe::headless_context();
    let window: AnyWindowHandle = cx
        .open_window(size(px(400.), px(300.)), |_, cx| cx.new(|_| Phrase))
        .unwrap()
        .into();
    cx.update_window(window, |_, window, _| window.activate_a11y())
        .unwrap();
    draw(&mut cx, window);
    let nodes = read(&mut cx, window, "onboarding");
    let whole = serde_json::to_string(&nodes).unwrap();
    assert!(whole.contains("onboarding:phrase-word/1"), "{whole}");
    assert!(!whole.contains("abandon"), "{whole}");
    assert!(whole.contains("•••"), "{whole}");
    // the word is drawn on screen: reveal hands it over, and only it
    let shown = cx
        .update_window(window, |_, window, _| {
            ax_door::reveal("onboarding", window, "onboarding:phrase-word/1")
        })
        .unwrap();
    assert_eq!(shown.status(), 200, "{}", shown.body());
    let shown: serde_json::Value = serde_json::from_str(shown.body()).unwrap();
    assert_eq!(shown["value"], "abandon", "{shown}");
    let missing = cx
        .update_window(window, |_, window, _| {
            ax_door::reveal("onboarding", window, "onboarding:phrase-word/2")
        })
        .unwrap();
    assert_eq!(missing.status(), 404);
    // and the tree stays masked after a reveal
    let whole = serde_json::to_string(&read(&mut cx, window, "onboarding")).unwrap();
    assert!(!whole.contains("abandon"), "{whole}");
}

#[test]
fn door_wait_answers_at_its_deadline() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let (mut cx, window) = open(console(), crate::shell::WindowKind::Console);
    let nodes = read(&mut cx, window, "console");
    let wait = |body: serde_json::Value| serde_json::from_value::<ax_door::Wait>(body).unwrap();
    let missing = wait(serde_json::json!({ "name": "no such node", "deadline_ms": 10 }));
    assert_eq!(missing.step(&nodes, false), None, "not yet decided");
    let late = missing.step(&nodes, true).unwrap();
    assert_eq!(late.status(), 408);
    assert!(late.body().contains("\"tree\""), "the tree at the deadline");
    let tab = wait(serde_json::json!({ "role": "tab", "in": "console", "deadline_ms": 10 }));
    assert_eq!(tab.step(&nodes, false).unwrap().status(), 200);
    let gone = wait(serde_json::json!({ "name": "no such node", "gone": true, "deadline_ms": 10 }));
    assert_eq!(gone.step(&nodes, false).unwrap().body(), r#"{"gone":true}"#);
}

#[test]
fn door_binds_loopback_only_and_is_absent_without_the_env() {
    assert_eq!(ax_door::door_port(None), Ok(None));
    assert_eq!(ax_door::door_port(Some("")), Ok(None));
    assert_eq!(ax_door::door_port(Some("0")), Ok(Some(0)));
    for address in ["0.0.0.0:4000", "127.0.0.1:4000", "[::]:4000", "localhost"] {
        assert!(ax_door::door_port(Some(address)).is_err(), "{address}");
    }
    assert!(ax_door::open_env(None, None).is_none());
    assert!(
        ax_door::open_env(None, Some("1")).is_none(),
        "the private switch alone opens nothing"
    );
    let listener = ax_door::bind(0).unwrap();
    assert!(listener.local_addr().unwrap().ip().is_loopback());
}

fn stranger_to(door: &DoorFile) -> DoorFile {
    DoorFile {
        port: door.port,
        token: "guess".into(),
    }
}

#[test]
fn door_round_trip_over_loopback() {
    let listener = ax_door::bind(0).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        ax_door::accept(listener, "t0ken", false, |request| {
            Some(Reply::new(200, serde_json::json!(format!("{request:?}"))))
        })
    });
    let door = DoorFile {
        port,
        token: "t0ken".into(),
    };
    let (status, body) = ax_door::call(&door, "GET", "/tree?window=console&compact=1", "").unwrap();
    assert_eq!(status, 200);
    assert!(
        body.contains("compact: true") && body.contains("console"),
        "{body}"
    );
    let act = r#"{"id":"console:view:chat","action":"press"}"#;
    let (status, body) = ax_door::call(&door, "POST", "/act", act).unwrap();
    assert_eq!(
        (status, body.contains("console:view:chat")),
        (200, true),
        "{body}"
    );
    assert_eq!(ax_door::call(&door, "POST", "/act", "{").unwrap().0, 400);
    assert_eq!(ax_door::call(&door, "GET", "/nowhere", "").unwrap().0, 404);
    let (status, body) = ax_door::call(
        &door,
        "POST",
        "/key",
        r#"{"keys":"shift-tab","window":"console"}"#,
    )
    .unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#"keys: \"shift-tab\""#), "{body}");
    let (status, body) = ax_door::call(&door, "GET", "/keys?window=console", "").unwrap();
    assert_eq!((status, body.contains("Keys")), (200, true), "{body}");
    // without DUCKTAPE_AX_DOOR_PRIVATE=1 there is no reveal at all
    let ask = r#"{"id":"onboarding:phrase-word/1"}"#;
    let (status, body) = ax_door::call(&door, "POST", "/reveal", ask).unwrap();
    assert_eq!(status, 404, "{body}");
    assert!(!body.contains("reveal"), "not even named: {body}");
    let stranger = DoorFile {
        port,
        token: "guess".into(),
    };
    assert_eq!(ax_door::call(&stranger, "GET", "/tree", "").unwrap().0, 401);

    let listener = ax_door::bind(0).unwrap();
    let private = DoorFile {
        port: listener.local_addr().unwrap().port(),
        token: "t0ken".into(),
    };
    std::thread::spawn(move || {
        ax_door::accept(listener, "t0ken", true, |request| {
            Some(Reply::new(200, serde_json::json!(format!("{request:?}"))))
        })
    });
    let (status, body) = ax_door::call(&private, "POST", "/reveal", ask).unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains("Reveal") && body.contains("phrase-word/1"),
        "{body}"
    );
    assert_eq!(
        ax_door::call(&stranger_to(&private), "POST", "/reveal", ask)
            .unwrap()
            .0,
        401,
        "the private door still takes the token"
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ducktape").join("ax-door.json");
    ax_door::write_door_file(&path, &door).unwrap();
    let written: DoorFile = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written, door);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
