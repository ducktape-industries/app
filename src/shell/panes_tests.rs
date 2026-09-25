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
        state.stage = crate::Stage::Desk;
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
        model.state.console_win = Some(key);
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
            assert_eq!(view.read(cx).layout(cx).panes.len(), expected);
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
        assert_eq!(view.read(cx).layout(cx).panes.len(), layout::MAX_PANES);
    });
    let instance = native.update(|_, cx| view.read(cx).layout(cx).panes[0].instance);
    native.update(|window, cx| press("pane/0/popout", window, cx));
    // the model put the pane in a window of its own and asked the shell to
    // open it; the test has no command loop, so it opens it the same way
    native.update(|_, cx| {
        let (&popped, own) = model
            .read(cx)
            .state
            .layouts
            .iter()
            .find(|(candidate, _)| **candidate != key)
            .expect("the pane left for a window of its own");
        let kind = WindowKind::View {
            module: own.panes[0].module,
        };
        model.update(cx, |model, cx| {
            model.open_window(popped, kind, oneshot::channel().0, None, cx)
        });
    });
    native.run_until_parked();
    let (popped_key, popped_handle, popped) = native.update(|_, cx| {
        assert_eq!(view.read(cx).layout(cx).panes.len(), layout::MAX_PANES - 1);
        let model = model.read(cx);
        let (&key, popped) = model
            .views
            .iter()
            .find(|(candidate, _)| **candidate != key)
            .expect("popout opened a view window");
        let popped = popped.upgrade().unwrap();
        assert_eq!(popped.read(cx).layout(cx).panes[0].instance, instance);
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
        assert_eq!(view.read(cx).layout(cx).panes.len(), layout::MAX_PANES);
        assert_eq!(
            view.read(cx).layout(cx).panes[layout::MAX_PANES - 1].instance,
            instance
        );
        assert!(popped.read(cx).layout(cx).panes.is_empty());
        model.update(cx, |model, _| {
            model.views.remove(&popped_key);
            model.windows.remove(&popped_key);
        });
    });
    for remaining in (0..layout::MAX_PANES).rev() {
        native.update(|window, cx| press("pane/0/close", window, cx));
        native.update(|window, cx| {
            draw(window, cx);
            assert_eq!(view.read(cx).layout(cx).panes.len(), remaining);
        });
    }
}

#[gpui_kit::test]
fn a_press_on_a_window_behind_brings_it_to_the_front(cx: &mut TestAppContext) {
    let (model, _, view, mut native) = console(cx);
    native.update(|window, cx| press("pane/0/split", window, cx));
    native.update(|window, cx| {
        draw(window, cx);
        assert_eq!(view.read(cx).layout(cx).focused, 1);
    });
    // the first window fills the desk; the second covers its top-left
    let behind = gpui_kit::point(px(1200.), px(700.));
    // over a menu's backdrop the press closes the menu, and raises nothing
    model.update(&mut native, |model, _| {
        model.state.overlay = Some(crate::Overlay::Menu(crate::Popover::Account))
    });
    native.update(|window, cx| {
        draw(window, cx);
    });
    native.simulate_click(behind, gpui_kit::Modifiers::none());
    native.update(|window, cx| {
        draw(window, cx);
        assert_eq!(view.read(cx).layout(cx).focused, 1, "raised under a menu");
        assert_eq!(model.read(cx).state.overlay, None);
    });
    native.simulate_click(behind, gpui_kit::Modifiers::none());
    native.update(|window, cx| {
        draw(window, cx);
        let layout = view.read(cx).layout(cx);
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
        let layout = view.read(cx).layout(cx);
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
        let layout = view.read(cx).layout(cx);
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

    for name in ALL_OVERLAYS {
        model.update(&mut native, |model, _| model.state.overlay = Some(name));
        assert_eq!(
            native.update(|_, cx| view.read(cx).command_w_pane(cx)),
            None,
            "⌘W under {name:?} closes the window, not a pane"
        );
        for stroke in ["secondary-d", "secondary-2"] {
            key(&mut native, stroke);
            assert_eq!(
                panes(&mut native, &view),
                (2, 0),
                "{stroke} reached the desk under {name:?}"
            );
        }
        model.update(&mut native, |model, _| model.state.overlay = None);
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

const ALL_OVERLAYS: [crate::Overlay; 7] = [
    crate::Overlay::Spotlight,
    crate::Overlay::Approve,
    crate::Overlay::Settings,
    crate::Overlay::Network,
    crate::Overlay::Menu(crate::Popover::Node),
    crate::Overlay::Menu(crate::Popover::Account),
    crate::Overlay::Menu(crate::Popover::Notifications),
];

/// ⌘K opens and closes Spotlight; Escape closes whatever is open, and a
/// click on its backdrop does too.
#[gpui_kit::test]
fn command_k_toggles_spotlight_and_escape_closes_any_overlay(cx: &mut TestAppContext) {
    let (model, _, _, mut native) = console(cx);
    let open = |native: &mut VisualTestContext| native.update(|_, cx| model.read(cx).state.overlay);
    key(&mut native, "secondary-k");
    assert_eq!(open(&mut native), Some(crate::Overlay::Spotlight));
    key(&mut native, "secondary-k");
    assert_eq!(open(&mut native), None);
    for overlay in ALL_OVERLAYS {
        model.update(&mut native, |model, _| model.state.overlay = Some(overlay));
        key(&mut native, "escape");
        assert_eq!(open(&mut native), None, "Escape left {overlay:?} open");
        // a click outside its card, low on the window: on its backdrop
        model.update(&mut native, |model, _| model.state.overlay = Some(overlay));
        native.update(|window, cx| {
            draw(window, cx);
        });
        native.simulate_click(
            gpui_kit::point(px(640.), px(790.)),
            gpui_kit::Modifiers::none(),
        );
        assert_eq!(
            open(&mut native),
            None,
            "its backdrop left {overlay:?} open"
        );
    }
}

/// The window in front is the model's active program, however it got
/// there, and a window's own change asks nothing back of the desk.
#[gpui_kit::test]
fn the_focused_window_is_the_active_program(cx: &mut TestAppContext) {
    let (model, _, view, mut native) = console(cx);
    native.update(|window, cx| {
        draw(window, cx);
    });
    let active =
        |native: &mut VisualTestContext| native.update(|_, cx| model.read(cx).state.active);
    native.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.pane_message(PaneMessage::Split("pane-ax-other"), window, cx)
        })
    });
    assert_eq!(active(&mut native), Some("pane-ax-other"));
    native.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.pane_message(PaneMessage::Focus(0), window, cx)
        })
    });
    assert_eq!(active(&mut native), Some("pane-ax-test"));
    key(&mut native, "secondary-2");
    assert_eq!(active(&mut native), Some("pane-ax-other"));
    // the model's own ask (Spotlight) is carried out by the window
    model.update(&mut native, |model, cx| {
        model.dispatch(Message::SelectView("pane-ax-test"), cx)
    });
    native.run_until_parked();
    native.update(|window, cx| {
        draw(window, cx);
    });
    assert_eq!(panes(&mut native, &view), (2, 0));
    assert_eq!(active(&mut native), Some("pane-ax-test"));
}

/// The empty window keeps the design's spacing while its rows fit, tightens
/// as the window gets short, and past that shows whole rows only.
#[test]
fn a_short_empty_window_tightens_its_rows() {
    use super::panes::{EmptySpacing, empty_spacing};
    let spacing = |row, outer| EmptySpacing {
        row,
        outer,
        shown: None,
    };
    assert_eq!(empty_spacing(6, 600.), spacing(10., 28.));
    assert_eq!(empty_spacing(6, 360.), spacing(6., 16.));
    assert_eq!(empty_spacing(6, 290.), spacing(3., 10.));
    // 185 tall: 185 - 20 - 76 = 89 of room, two whole 30px rows
    assert_eq!(empty_spacing(6, 185.).shown, Some(60.));
}
