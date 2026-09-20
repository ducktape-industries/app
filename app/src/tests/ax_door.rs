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

/// "Join your team", a stranger's first screen.
fn join_screen() -> (HeadlessAppContext, AnyWindowHandle) {
    let mut app = Ducktape::initial_state();
    app.hub_step = HubStep::Join;
    open(app, crate::shell::WindowKind::Onboarding)
}

fn invitation_field(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> String {
    let nodes = read(cx, window, "onboarding");
    nodes
        .iter()
        .find(|node| node.role == "PasswordInput")
        .unwrap_or_else(|| panic!("the invitation field: {}", json(&nodes)))
        .id
        .clone()
}

/// `invite` typed into the invitation field through the door.
fn type_invitation(cx: &mut HeadlessAppContext, window: AnyWindowHandle, invite: &str) {
    let field = invitation_field(cx, window);
    let typed = cx
        .update_window(window, |_, window, cx| {
            ax_door::perform_by_id("onboarding", window, cx, &field, "type", invite)
        })
        .unwrap();
    assert!(typed, "{field} is showing");
    cx.run_until_parked();
}

/// #144: Return in the invitation field submits it as Join network does —
/// the one field's form and its default button.
#[test]
fn return_in_the_invitation_field_submits_it_as_join_network_does() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let submitted = |by_key: bool| {
        let (mut cx, window) = join_screen();
        type_invitation(&mut cx, window, "not-an-invitation");
        draw(&mut cx, window);
        match by_key {
            true => keys(&mut cx, window, "enter", ""),
            false => {
                let nodes = read(&mut cx, window, "onboarding");
                let join = nodes
                    .iter()
                    .find(|node| node.name == "Join network")
                    .unwrap_or_else(|| panic!("Join network: {}", json(&nodes)));
                assert!(join.actions.contains(&"press"), "{}", json(&nodes));
                let pressed = cx
                    .update_window(window, |_, window, cx| {
                        ax_door::perform_by_id("onboarding", window, cx, &join.id, "press", "")
                    })
                    .unwrap();
                assert!(pressed);
                cx.run_until_parked();
                draw(&mut cx, window);
            }
        }
        json(&read(&mut cx, window, "onboarding"))
    };
    let by_key = submitted(true);
    assert!(by_key.contains("cannot be read here"), "rejected: {by_key}");
    assert_eq!(by_key, submitted(false));
}

/// A node no workspace serves: a wallet form sent to it is refused before any
/// key is written.
const UNSERVED: &str = "http://127.0.0.1:9";

/// How a walked wallet form ends: Return in its last field, a press on the
/// named button, or neither.
enum Last<'a> {
    Return,
    Press(&'a str),
    Neither,
}

/// A wallet form of more than one field (#144), each field typed in by name
/// and left by Return, which must hand focus to the next field. The tree once
/// `last` ends it.
fn walk_a_wallet_form(app: Ducktape, fields: &[(&str, &str)], last: Last) -> String {
    let (mut cx, window) = open(app, crate::shell::WindowKind::Onboarding);
    for (at, (name, text)) in fields.iter().enumerate() {
        let nodes = read(&mut cx, window, "onboarding");
        let field = nodes
            .iter()
            .find(|node| node.name == *name)
            .unwrap_or_else(|| panic!("{name}: {}", json(&nodes)))
            .id
            .clone();
        let typed = cx
            .update_window(window, |_, window, cx| {
                ax_door::perform_by_id("onboarding", window, cx, &field, "type", text)
            })
            .unwrap();
        assert!(typed, "{name} is showing");
        cx.run_until_parked();
        draw(&mut cx, window);
        let Some((next, _)) = fields.get(at + 1) else {
            break;
        };
        keys(&mut cx, window, "enter", "");
        let nodes = read(&mut cx, window, "onboarding");
        assert_eq!(
            focused(&nodes).map(|node| node.name.as_str()),
            Some(*next),
            "Return in {name}: {}",
            json(&nodes)
        );
    }
    match last {
        Last::Return => keys(&mut cx, window, "enter", ""),
        Last::Press(button) => {
            let nodes = read(&mut cx, window, "onboarding");
            let id = nodes
                .iter()
                .find(|node| node.name == button)
                .unwrap_or_else(|| panic!("{button}: {}", json(&nodes)))
                .id
                .clone();
            let pressed = cx
                .update_window(window, |_, window, cx| {
                    ax_door::perform_by_id("onboarding", window, cx, &id, "press", "")
                })
                .unwrap();
            assert!(pressed, "{button} is showing");
            cx.run_until_parked();
            draw(&mut cx, window);
        }
        Last::Neither => {}
    }
    json(&read(&mut cx, window, "onboarding"))
}

/// "Protect your wallet": Return in Password moves to Confirm password, and
/// Return there creates the wallet as Create wallet does — and does nothing
/// while the two differ and Create wallet is disabled.
#[test]
fn return_walks_protect_your_wallet_and_submits_it_as_create_wallet_does() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let app = || {
        let mut app = Ducktape::initial_state();
        app.hub_step = HubStep::Password;
        app.rpc = UNSERVED.into();
        app
    };
    let matched = [
        ("Password", "correct-horse"),
        ("Confirm password", "correct-horse"),
    ];
    let by_key = walk_a_wallet_form(app(), &matched, Last::Return);
    assert!(by_key.contains("has not answered"), "submitted: {by_key}");
    let by_press = walk_a_wallet_form(app(), &matched, Last::Press("Create wallet"));
    assert_eq!(by_key, by_press);
    let mismatched = [
        ("Password", "correct-horse"),
        ("Confirm password", "correct-hors"),
    ];
    assert_eq!(
        walk_a_wallet_form(app(), &mismatched, Last::Return),
        walk_a_wallet_form(app(), &mismatched, Last::Neither),
        "Return past a disabled Create wallet"
    );
}

/// "Restore your wallet": Return walks Wallet name → Recovery phrase → New
/// password, and there restores as Restore does — and does nothing while a
/// restore is already under way and Restore is disabled.
#[test]
fn return_walks_restore_your_wallet_and_submits_it_as_restore_does() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let app = |busy: bool| {
        let mut app = Ducktape::initial_state();
        app.hub_step = HubStep::Restore;
        app.rpc = UNSERVED.into();
        if busy {
            app.mutation_phase = MutationPhase::Onboarding;
        }
        app
    };
    let fields = [
        ("Wallet name", "restored"),
        ("Recovery phrase", "not a phrase"),
        ("New password", "correct-horse"),
    ];
    let by_key = walk_a_wallet_form(app(false), &fields, Last::Return);
    assert!(by_key.contains("has not answered"), "submitted: {by_key}");
    let by_press = walk_a_wallet_form(app(false), &fields, Last::Press("Restore"));
    assert_eq!(by_key, by_press);
    assert_eq!(
        walk_a_wallet_form(app(true), &fields, Last::Return),
        walk_a_wallet_form(app(true), &fields, Last::Neither),
        "Return past a disabled Restore"
    );
}

/// A node's description is served — why a control is disabled, a field's
/// placeholder — as a screen reader reads it after the name.
#[test]
fn door_serves_a_node_description() {
    let mut send = view_wire::kit::button("send", "Send", None, view_wire::ButtonPreset::Primary);
    let view_wire::Node::Button { description, .. } = &mut send else {
        unreachable!()
    };
    *description = Some("Create an account to send".into());
    let mut cx = crate::frame_probe::headless_context();
    let window: AnyWindowHandle = cx
        .open_window(size(px(400.), px(300.)), |window, cx| {
            let view = cx.new(|_| crate::view_tree::ViewTree::new(send));
            cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
        })
        .unwrap()
        .into();
    cx.update_window(window, |_, window, _| window.activate_a11y())
        .unwrap();
    draw(&mut cx, window);
    let nodes = read(&mut cx, window, "view");
    let send = nodes
        .iter()
        .find(|node| node.name == "Send")
        .unwrap_or_else(|| panic!("Send: {}", json(&nodes)));
    assert_eq!(
        send.description.as_deref(),
        Some("Create an account to send")
    );
    assert!(
        json(&nodes).contains(r#""description":"Create an account to send""#),
        "{}",
        json(&nodes)
    );
}

/// A named wire overlay is a modal AccessKit scope: its base is covered for
/// door reads and acts, while closing it restores the base controls.
#[test]
fn door_scopes_a_named_overlay_to_the_active_modal() {
    use std::sync::{Arc, Mutex};

    fn fixture(open: bool) -> view_wire::Node {
        let base = view_wire::kit::button(
            "base-action",
            "Base action",
            Some(1),
            view_wire::ButtonPreset::Primary,
        );
        let modal = view_wire::kit::button(
            "modal-action",
            "Modal action",
            Some(2),
            view_wire::ButtonPreset::Primary,
        );
        view_wire::Node::Overlay {
            key: "fixture-overlay".into(),
            label: Some("Fixture dialog".into()),
            padding: 0.,
            backdrop: view_wire::Rgba([0., 0., 0., 0.4]),
            align_x: view_wire::AlignX::Center,
            align_y: view_wire::AlignY::Center,
            on_dismiss: None,
            children: if open { vec![base, modal] } else { vec![base] },
        }
    }

    let mut cx = crate::frame_probe::headless_context();
    let view = cx.new(|_| crate::view_tree::ViewTree::new(fixture(false)));
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let subscription = cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &view_wire::Event, _| {
            observed.lock().unwrap().push(event.clone());
        })
    });
    let root_view = view.clone();
    let window: AnyWindowHandle = cx
        .open_window(size(px(400.), px(300.)), |window, cx| {
            cx.new(|cx| gpui_kit::component::Root::new(root_view, window, cx))
        })
        .unwrap()
        .into();
    cx.update_window(window, |_, window, _| window.activate_a11y())
        .unwrap();
    draw(&mut cx, window);
    let base_id = read(&mut cx, window, "fixture")
        .into_iter()
        .find(|node| node.name == "Base action")
        .expect("base action before open")
        .id;
    cx.update_window(window, |_, _, cx| {
        view.update(cx, |view, cx| view.replace(fixture(true), cx));
    })
    .unwrap();
    draw(&mut cx, window);

    let modal_accesskit = cx
        .update_window(window, |_, window, _| {
            window.a11y_tree().and_then(|update| {
                update
                    .nodes
                    .iter()
                    .map(|(_, node)| node)
                    .find(|node| node.role() == gpui_kit::Role::Dialog)
                    .map(|node| (node.label().map(str::to_owned), node.is_modal()))
            })
        })
        .unwrap();
    assert_eq!(
        modal_accesskit,
        Some((Some("Fixture dialog".to_owned()), true))
    );

    let scoped = read(&mut cx, window, "fixture");
    let base = scoped.iter().find(|node| node.name == "Base action");
    let modal = scoped.iter().find(|node| node.name == "Modal action");
    assert!(base.is_none(), "covered base is absent: {}", json(&scoped));
    let modal = modal.expect("modal action is reachable");
    assert!(modal.actions.contains(&"press"));
    let offers = serde_json::to_string(&ax_door::offers(&scoped)).unwrap();
    assert!(offers.contains("Modal action"), "{offers}");
    assert!(!offers.contains("Base action"), "{offers}");

    cx.update_window(window, |_, window, cx| {
        assert!(ax_door::perform_by_id(
            "fixture", window, cx, &modal.id, "focus", ""
        ));
    })
    .unwrap();
    draw(&mut cx, window);
    keys(&mut cx, window, "tab shift-tab", "");
    let focused_nodes = read(&mut cx, window, "fixture");
    let focused_modal = focused(&focused_nodes)
        .map(|node| node.name.as_str())
        .unwrap_or_default();
    assert_eq!(focused_modal, "Modal action");

    let covered = cx
        .update_window(window, |_, window, cx| {
            ax_door::perform_by_id("fixture", window, cx, &base_id, "press", "")
        })
        .unwrap();
    assert!(!covered);
    assert!(events.lock().unwrap().is_empty());
    let pressed = cx
        .update_window(window, |_, window, cx| {
            ax_door::perform_by_id("fixture", window, cx, &modal.id, "press", "")
        })
        .unwrap();
    assert!(pressed);
    assert_eq!(&*events.lock().unwrap(), &[view_wire::Event::Message(2)]);

    cx.update_window(window, |_, _, cx| {
        view.update(cx, |view, cx| view.replace(fixture(false), cx));
    })
    .unwrap();
    draw(&mut cx, window);
    let restored = read(&mut cx, window, "fixture");
    let base = restored
        .iter()
        .find(|node| node.name == "Base action")
        .expect("base action returns after close");
    assert!(!restored.iter().any(|node| node.name == "Modal action"));
    let offers = serde_json::to_string(&ax_door::offers(&restored)).unwrap();
    assert!(offers.contains("Base action"), "{offers}");
    cx.update_window(window, |_, window, cx| {
        assert!(ax_door::perform_by_id(
            "fixture", window, cx, &base.id, "focus", ""
        ));
    })
    .unwrap();
    keys(&mut cx, window, "tab shift-tab", "");
    assert_eq!(
        focused(&read(&mut cx, window, "fixture")).map(|node| node.name.as_str()),
        Some("Base action")
    );
    let pressed = cx
        .update_window(window, |_, window, cx| {
            ax_door::perform_by_id("fixture", window, cx, &base.id, "press", "")
        })
        .unwrap();
    assert!(pressed);
    assert_eq!(
        &*events.lock().unwrap(),
        &[view_wire::Event::Message(2), view_wire::Event::Message(1)]
    );
    drop(subscription);
}

#[test]
fn door_nested_modal_is_topmost_and_window_local() {
    fn fixture(stage: u8) -> view_wire::Node {
        let base = view_wire::kit::button(
            "nested-base",
            "Nested base",
            Some(1),
            view_wire::ButtonPreset::Primary,
        );
        let outer_action = view_wire::kit::button(
            "outer-action",
            "Outer action",
            Some(2),
            view_wire::ButtonPreset::Primary,
        );
        let inner = view_wire::Node::Overlay {
            key: "inner-overlay".into(),
            label: Some("Inner dialog".into()),
            padding: 0.,
            backdrop: view_wire::Rgba([0.; 4]),
            align_x: view_wire::AlignX::Left,
            align_y: view_wire::AlignY::Top,
            on_dismiss: None,
            children: vec![
                view_wire::Node::empty(),
                view_wire::kit::button(
                    "inner-action",
                    "Inner action",
                    Some(3),
                    view_wire::ButtonPreset::Primary,
                ),
            ],
        };
        let modal = match stage {
            0 => None,
            1 => Some(view_wire::kit::column("outer-card", [outer_action])),
            _ => Some(view_wire::kit::column("outer-card", [outer_action, inner])),
        };
        view_wire::Node::Overlay {
            key: "outer-overlay".into(),
            label: Some("Outer dialog".into()),
            padding: 0.,
            backdrop: view_wire::Rgba([0.; 4]),
            align_x: view_wire::AlignX::Left,
            align_y: view_wire::AlignY::Top,
            on_dismiss: None,
            children: match modal {
                Some(modal) => vec![base, modal],
                None => vec![base],
            },
        }
    }

    let mut cx = crate::frame_probe::headless_context();
    let first_view = cx.new(|_| crate::view_tree::ViewTree::new(fixture(0)));
    let second_view = cx.new(|_| crate::view_tree::ViewTree::new(fixture(0)));
    let open = |cx: &mut HeadlessAppContext, view: gpui_kit::Entity<crate::view_tree::ViewTree>| {
        cx.open_window(size(px(400.), px(300.)), |window, cx| {
            let view = view.clone();
            cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
        })
        .unwrap()
        .into()
    };
    let first: AnyWindowHandle = open(&mut cx, first_view.clone());
    let second: AnyWindowHandle = open(&mut cx, second_view);
    for window in [first, second] {
        cx.update_window(window, |_, window, _| window.activate_a11y())
            .unwrap();
        draw(&mut cx, window);
    }
    let base_id = read(&mut cx, first, "nested")
        .into_iter()
        .find(|node| node.name == "Nested base")
        .expect("the underlay is visible before opening")
        .id;

    cx.update_window(first, |_, _, cx| {
        first_view.update(cx, |view, cx| view.replace(fixture(1), cx));
    })
    .unwrap();
    draw(&mut cx, first);
    let outer_id = read(&mut cx, first, "nested")
        .into_iter()
        .find(|node| node.name == "Outer action")
        .expect("the outer dialog is visible")
        .id;

    cx.update_window(first, |_, _, cx| {
        first_view.update(cx, |view, cx| view.replace(fixture(2), cx));
    })
    .unwrap();
    draw(&mut cx, first);
    let nested = read(&mut cx, first, "nested");
    assert!(nested.iter().any(|node| node.name == "Inner action"));
    assert!(!nested.iter().any(|node| node.name == "Outer action"));
    assert!(!nested.iter().any(|node| node.name == "Nested base"));
    assert_eq!(json(&nested), json(&read(&mut cx, first, "nested")));
    assert!(
        !cx.update_window(first, |_, window, cx| {
            ax_door::perform_by_id("nested", window, cx, &outer_id, "press", "")
        })
        .unwrap()
    );
    assert!(
        !cx.update_window(first, |_, window, cx| {
            ax_door::perform_by_id("nested", window, cx, &base_id, "press", "")
        })
        .unwrap()
    );

    let other = read(&mut cx, second, "nested");
    assert!(other.iter().any(|node| node.name == "Nested base"));
    assert!(!other.iter().any(|node| node.name == "Inner action"));
}

/// #147: a window the OS stops drawing (covered, asleep, locked) still
/// serves its current tree — a door read draws it — and the revision moves
/// only when the tree does.
#[test]
fn a_door_read_serves_a_change_no_frame_has_drawn() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let (mut cx, window) = join_screen();
    let (calls, served) = futures::channel::mpsc::unbounded();
    cx.update(|cx| {
        cx.spawn(async move |cx| {
            ax_door::serve(served, move |_| vec![("onboarding".into(), window)], cx).await
        })
        .detach()
    });
    let tree = |cx: &mut HeadlessAppContext| {
        let (reply, answer) = std::sync::mpsc::channel();
        let request = ax_door::Request::Tree {
            filter: Default::default(),
            compact: true,
            bounds: false,
        };
        calls.unbounded_send((request, reply)).unwrap();
        cx.run_until_parked();
        answer.try_recv().expect("the door answered")
    };
    let joinable = |body: &str| {
        let nodes: Vec<serde_json::Value> = serde_json::from_str(body).unwrap();
        let join = nodes
            .iter()
            .find(|node| node["name"] == "Join network")
            .unwrap_or_else(|| panic!("Join network: {body}"));
        !join["state"]
            .as_array()
            .unwrap()
            .contains(&"disabled".into())
    };
    let first = tree(&mut cx);
    assert!(!joinable(first.body()), "no invitation yet");
    let again = tree(&mut cx);
    assert_eq!(
        (again.body(), again.revision()),
        (first.body(), first.revision()),
        "nothing changed, nothing advanced"
    );
    // one key, read before any frame: this harness draws a dirty window as
    // an update ends and a window draws before each key it dispatches, so
    // the change and the read share one update and the change is one key
    let field = invitation_field(&mut cx, window);
    let (stale, served) = cx
        .update_window(window, |_, window, cx| {
            let typed = ax_door::perform_by_id("onboarding", window, cx, &field, "type", "x");
            assert!(typed);
            let stale = ax_door::snapshot("onboarding", window, false);
            let mut seen = ax_door::Seen::default();
            let served = ax_door::current("onboarding", window, cx, false, &mut seen);
            (json(&stale), json(&served))
        })
        .unwrap();
    assert!(!joinable(&stale), "the last frame's tree: {stale}");
    assert!(joinable(&served), "{served}");
    let after = tree(&mut cx);
    assert!(joinable(after.body()), "{}", after.body());
    assert!(
        after.revision() > first.revision(),
        "{:?} after {:?}",
        after.revision(),
        first.revision()
    );
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
fn door_drag_request_takes_local_or_window_px_and_refuses_bad_ones() {
    let drag = |body: serde_json::Value| serde_json::from_value::<ax_door::Drag>(body);
    let local =
        drag(serde_json::json!({ "id": "console:view:chat", "from": [1, 2], "to": [3.5, 4] }))
            .unwrap();
    assert_eq!(
        local.checked().map_err(|reply| reply.status()),
        Ok(4),
        "4 steps unless given"
    );
    let window = drag(
        serde_json::json!({ "from": [0, 0], "to": [10, 10], "steps": 9, "window": "console" }),
    )
    .unwrap();
    assert_eq!(window.checked().map_err(|reply| reply.status()), Ok(9));
    assert!(
        drag(serde_json::json!({ "from": [1, 2] })).is_err(),
        "no destination"
    );
    assert!(
        drag(serde_json::json!({ "from": [1, 2, 3], "to": [1, 2] })).is_err(),
        "not a pair"
    );
    assert!(
        drag(serde_json::json!({ "from": ["1", "2"], "to": [1, 2] })).is_err(),
        "not numbers"
    );
    for body in [
        serde_json::json!({ "from": [-1, 0], "to": [1, 2] }),
        serde_json::json!({ "from": [0, 0], "to": [1, -2] }),
        serde_json::json!({ "from": [0, 0], "to": [1, 2], "steps": 0 }),
    ] {
        let status = drag(body.clone()).unwrap().checked().unwrap_err().status();
        assert_eq!(status, 400, "{body}");
    }
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
            Some(Reply::new(200, serde_json::json!(format!("{request:?}"))).revised(7))
        })
    });
    let door = DoorFile {
        port,
        token: "t0ken".into(),
    };
    {
        use std::io::{Read as _, Write as _};
        let mut raw = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        raw.write_all(b"GET /tree HTTP/1.1\r\nAuthorization: Bearer t0ken\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        raw.read_to_string(&mut response).unwrap();
        let head = response.split_once("\r\n\r\n").unwrap().0;
        assert!(head.contains("\r\nX-Ax-Revision: 7\r\n"), "{head}");
    }
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
    let drag = r#"{"id":"console:view:chat","from":[1,2],"to":[3,4]}"#;
    let (status, body) = ax_door::call(&door, "POST", "/drag", drag).unwrap();
    assert_eq!(
        (
            status,
            body.contains(r#"Drag(Drag { id: Some(\"console:view:chat\")"#)
        ),
        (200, true),
        "{body}"
    );
    assert_eq!(
        ax_door::call(&door, "POST", "/drag", r#"{"from":[1]}"#)
            .unwrap()
            .0,
        400
    );
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
