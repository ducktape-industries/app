//! Real pane controls exercised through the same AccessKit actions as the AX door.
use super::*;
use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ElementId, TestAppContext, VisualTestContext, px, size};

fn draw(window: &mut Window, cx: &mut gpui_kit::App) -> serde_json::Value {
    window.activate_a11y();
    window.render_frame(cx);
    window.render_frame(cx);
    serde_json::to_value(crate::ax_door::snapshot("console", window, false)).unwrap()
}

fn press(id: &str, window: &mut Window, cx: &mut gpui_kit::App) {
    draw(window, cx);
    let node = window
        .a11y_tree()
        .unwrap()
        .nodes
        .iter()
        .find_map(|(node, _)| {
            window
                .a11y_element_id(*node)
                .is_some_and(|path| {
                    path.iter().any(
                        |element| matches!(element, ElementId::Name(name) if name.as_ref() == id),
                    )
                })
                .then_some(*node)
        })
        .unwrap_or_else(|| panic!("missing AX control {id}"));
    window.dispatch_a11y_action(
        ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: node,
            data: None,
        },
        cx,
    );
}

#[gpui_kit::test]
fn pane_strip_ax_actions_split_close_and_move_instances(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let model = cx.new(|cx| {
        let (mut state, _) = Ducktape::boot();
        state.screen = Screen::Console;
        state.browsing = true;
        state.active = Some("pane-ax-test");
        Desktop {
            state,
            tray: crate::tray::init(cx).0,
            windows: BTreeMap::new(),
            views: BTreeMap::new(),
            streams: HashMap::new(),
        }
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
    let mut native = VisualTestContext::from_window(handle.into(), cx);
    let nodes = native.update(draw);
    for control in ["split", "close", "popout"] {
        let id = format!("console:pane/0/{control}");
        assert!(
            nodes
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["id"] == id && node["role"] == "Button"),
            "missing {id}: {nodes}"
        );
    }
    for expected in [2, 3] {
        native.update(|window, cx| press("pane/0/split", window, cx));
        native.update(|window, cx| {
            draw(window, cx);
            assert_eq!(view.read(cx).layout.panes.len(), expected);
        });
    }
    native.update(|window, cx| {
        let nodes = draw(window, cx);
        let split = nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "console:pane/0/split")
            .unwrap();
        assert!(
            split["state"]
                .as_array()
                .unwrap()
                .iter()
                .any(|state| state == "disabled")
        );
        press("pane/0/split", window, cx);
        assert_eq!(view.read(cx).layout.panes.len(), 3);
    });
    let instance = native.update(|_, cx| view.read(cx).layout.panes[0].instance);
    native.update(|window, cx| press("pane/0/popout", window, cx));
    native.run_until_parked();
    let (popped_key, popped_handle, popped) = native.update(|_, cx| {
        assert_eq!(view.read(cx).layout.panes.len(), 2);
        let model = model.read(cx);
        let (&key, popped) = model
            .views
            .iter()
            .find(|(candidate, _)| **candidate != key)
            .expect("popout opened a view window");
        let popped = popped.upgrade().unwrap();
        assert_eq!(popped.read(cx).layout.panes[0].instance, instance);
        (key, model.windows[&key], popped)
    });
    native.update(|_, cx| {
        popped_handle
            .update(cx, |_, window, cx| press("pane/0/popin", window, cx))
            .unwrap();
    });
    native.run_until_parked();
    native.update(|window, cx| {
        draw(window, cx);
        assert_eq!(view.read(cx).layout.panes.len(), 3);
        assert_eq!(view.read(cx).layout.panes[2].instance, instance);
        assert!(popped.read(cx).layout.panes.is_empty());
        model.update(cx, |model, _| {
            model.views.remove(&popped_key);
            model.windows.remove(&popped_key);
        });
    });
    for remaining in [2, 1, 0] {
        native.update(|window, cx| press("pane/0/close", window, cx));
        native.update(|window, cx| {
            draw(window, cx);
            assert_eq!(view.read(cx).layout.panes.len(), remaining);
        });
    }
}
