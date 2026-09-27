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
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
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
    let first = native.update(|window, cx| {
        draw(window, cx);
        let layout = view.read(cx).layout(cx);
        assert_eq!(layout.focused, 1);
        layout.panes[0].frame.unwrap()
    });
    // the second cascades down and right of the first: the first's
    // top-left corner stays uncovered
    let behind = gpui_kit::point(px(first.x + 14.), px(desk::BAR + first.y + 14.));
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
    let title = gpui_kit::point(px(first.x + 100.), px(desk::BAR + first.y + 30.));
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

/// What had the keys before something opened over the desk has them again
/// once it closes, however it closed: typing carries on where it was.
#[gpui_kit::test]
fn closing_an_overlay_gives_the_keys_back_to_what_had_them(cx: &mut TestAppContext) {
    let (model, _, _, mut native) = console(cx);
    let focused = |native: &mut VisualTestContext| native.update(|window, cx| window.focused(cx));
    key(&mut native, "tab");
    let before = focused(&mut native);
    assert!(before.is_some());
    key(&mut native, "secondary-k");
    native.run_until_parked();
    assert_ne!(
        focused(&mut native),
        before,
        "Spotlight's field takes the keys"
    );
    key(&mut native, "escape");
    native.run_until_parked();
    native.update(|window, cx| {
        draw(window, cx);
    });
    assert_eq!(focused(&mut native), before, "Escape");
    // a bar menu, closed by the model (a click outside, a pick)
    model.update(&mut native, |model, _| {
        model.state.overlay = Some(crate::Overlay::Menu(crate::Popover::Node))
    });
    native.update(|window, cx| {
        draw(window, cx);
    });
    model.update(&mut native, |model, cx| {
        model.dispatch(
            Message::CloseOverlay(crate::Overlay::Menu(crate::Popover::Node)),
            cx,
        )
    });
    native.run_until_parked();
    native.update(|window, cx| {
        draw(window, cx);
    });
    assert_eq!(focused(&mut native), before, "a menu");
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

/// A window that comes to the front (⌘D, ⌘W, ⌘1…9, ⌃Tab) has the keys:
/// what had them in it before, else its first control, else the window
/// itself — never the root, where typing goes nowhere.
#[gpui_kit::test]
fn a_window_brought_to_the_front_has_the_keys(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = console(cx);
    let focused = |native: &mut VisualTestContext| native.update(|window, cx| window.focused(cx));
    let in_front = |native: &mut VisualTestContext| {
        native.update(|window, cx| {
            let view = view.read(cx);
            let layout = view.layout(cx);
            let instance = layout.panes[layout.focused].instance;
            view.pane_keys[&instance].0.contains_focused(window, cx)
        })
    };
    key(&mut native, "secondary-d");
    assert!(in_front(&mut native), "⌘D: the new window");
    key(&mut native, "secondary-1");
    assert!(in_front(&mut native), "⌘1");
    let first = focused(&mut native);
    key(&mut native, "ctrl-tab");
    assert!(in_front(&mut native), "⌃Tab");
    assert_ne!(focused(&mut native), first);
    key(&mut native, "secondary-w");
    assert_eq!(panes(&mut native, &view), (1, 0));
    assert_eq!(focused(&mut native), first, "⌘W: back to what had them");
}

/// A dialog on a scrim is modal: Tab and Shift+Tab go round its controls
/// and never reach the bar behind it.
#[gpui_kit::test]
fn tab_stays_in_a_modal_dialog(cx: &mut TestAppContext) {
    let (model, _, view, mut native) = console(cx);
    model.update(&mut native, |model, _| {
        model.state.overlay = Some(crate::Overlay::Settings)
    });
    native.update(|window, cx| {
        draw(window, cx);
    });
    let mut seen = Vec::new();
    for stroke in ["tab"; 12].into_iter().chain(["shift-tab"; 12]) {
        key(&mut native, stroke);
        native.update(|window, cx| {
            assert!(
                view.read(cx).modal.contains_focused(window, cx),
                "{stroke} left Settings"
            );
            let now = window.focused(cx);
            if !seen.contains(&now) {
                seen.push(now);
            }
        });
    }
    assert!(seen.len() > 2, "Tab went round Settings' controls");
}

/// An empty window's field has the keys as soon as the window opens: what
/// is typed narrows the programs, ↓ picks the next, Enter opens it there.
#[gpui_kit::test]
fn an_empty_window_opens_what_its_field_finds(cx: &mut TestAppContext) {
    crate::runtime::list_for_test("cmdtest-alpha");
    crate::runtime::list_for_test("cmdtest-beta");
    let (_, _, view, mut native) = console(cx);
    native.update(|window, cx| {
        draw(window, cx);
        window.dispatch_action(Box::new(keys::NewWindow), cx);
    });
    native.update(|window, cx| {
        draw(window, cx);
    });
    native.update(|window, cx| {
        draw(window, cx);
    });
    native.simulate_input("cmdtest");
    native.simulate_keystrokes("down tab enter");
    native.update(|window, cx| {
        draw(window, cx);
        let layout = view.read(cx).layout(cx);
        assert_eq!(layout.panes[1].module, "cmdtest-beta");
    });
}
