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
        Desktop::new(state, crate::tray::init(cx).0)
    });
    let key = WindowKey::unique();
    let mut view = None;
    let handle = cx.open_window(size(px(1280.), px(800.)), |window, cx| {
        let desktop =
            cx.new(|cx| DesktopWindow::new(model.clone(), key, WindowKind::Console, window, cx));
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

fn key(native: &mut VisualTestContext, stroke: &str) {
    native.update(|window, cx| {
        draw(window, cx);
        window.dispatch_keystroke(gpui_kit::Keystroke::parse(stroke).unwrap(), cx);
        draw(window, cx);
    });
}

fn panes(native: &mut VisualTestContext, view: &Entity<DesktopWindow>) -> (usize, usize) {
    native.update(|_, cx| {
        let layout = &view.read(cx).layout;
        (layout.panes.len(), layout.focused)
    })
}

/// The desk's own keys (⌘D, ⌘1, ⌘W) act on its windows only while no
/// overlay is open; an overlay keeps its keys, and ⌘W is then the app
/// window's (checked through `command_w_pane`: on Linux that window
/// minimizes, which the test platform can't).
#[gpui_kit::test]
fn desk_keys_act_on_windows_only_with_no_overlay_open(cx: &mut TestAppContext) {
    let (model, _, view, mut native) = console(cx);
    native.update(|window, cx| {
        draw(window, cx);
    });
    assert_eq!(panes(&mut native, &view), (1, 0));
    key(&mut native, "secondary-d");
    assert_eq!(panes(&mut native, &view), (2, 1), "⌘D halves the window");
    key(&mut native, "secondary-1");
    assert_eq!(panes(&mut native, &view), (2, 0), "⌘1 focuses the first");

    type Open = fn(&mut Ducktape);
    let overlays: [(&str, Open); 5] = [
        ("spotlight", |state| state.spotlight = true),
        ("approve", |state| state.approving = true),
        ("settings", |state| state.settings = true),
        ("network menu", |state| state.network_menu = true),
        ("popover", |state| {
            state.popover = Some(crate::Popover::Node)
        }),
    ];
    for (name, open) in overlays {
        model.update(&mut native, |model, _| open(&mut model.state));
        assert_eq!(
            native.update(|_, cx| view.read(cx).command_w_pane(cx)),
            None,
            "⌘W under the {name} closes the window, not a pane"
        );
        for stroke in ["secondary-d", "secondary-2"] {
            key(&mut native, stroke);
            assert_eq!(
                panes(&mut native, &view),
                (2, 0),
                "{stroke} reached the desk under the {name}"
            );
        }
        model.update(&mut native, |model, _| {
            let state = &mut model.state;
            state.spotlight = false;
            state.approving = false;
            state.settings = false;
            state.network_menu = false;
            state.popover = None;
        });
    }

    assert_eq!(
        native.update(|_, cx| view.read(cx).command_w_pane(cx)),
        Some(0)
    );
    key(&mut native, "secondary-w");
    assert_eq!(
        panes(&mut native, &view).0,
        1,
        "⌘W closes the focused window"
    );
    key(&mut native, "secondary-w");
    assert_eq!(panes(&mut native, &view).0, 0);
    assert_eq!(
        native.update(|_, cx| view.read(cx).command_w_pane(cx)),
        None,
        "with no window left, ⌘W is the app window's"
    );
}

/// ⌘K opens and closes Spotlight; Escape closes Settings and a menu.
#[gpui_kit::test]
fn command_k_toggles_spotlight_and_escape_closes_settings_and_menus(cx: &mut TestAppContext) {
    let (model, _, _, mut native) = console(cx);
    let read = |native: &mut VisualTestContext| {
        native.update(|_, cx| {
            let state = &model.read(cx).state;
            (state.spotlight, state.settings, state.popover)
        })
    };
    key(&mut native, "secondary-k");
    assert_eq!(read(&mut native), (true, false, None));
    key(&mut native, "secondary-k");
    assert_eq!(read(&mut native), (false, false, None));
    model.update(&mut native, |model, _| model.state.settings = true);
    key(&mut native, "escape");
    assert_eq!(read(&mut native), (false, false, None));
    model.update(&mut native, |model, _| {
        model.state.popover = Some(crate::Popover::Account)
    });
    key(&mut native, "escape");
    assert_eq!(read(&mut native), (false, false, None));
}
