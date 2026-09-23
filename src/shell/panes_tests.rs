//! Real pane controls exercised through the same AccessKit actions as the AX door.
use super::*;
use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ElementId, TestAppContext, VisualTestContext, px, size};

fn draw(window: &mut Window, cx: &mut gpui_kit::App) -> serde_json::Value {
    window.activate_a11y();
    window.render_frame(cx);
    window.render_frame(cx);
    serde_json::to_value(crate::ax::snapshot("console", window, false)).unwrap()
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

/// A console window on a desk with a program to show, as the AX door sees it.
fn console(
    cx: &mut TestAppContext,
) -> (
    Entity<Desktop>,
    WindowKey,
    Entity<DesktopWindow>,
    VisualTestContext,
) {
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
            desk_bounds: None,
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
            drag: None,
            inputs: HashMap::new(),
            spotlight_focused: false,
            focus: cx.focus_handle(),
            _activation: cx.observe_window_activation(window, |_, _, _| {}),
            _observer: cx.observe(&model, |_, _, cx| cx.notify()),
            _keystrokes: DesktopWindow::intercept_global_keys(window, cx),
            _focus_lost: cx.on_focus_lost(window, |this, window, cx| this.focus_lost(window, cx)),
        });
        view = Some(desktop.clone());
        gpui_kit::component::Root::new(desktop, window, cx)
    });
    let view = view.unwrap();
    model.update(cx, |model, _| {
        model.windows.insert(key, handle.into());
        model.views.insert(key, view.downgrade());
    });
    (
        model,
        key,
        view,
        VisualTestContext::from_window(handle.into(), cx),
    )
}

#[gpui_kit::test]
fn pane_strip_ax_actions_split_close_and_move_instances(cx: &mut TestAppContext) {
    let (model, key, view, mut native) = console(cx);
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
    for expected in 2..=layout::MAX_PANES {
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
        assert_eq!(view.read(cx).layout.panes.len(), layout::MAX_PANES);
    });
    let instance = native.update(|_, cx| view.read(cx).layout.panes[0].instance);
    native.update(|window, cx| press("pane/0/popout", window, cx));
    native.run_until_parked();
    let (popped_key, popped_handle, popped) = native.update(|_, cx| {
        assert_eq!(view.read(cx).layout.panes.len(), layout::MAX_PANES - 1);
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
        assert_eq!(view.read(cx).layout.panes.len(), layout::MAX_PANES);
        assert_eq!(
            view.read(cx).layout.panes[layout::MAX_PANES - 1].instance,
            instance
        );
        assert!(popped.read(cx).layout.panes.is_empty());
        model.update(cx, |model, _| {
            model.views.remove(&popped_key);
            model.windows.remove(&popped_key);
        });
    });
    for remaining in (0..layout::MAX_PANES).rev() {
        native.update(|window, cx| press("pane/0/close", window, cx));
        native.update(|window, cx| {
            draw(window, cx);
            assert_eq!(view.read(cx).layout.panes.len(), remaining);
        });
    }
}

#[gpui_kit::test]
fn a_press_on_a_window_behind_brings_it_to_the_front(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = console(cx);
    native.update(|window, cx| press("pane/0/split", window, cx));
    native.update(|window, cx| {
        draw(window, cx);
        assert_eq!(view.read(cx).layout.focused, 1);
    });
    // the first window fills the desk; the second covers its top-left
    let behind = gpui_kit::point(px(1200.), px(700.));
    native.simulate_click(behind, gpui_kit::Modifiers::none());
    native.update(|window, cx| {
        draw(window, cx);
        let layout = &view.read(cx).layout;
        assert_eq!(layout.focused, 0);
        assert_eq!(layout.stacking(), vec![1, 0]);
    });
    // a double press where both title bars lie fills the front window alone
    let title = gpui_kit::point(px(600.), px(desk::BAR + 42.));
    native.simulate_click(title, gpui_kit::Modifiers::none());
    native.simulate_event(gpui_kit::MouseDownEvent {
        button: gpui_kit::MouseButton::Left,
        position: title,
        modifiers: gpui_kit::Modifiers::none(),
        click_count: 2,
        first_mouse: false,
    });
    native.update(|window, cx| {
        draw(window, cx);
        let layout = &view.read(cx).layout;
        assert!(layout.panes[0].restore.is_some(), "the front window filled");
        assert!(
            layout.panes[1].restore.is_none(),
            "the one behind heard nothing"
        );
    });
}
