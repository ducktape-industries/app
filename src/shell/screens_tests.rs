//! AX-tree coverage for the shell's own screens: a native input reports its
//! current text as the AX value unless it is `secret`, and an error alert
//! carries a name (`aria_label`), the way `src/ax/tree.rs`'s compact filter
//! requires.
use super::*;
use gpui_kit::accesskit::{Action, ActionData, ActionRequest, TreeId};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, px, size};

fn draw(window: &mut Window, cx: &mut gpui_kit::App) -> serde_json::Value {
    window.activate_a11y();
    window.render_frame(cx);
    window.render_frame(cx);
    serde_json::to_value(crate::ax::snapshot("shell", window, false)).unwrap()
}

/// Sends the door's own SetValue action to the field named `"{key}/field"`,
/// the way a real AX client types into it.
fn type_into(field: &str, text: &str, window: &mut Window, cx: &mut gpui_kit::App) {
    draw(window, cx);
    let target = window
        .a11y_tree()
        .unwrap()
        .nodes
        .iter()
        .find_map(|(node, _)| {
            window
                .a11y_element_id(*node)
                .is_some_and(|path| {
                    path.iter().any(|element| {
                        matches!(element, ElementId::Name(name) if name.as_ref() == field)
                    })
                })
                .then_some(*node)
        })
        .unwrap_or_else(|| panic!("missing AX control {field}"));
    window.dispatch_a11y_action(
        ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: target,
            data: Some(ActionData::Value(text.into())),
        },
        cx,
    );
}

fn find<'a>(nodes: &'a serde_json::Value, role: &str, name: &str) -> &'a serde_json::Value {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["role"] == role && node["name"] == name)
        .unwrap_or_else(|| panic!("missing {role} {name:?} in {nodes}"))
}

fn open(state: Ducktape, cx: &mut TestAppContext) -> (Entity<DesktopWindow>, VisualTestContext) {
    let model = cx.new(|cx| Desktop {
        state,
        tray: crate::tray::init(cx).0,
        windows: BTreeMap::new(),
        views: BTreeMap::new(),
        streams: HashMap::new(),
    });
    let key = WindowKey::unique();
    let mut view = None;
    let handle = cx.open_window(size(px(1280.), px(800.)), |window, cx| {
        let desktop = cx.new(|cx| DesktopWindow {
            model: model.clone(),
            key,
            kind: WindowKind::Console,
            layout: layout::Layout::default(),
            mounted: BTreeMap::new(),
            initialized: false,
            resize: None,
            measured_widths: Default::default(),
            inputs: HashMap::new(),
            focus: cx.focus_handle(),
            _activation: cx.observe_window_activation(window, |_, _, _| {}),
            _observer: cx.observe(&model, |_, _, cx| cx.notify()),
            _keystrokes: DesktopWindow::intercept_global_keys(window, cx),
            _focus_lost: cx.on_focus_lost(window, |_, _, _| {}),
        });
        view = Some(desktop.clone());
        gpui_kit::component::Root::new(desktop, window, cx)
    });
    let view = view.unwrap();
    model.update(cx, |model, _| {
        model.windows.insert(key, handle.into());
        model.views.insert(key, view.downgrade());
    });
    (view, VisualTestContext::from_window(handle.into(), cx))
}

#[gpui_kit::test]
fn connect_screen_exposes_the_endpoint_and_names_its_error(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (mut state, _) = Ducktape::boot();
    state.endpoint = "127.0.0.1:9000".to_string();
    state.endpoint_error = "no route to host".to_string();
    let (_view, mut native) = open(state, cx);

    let nodes = native.update(draw);
    assert_eq!(
        find(&nodes, "TextInput", "Node address")["value"],
        "127.0.0.1:9000"
    );
    assert_eq!(
        find(&nodes, "Alert", "no route to host")["name"],
        "no route to host"
    );

    // The value tracks what is actually typed, not just the seed the field
    // was constructed with.
    native.update(|window, cx| type_into("endpoint/field", "10.0.0.5:8844", window, cx));
    let nodes = native.update(draw);
    assert_eq!(
        find(&nodes, "TextInput", "Node address")["value"],
        "10.0.0.5:8844"
    );
}

#[gpui_kit::test]
fn sign_in_screens_keep_secret_fields_out_of_the_ax_value(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (mut state, _) = Ducktape::boot();
    state.screen = Screen::Console;
    state.unlock_error = "wrong password".to_string();
    let (view, mut native) = open(state, cx);

    // Typing into either field clears the error (screens.rs's own UX), so
    // it has to be checked before that, not folded into the same draw.
    let nodes = native.update(draw);
    assert_eq!(
        find(&nodes, "Alert", "wrong password")["name"],
        "wrong password"
    );

    native.update(|window, cx| type_into("account-name/field", "duck", window, cx));
    native.update(|window, cx| type_into("password/field", "hunter2", window, cx));
    let nodes = native.update(draw);
    assert_eq!(find(&nodes, "TextInput", "Account name")["value"], "duck");
    let password = find(&nodes, "PasswordInput", "Password");
    assert!(
        password.get("value").is_none(),
        "password value leaked into the AX tree: {password}"
    );

    // The recovery-phrase input is not visually masked (it is not a
    // PasswordInput), but its typed text must stay out of the tree too.
    let model = native.update(|_, cx| view.read(cx).model.clone());
    model.update(cx, |model, _| model.state.restoring = true);
    native.update(|window, cx| {
        type_into(
            "restore-phrase/field",
            "abandon abandon abandon",
            window,
            cx,
        )
    });
    let nodes = native.update(draw);
    let phrase = find(&nodes, "TextInput", "Recovery phrase");
    assert!(
        phrase.get("value").is_none(),
        "recovery phrase leaked into the AX tree: {phrase}"
    );
}
