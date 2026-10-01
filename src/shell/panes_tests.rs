//! Real pane controls exercised through the same AccessKit actions as the AX door.
use super::entities::tests::{active, status};
use super::entities::{Entities, Overlay, Popover, SettingsPage, Spot};
use super::layers::tests::{
    frame, line, open_help, open_now, pane, polled, popped, run_spot, select_view, set_motion,
    show, toast, tree_renders,
};
use super::*;
use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, Window, px, size};

pub(super) fn draw(window: &mut Window, cx: &mut gpui_kit::App) -> serde_json::Value {
    window.activate_a11y();
    window.render_frame(cx);
    // the frame's callbacks (the desk's size, a switch's end) run before
    // the next frame, as the platform delivers them
    window.simulate_next_frame(cx);
    window.render_frame(cx);
    serde_json::to_value(crate::ax::snapshot("console", window, false)).unwrap()
}

/// The ids in a snapshot, in the tree's order.
pub(super) fn ids(nodes: &serde_json::Value) -> Vec<String> {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str().map(str::to_owned))
        .collect()
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
pub(super) fn console(
    cx: &mut TestAppContext,
) -> (Entities, WindowKey, Entity<WindowRoot>, VisualTestContext) {
    let mut seed = super::layers::tests::Seed::boot();
    // its own roster and centre, not the app's ones every test shares
    seed.roster = Default::default();
    seed.center = Default::default();
    seed.screen = super::entities::Screen::Desk;
    seed.active = Some("pane-ax-test");
    super::layers::tests::open_console(seed, cx)
}

#[gpui_kit::test]
fn pane_strip_ax_actions_split_close_and_move_instances(cx: &mut TestAppContext) {
    let (app, key, view, mut native) = console(cx);
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
    // the pane leaves for a window of its own, which `Windows` opens
    native.update(|window, cx| press("pane/0/popout", window, cx));
    native.run_until_parked();
    let (popped_key, popped_handle, popped) = popped(&app, key, &mut native);
    native.update(|_, cx| {
        assert_eq!(view.read(cx).layout(cx).panes.len(), layout::MAX_PANES - 1);
        assert_eq!(popped.read(cx).layout(cx).panes[0].instance, instance);
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
        // its window went with it, and `Windows` heard so
        assert!(
            !app.windows.read(cx).handles().contains_key(&popped_key),
            "the pop-out's window is still listed"
        );
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
    let (_, _, view, mut native) = console(cx);
    native.update(|window, cx| press("pane/0/split", window, cx));
    let first = native.update(|window, cx| {
        draw(window, cx);
        let layout = view.read(cx).layout(cx);
        assert_eq!(layout.focused, 1);
        layout.panes[0].frame.unwrap()
    });
    // the second cascades down and right of the first: the first's
    // top-left corner stays uncovered
    let behind = gpui_kit::point(px(first.x + 14.), px(layers::BAR + first.y + 14.));
    // over a menu's backdrop the press closes the menu, and raises nothing
    show(&view, Some(Overlay::Menu(Popover::Account)), &mut native);
    native.update(|window, cx| {
        draw(window, cx);
    });
    native.simulate_click(behind, gpui_kit::Modifiers::none());
    native.update(|window, cx| {
        draw(window, cx);
        assert_eq!(view.read(cx).layout(cx).focused, 1, "raised under a menu");
    });
    assert_eq!(open_now(&view, &mut native), None);
    native.simulate_click(behind, gpui_kit::Modifiers::none());
    native.update(|window, cx| {
        draw(window, cx);
        let layout = view.read(cx).layout(cx);
        assert_eq!(layout.focused, 0);
        assert_eq!(layout.stacking(), vec![1, 0]);
    });
    settle(&mut native);
    assert!(
        in_front(&mut native, &view),
        "the raised window has the keys"
    );
    let keys = native.update(|window, cx| window.focused(cx));
    // a double press where both title bars lie fills the front window alone
    let title = gpui_kit::point(px(first.x + 100.), px(layers::BAR + first.y + 30.));
    native.simulate_click(title, gpui_kit::Modifiers::none());
    settle(&mut native);
    assert_eq!(
        native.update(|window, cx| window.focused(cx)),
        keys,
        "a press on the front window's title bar moved the keys"
    );
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
    settle(&mut native);
    assert_eq!(
        native.update(|window, cx| window.focused(cx)),
        keys,
        "filling the front window moved the keys"
    );
}

pub(super) fn key(native: &mut VisualTestContext, stroke: &str) {
    native.update(|window, cx| {
        draw(window, cx);
        window.dispatch_keystroke(gpui_kit::Keystroke::parse(stroke).unwrap(), cx);
        draw(window, cx);
    });
}

pub(super) fn panes(native: &mut VisualTestContext, view: &Entity<WindowRoot>) -> (usize, usize) {
    native.update(|_, cx| {
        let layout = view.read(cx).layout(cx);
        (layout.panes.len(), layout.focused)
    })
}

/// The desk's own keys (⌘N, ⌘1, ⌘W) act on its windows only while no
/// overlay is open; an overlay keeps its keys, and ⌘W is then the app
/// window's (checked through `command_w_pane`: on Linux that window
/// minimizes, which the test platform can't).
#[gpui_kit::test]
fn desk_keys_act_on_windows_only_with_no_overlay_open(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = console(cx);
    native.update(|window, cx| {
        draw(window, cx);
    });
    assert_eq!(panes(&mut native, &view), (1, 0));
    key(&mut native, "secondary-n");
    assert_eq!(panes(&mut native, &view), (2, 1), "⌘N opens a window");
    key(&mut native, "secondary-1");
    assert_eq!(panes(&mut native, &view), (2, 0), "⌘1 focuses the first");

    for name in ALL_OVERLAYS {
        show(&view, Some(name), &mut native);
        assert_eq!(
            native.update(|_, cx| view.read(cx).command_w_pane(cx)),
            None,
            "⌘W under {name:?} closes the window, not a pane"
        );
        for stroke in ["secondary-n", "secondary-2"] {
            key(&mut native, stroke);
            assert_eq!(
                panes(&mut native, &view),
                (2, 0),
                "{stroke} reached the desk under {name:?}"
            );
        }
        show(&view, None, &mut native);
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

const ALL_OVERLAYS: [Overlay; 7] = [
    Overlay::Spotlight,
    Overlay::Approve,
    Overlay::Settings(SettingsPage::Appearance),
    Overlay::Network,
    Overlay::Menu(Popover::Node),
    Overlay::Menu(Popover::Account),
    Overlay::Menu(Popover::Notifications),
];

/// ⌘K opens and closes Spotlight; Escape closes whatever is open, and a
/// click on its backdrop does too.
#[gpui_kit::test]
fn command_k_toggles_spotlight_and_escape_closes_any_overlay(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = console(cx);
    let open = |native: &mut VisualTestContext| open_now(&view, native);
    key(&mut native, "secondary-k");
    assert_eq!(open(&mut native), Some(Overlay::Spotlight));
    key(&mut native, "secondary-k");
    assert_eq!(open(&mut native), None);
    for overlay in ALL_OVERLAYS {
        show(&view, Some(overlay), &mut native);
        key(&mut native, "escape");
        assert_eq!(open(&mut native), None, "Escape left {overlay:?} open");
        // a click outside its card, low on the window: on its backdrop
        show(&view, Some(overlay), &mut native);
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
    let (_, _, view, mut native) = console(cx);
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
    // a bar menu, closed by a click outside or a pick
    show(&view, Some(Overlay::Menu(Popover::Node)), &mut native);
    native.update(|window, cx| {
        draw(window, cx);
    });
    show(&view, None, &mut native);
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
    let (app, _, view, mut native) = console(cx);
    native.update(|window, cx| {
        draw(window, cx);
    });
    native.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.pane_message(PaneMessage::Split("pane-ax-other"), window, cx)
        })
    });
    assert_eq!(active(&app, &native), Some("pane-ax-other"));
    native.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.pane_message(PaneMessage::Focus(0), window, cx)
        })
    });
    assert_eq!(active(&app, &native), Some("pane-ax-test"));
    key(&mut native, "secondary-2");
    assert_eq!(active(&app, &native), Some("pane-ax-other"));
    // a pick (Spotlight's) lands on the console's desk
    select_view(&app, "pane-ax-test", &mut native);
    native.update(|window, cx| {
        draw(window, cx);
    });
    assert_eq!(panes(&mut native, &view), (2, 0));
    assert_eq!(active(&app, &native), Some("pane-ax-test"));
}

/// A window that comes to the front (⌘N, ⌘W, ⌘1…9, ⌃Tab) has the keys:
/// what had them in it before, else its first control, else the window
/// itself — never the root, where typing goes nowhere.
#[gpui_kit::test]
fn a_window_brought_to_the_front_has_the_keys(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = console(cx);
    let focused = |native: &mut VisualTestContext| native.update(|window, cx| window.focused(cx));
    key(&mut native, "secondary-n");
    assert!(in_front(&mut native, &view), "⌘N: the new window");
    key(&mut native, "secondary-1");
    assert!(in_front(&mut native, &view), "⌘1");
    let first = focused(&mut native);
    key(&mut native, "ctrl-tab");
    assert!(in_front(&mut native, &view), "⌃Tab");
    assert_ne!(focused(&mut native), first);
    key(&mut native, "secondary-w");
    assert_eq!(panes(&mut native, &view), (1, 0));
    assert_eq!(focused(&mut native), first, "⌘W: back to what had them");
}

/// The window in front has the keys, somewhere in its own box.
pub(super) fn in_front(native: &mut VisualTestContext, view: &Entity<WindowRoot>) -> bool {
    native.update(|window, cx| {
        let view = view.read(cx);
        let layout = view.layout(cx);
        let instance = layout.panes[layout.focused].instance;
        view.panes.read(cx).pane_keys[&instance]
            .0
            .contains_focused(window, cx)
    })
}

/// Draws until the deferred focus moves have landed.
pub(super) fn settle(native: &mut VisualTestContext) {
    for _ in 0..3 {
        native.update(|window, cx| {
            draw(window, cx);
        });
        native.run_until_parked();
    }
}

/// A window the model brings to the front, not the desk's keys, has the
/// keys too: Help from ⌘/, the Help menu or the empty desk's button.
#[gpui_kit::test]
fn help_the_model_opens_has_the_keys(cx: &mut TestAppContext) {
    let (app, _, view, mut native) = console(cx);
    settle(&mut native);
    open_help(&app, &mut native);
    settle(&mut native);
    assert_eq!(panes(&mut native, &view), (2, 1));
    assert!(
        in_front(&mut native, &view),
        "the keys stayed out of Help: focused = {:?}",
        native.update(|window, cx| window.focused(cx))
    );
}

/// A dialog over the desk keeps the keys from a window the model opens
/// behind it (Help from the app's menu while Settings is open); that
/// window has them once the dialog closes.
#[gpui_kit::test]
fn a_dialog_keeps_the_keys_until_it_closes(cx: &mut TestAppContext) {
    let (app, _, view, mut native) = console(cx);
    settle(&mut native);
    show(
        &view,
        Some(Overlay::Settings(SettingsPage::Appearance)),
        &mut native,
    );
    settle(&mut native);
    open_help(&app, &mut native);
    settle(&mut native);
    assert_eq!(panes(&mut native, &view), (2, 1));
    native.update(|window, cx| {
        assert!(
            view.read(cx)
                .dialogs()
                .read(cx)
                .modal
                .contains_focused(window, cx),
            "Help took the keys from Settings"
        )
    });
    show(&view, None, &mut native);
    settle(&mut native);
    assert!(in_front(&mut native, &view), "Settings closed");
}

/// A program Spotlight or a menu picks comes to the front with the keys,
/// not the window it was picked over.
#[gpui_kit::test]
fn a_view_the_model_selects_has_the_keys(cx: &mut TestAppContext) {
    let (app, _, view, mut native) = console(cx);
    native.update(|window, cx| {
        draw(window, cx);
        view.update(cx, |view, cx| {
            view.pane_message(PaneMessage::Split("pane-ax-other"), window, cx)
        })
    });
    settle(&mut native);
    assert!(in_front(&mut native, &view), "the split has the keys");
    select_view(&app, "pane-ax-test", &mut native);
    settle(&mut native);
    assert_eq!(panes(&mut native, &view), (2, 0));
    assert!(in_front(&mut native, &view), "SelectView");
    // picked in Spotlight: it closes, and the keys go to the pick, not
    // back to what had them when it opened
    key(&mut native, "secondary-k");
    settle(&mut native);
    run_spot(&view, Spot::Open("pane-ax-other"), &mut native);
    settle(&mut native);
    assert_eq!(panes(&mut native, &view), (2, 1));
    assert!(in_front(&mut native, &view), "Spotlight");
}

/// A dialog on a scrim is modal: Tab and Shift+Tab go round its controls
/// and never reach the bar behind it.
#[gpui_kit::test]
fn tab_stays_in_a_modal_dialog(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = console(cx);
    show(
        &view,
        Some(Overlay::Settings(SettingsPage::Appearance)),
        &mut native,
    );
    native.update(|window, cx| {
        draw(window, cx);
    });
    let mut seen = Vec::new();
    for stroke in ["tab"; 12].into_iter().chain(["shift-tab"; 12]) {
        key(&mut native, stroke);
        native.update(|window, cx| {
            assert!(
                view.read(cx)
                    .dialogs()
                    .read(cx)
                    .modal
                    .contains_focused(window, cx),
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
    let (app, _, view, mut native) = console(cx);
    let roster = crate::runtime::Roster::listing(&["cmdtest-alpha", "cmdtest-beta"]);
    app.rail
        .update(&mut native, |rail, cx| rail.read_off(roster, cx));
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
    native.simulate_keystrokes("down enter");
    native.update(|window, cx| {
        draw(window, cx);
        let layout = view.read(cx).layout(cx);
        assert_eq!(layout.panes[1].module, "cmdtest-beta");
    });
}

/// ⌘/ opens Help in a window of its own: the app draws it, with no program
/// mounted behind it and no pop-out, since nothing there could leave.
#[gpui_kit::test]
fn help_opens_in_a_window_the_app_draws(cx: &mut TestAppContext) {
    let (app, _, view, mut native) = console(cx);
    native.update(|window, cx| {
        draw(window, cx);
        window.dispatch_action(Box::new(keys::OpenHelp), cx);
    });
    let nodes = native.update(draw);
    native.update(|_, cx| {
        let layout = view.read(cx).layout(cx);
        let index = layout
            .panes
            .iter()
            .position(|pane| pane.module == layout::HELP)
            .expect("help opened");
        let instance = layout.panes[index].instance;
        assert!(app.seats.read(cx).seat(instance).is_none());
        let ids: Vec<&str> = nodes
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|node| node["id"].as_str())
            .collect();
        assert!(ids.contains(&"console:help"), "{ids:?}");
        assert!(!ids.contains(&format!("console:pane/{index}/popout").as_str()));
    });
}

/// Help in a window narrower than its page keeps the page's width and
/// scrolls to the rest, rather than squeezing it.
#[gpui_kit::test]
fn a_narrow_help_window_scrolls_rather_than_squeezes(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = console(cx);
    native.update(|window, cx| {
        draw(window, cx);
        window.dispatch_action(Box::new(keys::OpenHelp), cx);
    });
    native.update(|window, cx| {
        draw(window, cx);
    });
    let narrow = layout::Frame {
        x: 20.,
        y: 20.,
        w: 320.,
        h: 400.,
    };
    let index = native.update(|_, cx| {
        view.read(cx)
            .layout(cx)
            .panes
            .iter()
            .position(|pane| pane.module == layout::HELP)
            .unwrap()
    });
    pane(&view, PaneMessage::Frame(index, narrow), &mut native);
    let width = |nodes: &serde_json::Value, id: &str| {
        nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == format!("console:{id}"))
            .and_then(|node| node["bounds"][2].as_i64())
            .unwrap_or_else(|| panic!("no {id}: {nodes}"))
    };
    native.update(|window, cx| {
        draw(window, cx);
        let nodes = serde_json::to_value(crate::ax::snapshot("console", window, true)).unwrap();
        let (shown, page) = (width(&nodes, "help"), width(&nodes, "help/page"));
        // the page runs past the window: there is something to scroll to
        assert!(page > shown, "squeezed: page {page}, window {shown}");
    });
}

/// A window placed before its view came widens once the view draws: the
/// tab seats it, says so (`Intent::Seated`), and the desk fits the window
/// to the view's minimum and its border.
#[gpui_kit::test]
fn a_view_that_draws_widens_the_window_it_came_to(cx: &mut TestAppContext) {
    const MODULE: &str = "pane-seated-view";
    let (app, _, view, mut native) = console(cx);
    native.update(|window, cx| {
        draw(window, cx);
    });
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    let width = |native: &mut VisualTestContext| {
        native.update(|_, cx| view.read(cx).layout(cx).panes[0].frame.unwrap().w)
    };
    assert_eq!(width(&mut native), 768., "60% of the desk, no view yet");
    let tab = native.update(|_, cx| {
        let instance = view.read(cx).layout(cx).panes[0].instance;
        app.seats
            .read(cx)
            .seat(instance)
            .expect("a seat for the pane")
    });
    let said = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _heard = native.update(|_, cx| {
        let said = said.clone();
        cx.subscribe(&tab, move |_, intent: &crate::runtime::Intent, _| {
            said.borrow_mut().push(intent.clone())
        })
    });
    crate::runtime::seat_for_test(MODULE, 1000);
    native.update(|window, cx| {
        draw(window, cx);
    });
    native.run_until_parked();
    assert!(
        said.borrow().contains(&crate::runtime::Intent::Seated),
        "{:?}",
        said.borrow()
    );
    assert_eq!(width(&mut native), 1002.);
}

/// A window redraws for whatever moved around its panes (a menu, the bar,
/// the toast). With nothing in the pane changed that redraw must not
/// render a seated view's tree again: the counts of
/// `GET /perf`, with no `draw` helper here (its `activate_a11y` refreshes
/// the window: every cache misses once). Times are flaky under the test scheduler; counts are not.
#[gpui_kit::test]
fn a_desk_redraw_with_nothing_changed_renders_no_view_tree(cx: &mut TestAppContext) {
    const MODULE: &str = "pane-desk-redraw-view";
    let _on = crate::perf::on_for_test();
    let (_, key, view, mut native) = console(cx);
    frame(&mut native);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    for _ in 0..8 {
        frame(&mut native);
    }
    let settled = tree_renders(MODULE);
    assert!(settled > 0, "the view drew its tree once");
    let desk = window_count(key, "renders");
    for _ in 0..3 {
        view.update(&mut native, |_, cx| cx.notify());
        for _ in 0..3 {
            frame(&mut native);
        }
    }
    assert!(
        window_count(key, "renders") >= desk + 3,
        "the desk drew again"
    );
    assert_eq!(
        tree_renders(MODULE),
        settled,
        "a redraw of the desk rendered the view's tree again"
    );
}

/// A window's count under `stage` in the perf registry.
pub(super) fn window_count(key: WindowKey, stage: &str) -> u64 {
    crate::perf::snapshot(false)["windows"][key.0.to_string()][stage]
        .as_u64()
        .unwrap_or(0)
}

/// A modifier key going down or up changes nothing on screen, so it draws
/// no frame.
#[gpui_kit::test]
fn a_modifier_press_draws_no_frame(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (_, key, _, mut native) = console(cx);
    native.run_until_parked();
    let still = window_count(key, "renders");
    assert!(still > 0, "the window never drew");
    for modifiers in [gpui_kit::Modifiers::control(), gpui_kit::Modifiers::none()] {
        native.simulate_modifiers_change(modifiers);
        native.run_until_parked();
    }
    assert_eq!(
        window_count(key, "renders"),
        still,
        "a modifier key drew the window"
    );
}

/// A Help window brought to the front keeps the keys in its own box: it
/// draws no finder field, so nothing off-screen may take them.
#[gpui_kit::test]
fn a_help_window_in_front_keeps_the_keys_in_its_box(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = console(cx);
    native.update(|window, cx| {
        draw(window, cx);
        window.dispatch_action(Box::new(keys::OpenHelp), cx);
    });
    let index = native.update(|window, cx| {
        draw(window, cx);
        let layout = view.read(cx).layout(cx);
        layout
            .panes
            .iter()
            .position(|pane| pane.module == layout::HELP)
            .expect("help opened")
    });
    // brought to the front by a desk key, as the other windows are
    key(&mut native, "secondary-1");
    key(&mut native, &format!("secondary-{}", index + 1));
    // one draw restores the focus, the next sees where it landed
    for _ in 0..3 {
        native.update(|window, cx| {
            draw(window, cx);
        });
        native.run_until_parked();
    }
    native.update(|window, cx| {
        let view = view.read(cx);
        let layout = view.layout(cx);
        assert_eq!(layout.focused, index, "Help is in front");
        let own = view.panes.read(cx).pane_keys[&layout.panes[index].instance]
            .0
            .clone();
        assert!(
            own.contains_focused(window, cx),
            "the keys left the Help window: focused = {:?}",
            window.focused(cx)
        );
    });
}

/// A window held by its title bar follows the moves that come before the
/// next frame: a quick hand's, or the AX door's drag, which presses, moves
/// and lets go in one go.
#[gpui_kit::test]
fn help_follows_moves_that_come_before_a_frame(cx: &mut TestAppContext) {
    use gpui_kit::{MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput};
    let (app, _, view, mut native) = console(cx);
    settle(&mut native);
    open_help(&app, &mut native);
    settle(&mut native);
    let (index, start) = native.update(|_, cx| {
        let layout = view.read(cx).layout(cx);
        (layout.focused, layout.panes[layout.focused].frame.unwrap())
    });
    let title = gpui_kit::point(px(start.x + 100.), px(layers::BAR + start.y + 15.));
    let to = gpui_kit::point(title.x + px(40.), title.y + px(30.));
    // one update: no frame is drawn between the press and the move
    native.update(|window, cx| {
        for event in [
            PlatformInput::MouseDown(MouseDownEvent {
                position: title,
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            PlatformInput::MouseMove(MouseMoveEvent {
                position: to,
                pressed_button: Some(MouseButton::Left),
                modifiers: Default::default(),
            }),
            PlatformInput::MouseUp(MouseUpEvent {
                position: to,
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
            }),
        ] {
            window.dispatch_event(event, cx);
        }
    });
    let frame = native.update(|_, cx| view.read(cx).layout(cx).panes[index].frame.unwrap());
    assert_eq!(
        frame,
        layout::Frame {
            x: start.x + 40.,
            y: start.y + 30.,
            ..start
        },
        "Help stayed where the press found it"
    );
    assert!(
        native.update(|_, cx| view.read(cx).panes.read(cx).drag.is_none()),
        "the release did not let go"
    );
}

/// A row of the bell's list clicked closes the menu and opens what the
/// notice is about: its program's seat on the console (`Windows::open_notice`).
#[gpui_kit::test]
fn a_bell_row_opens_the_notices_program(cx: &mut TestAppContext) {
    const MODULE: &str = "pane-bell-row-view";
    let (app, _, view, mut native) = console(cx);
    let roster = crate::runtime::Roster::listing(&[MODULE]);
    app.rail
        .update(&mut native, |rail, cx| rail.read_off(roster, cx));
    let id = native.update(|_, cx| {
        let notifications = app.notifications.read(cx);
        let settings = app.prefs.read(cx).get().notify.clone();
        let post = view_wire::methods::Notification {
            title: "Ping".into(),
            body: "From the bell".into(),
            tag: String::new(),
            link: String::new(),
        };
        let _ = notifications.center().lock().post(
            &settings,
            MODULE,
            "Bell",
            post,
            std::time::Instant::now(),
            0,
        );
        notifications.entries()[0].id
    });
    app.notifications.update(&mut native, |notifications, cx| {
        _ = notifications.refresh(cx)
    });
    show(
        &view,
        Some(Overlay::Menu(Popover::Notifications)),
        &mut native,
    );
    native.update(|window, cx| press(&format!("notif/{id}"), window, cx));
    settle(&mut native);
    assert_eq!(
        native.update(|_, cx| *view.read(cx).overlays().read(cx).get()),
        None,
        "the bell stayed open"
    );
    assert!(
        native.update(|_, cx| view.read(cx).desk.read(cx).holds(MODULE)),
        "the notice's program never opened"
    );
}

/// A pane that leaves the desk with an intent still to hand over (a badge
/// its last update set) gets it to the rail: `hide` returns them and the
/// reconcile routes them, since the seat's own route is dropped in the
/// same update.
#[gpui_kit::test]
fn a_hidden_seats_intents_still_arrive(cx: &mut TestAppContext) {
    const MODULE: &str = "pane-hidden-intent-view";
    let (app, _, view, mut native) = console(cx);
    crate::runtime::seat_for_test(MODULE, 400);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    let (index, instance) = native.update(|_, cx| {
        let layout = view.read(cx).layout(cx);
        let index = layout
            .panes
            .iter()
            .position(|pane| pane.module == MODULE)
            .expect("the pane opened");
        let instance = layout.panes[index].instance;
        let seat = app.seats.read(cx).seat(instance).expect("seated");
        (index, seat.read(cx).instance())
    });
    crate::runtime::intent_for_test(MODULE, instance, crate::runtime::Intent::Badge(3));
    pane(&view, PaneMessage::Close(index), &mut native);
    assert_eq!(
        native.update(|_, cx| app.rail.read(cx).badge(MODULE)),
        3,
        "the badge the hidden seat handed over reached the rail"
    );
}

/// Closing a pane drops its seat, and the pictures its view drew leave
/// the window's GPU atlas with it.
#[gpui_kit::test]
fn a_closed_panes_pictures_leave_the_atlas(cx: &mut TestAppContext) {
    use gpui_kit::Styled as _;
    const MODULE: &str = "pane-closed-picture-view";
    const HASH: u64 = 31;
    let (app, _, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(
        MODULE,
        400,
        view_wire::Node::Image {
            id: Some(view_wire::ElementIdWire::Name("picture".into())),
            hash: HASH,
            data: Some(view_wire::ImageData::Rgba {
                width: 2,
                height: 2,
                pixels: [255, 0, 0, 255].repeat(4),
            }),
            label: None,
            image_style: view_wire::ImageStyle {
                grayscale: false,
                object_fit: view_wire::ImageObjectFit::Fill,
            },
            loading: false,
            fallback: false,
            state_children: Vec::new(),
            style: gpui_kit::div().size(px(30.)).style().clone(),
            interactivity: Default::default(),
        },
    );
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    for _ in 0..8 {
        frame(&mut native);
    }
    let (index, image) = native.update(|_, cx| {
        let layout = view.read(cx).layout(cx);
        let index = layout
            .panes
            .iter()
            .position(|pane| pane.module == MODULE)
            .expect("the pane opened");
        let seat = app.seats.read(cx).seat(layout.panes[index].instance);
        let tree = seat
            .expect("seated")
            .read(cx)
            .tree()
            .expect("the view drew");
        let image = tree
            .read(cx)
            .image_for_test(HASH)
            .expect("the picture decoded");
        (index, image)
    });
    assert!(
        native.update(|window, _| window.has_image_atlas_entry(&image)),
        "the picture painted"
    );
    pane(&view, PaneMessage::Close(index), &mut native);
    frame(&mut native);
    assert!(
        !native.update(|window, _| window.has_image_atlas_entry(&image)),
        "the closed pane's picture stayed in the atlas"
    );
}

/// `Seat::keys_free`, as `PaneLayer` pushes it: a
/// seat's keys are free only while its pane is in front, nothing is open
/// over the desk and no hold is on; the back pane's never are.
#[gpui_kit::test]
fn a_seats_keys_are_free_only_in_front_with_nothing_over_the_desk(cx: &mut TestAppContext) {
    const FRONT: &str = "pane-keys-front-view";
    const BACK: &str = "pane-keys-back-view";
    let (app, _, view, mut native) = console(cx);
    crate::runtime::seat_for_test(BACK, 400);
    crate::runtime::seat_for_test(FRONT, 400);
    pane(&view, PaneMessage::Select(BACK), &mut native);
    pane(&view, PaneMessage::Split(FRONT), &mut native);
    let keys_free = |module: &str, native: &mut VisualTestContext| {
        native.update(|_, cx| {
            let layout = view.read(cx).layout(cx);
            let pane = layout
                .panes
                .iter()
                .find(|pane| pane.module == module)
                .expect("the pane opened");
            let seat = app.seats.read(cx).seat(pane.instance).expect("seated");
            seat.read(cx).keys_free()
        })
    };
    assert!(
        keys_free(FRONT, &mut native),
        "the front pane's keys are free"
    );
    assert!(
        !keys_free(BACK, &mut native),
        "a back pane's keys are never free"
    );
    show(&view, Some(Overlay::Spotlight), &mut native);
    native.run_until_parked();
    assert!(!keys_free(FRONT, &mut native), "free under Spotlight");
    show(&view, None, &mut native);
    native.run_until_parked();
    assert!(
        keys_free(FRONT, &mut native),
        "not free again once Spotlight closed"
    );
    let front = native.update(|_, cx| view.read(cx).layout(cx).focused);
    pane(&view, PaneMessage::Hold(front), &mut native);
    assert!(
        !keys_free(FRONT, &mut native),
        "free while the keyboard holds the pane"
    );
    pane(&view, PaneMessage::Release { keep: true }, &mut native);
    assert!(
        keys_free(FRONT, &mut native),
        "not free again after the hold"
    );
}

/// A view's link (`link.open`) is routed by `Seats` to `Windows::open_link`
/// on the seat's next turn: the listed view opens on the console's desk,
/// comes to the front and is handed the route.
#[gpui_kit::test]
fn a_views_link_opens_its_seat_on_the_console(cx: &mut TestAppContext) {
    const MODULE: &str = "pane-link-intent-view";
    const LINKED: &str = "pane-link-intent-target";
    let (app, _, view, mut native) = console(cx);
    crate::runtime::seat_for_test(MODULE, 400);
    let roster = crate::runtime::Roster::listing(&[MODULE, LINKED]);
    app.rail
        .update(&mut native, |rail, cx| rail.read_off(roster, cx));
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    let instance = native.update(|_, cx| {
        let layout = view.read(cx).layout(cx);
        let pane = layout
            .panes
            .iter()
            .find(|pane| pane.module == MODULE)
            .expect("the pane opened");
        let seat = app.seats.read(cx).seat(pane.instance).expect("seated");
        seat.read(cx).instance()
    });
    crate::runtime::intent_for_test(
        MODULE,
        instance,
        crate::runtime::Intent::OpenLink(format!("duck://{LINKED}/room/7")),
    );
    // the seat's next turn, as the door settles them; a turn owed to a
    // frame lands after it
    for _ in 0..2 {
        app.seats.update(&mut native, |seats, cx| seats.settle(cx));
        native.run_until_parked();
        frame(&mut native);
    }
    let modules: Vec<_> = native.update(|_, cx| {
        view.read(cx)
            .layout(cx)
            .panes
            .iter()
            .map(|pane| pane.module)
            .collect()
    });
    assert!(
        modules.contains(&LINKED),
        "the link opened nothing: {modules:?}"
    );
    assert_eq!(active(&app, &native), Some(LINKED));
    assert_eq!(
        crate::runtime::take_route(LINKED).as_deref(),
        Some("room/7"),
        "the view was not handed its route"
    );
}

/// A frame as the platform delivers one, a figure's tick after the last:
/// the timers that came due run, whatever they dirtied draws, then the
/// next-frame callbacks.
fn tick_frame(native: &mut VisualTestContext) {
    native
        .executor()
        .advance_clock(std::time::Duration::from_secs(1) / super::figure::FPS as u32);
    native.run_until_parked();
    frame(native);
}

/// Motion switched off with the desk otherwise idle: the empty desk's
/// figure hears it from the pane's observer, not from a draw, so it stops
/// on the frame that shows it and asks for none after it. Switched on
/// again, it tumbles with nothing else drawing it in. Each of its frames
/// draws the pane around it (the pane is cached, and a figure's frame
/// dirties it), so the pane's count is the figure's; the window's is not,
/// since the bar's dot pulses while motion is on.
#[gpui_kit::test]
fn an_empty_pane_stops_its_figure_when_motion_goes_off_without_a_frame_loop(
    cx: &mut TestAppContext,
) {
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    native.update(|_, cx| cx.set_reduce_motion(false));
    set_motion(&app, true, &mut native);
    pane(&view, PaneMessage::Close(0), &mut native);
    assert_eq!(panes(&mut native, &view).0, 0, "the desk is bare");
    let drawn = window_count(key, "renders.empty");
    for _ in 0..10 {
        tick_frame(&mut native);
    }
    assert!(
        window_count(key, "renders.empty") >= drawn + 10,
        "with motion on the figure tumbles"
    );

    // the frame that shows it still is the one the switch draws
    set_motion(&app, false, &mut native);
    let drawn = window_count(key, "renders.empty");
    for _ in 0..super::figure::FPS {
        tick_frame(&mut native);
    }
    assert_eq!(
        window_count(key, "renders.empty"),
        drawn,
        "the still figure drew its pane on"
    );

    set_motion(&app, true, &mut native);
    for _ in 0..10 {
        tick_frame(&mut native);
    }
    assert!(
        window_count(key, "renders.empty") >= drawn + 10,
        "switched on again, the figure stayed still"
    );
}

/// The bar's breath and the bare desk's figure, both 30 frames a second
/// and started apart (the figure 10 ms after the breath, between two of
/// its frames), wake on one grid: over a second the window draws at 30
/// instants, not at each one's own 30.
#[gpui_kit::test]
fn the_breath_and_the_figure_share_frames(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    native.update(|_, cx| cx.set_reduce_motion(false));
    native.update(|window, _| window.activate_window());
    set_motion(&app, true, &mut native);
    polled(&app, status(7), &mut native);
    for _ in 0..4 {
        frame(&mut native);
    }
    native
        .executor()
        .advance_clock(std::time::Duration::from_millis(10));
    pane(&view, PaneMessage::Close(0), &mut native);
    assert_eq!(panes(&mut native, &view).0, 0, "the desk is bare");
    let (dot, empty) = (
        window_count(key, "renders.dot"),
        window_count(key, "renders.empty"),
    );
    // a millisecond a step: two wakes in one step drew at one instant
    let (mut drawn, mut instants) = (window_count(key, "renders"), 0);
    for _ in 0..1000 {
        native
            .executor()
            .advance_clock(std::time::Duration::from_millis(1));
        native.run_until_parked();
        let now = window_count(key, "renders");
        instants += u64::from(now > drawn);
        drawn = now;
    }
    assert!(
        window_count(key, "renders.dot") >= dot + 29
            && window_count(key, "renders.empty") >= empty + 29,
        "the breath and the figure both moved"
    );
    assert!(
        (29..=31).contains(&instants),
        "the window drew at {instants} instants in a second"
    );
}

/// The system asking for less motion, or the motion switch off: the bare
/// desk's breath and figure hold still, and the window draws nothing over
/// two seconds.
#[gpui_kit::test]
fn a_still_desk_draws_no_frame(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    native.update(|_, cx| cx.set_reduce_motion(false));
    native.update(|window, _| window.activate_window());
    set_motion(&app, true, &mut native);
    polled(&app, status(7), &mut native);
    pane(&view, PaneMessage::Close(0), &mut native);
    assert_eq!(panes(&mut native, &view).0, 0, "the desk is bare");
    let run = |native: &mut VisualTestContext, ms: u64| {
        for _ in 0..ms / 40 {
            native
                .executor()
                .advance_clock(std::time::Duration::from_millis(40));
            native.run_until_parked();
        }
    };
    run(&mut native, 400);
    assert!(window_count(key, "renders.empty") > 1, "the figure moved");
    for (reduce, motion) in [(true, true), (false, false)] {
        native.update(|_, cx| cx.set_reduce_motion(reduce));
        set_motion(&app, motion, &mut native);
        // the frame that shows them still, and a tick already waiting
        run(&mut native, 200);
        let drawn = window_count(key, "renders");
        run(&mut native, 2000);
        assert_eq!(
            window_count(key, "renders"),
            drawn,
            "less motion asked {reduce}, the switch {motion}: the window drew"
        );
    }
}

/// The bare desk reads the programs off the `Rail`, not the model: a
/// roster change draws it once, with no dispatch between.
#[gpui_kit::test]
fn a_roster_change_redraws_the_bare_desk(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    // a still figure: its frames are not the roster's
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Close(0), &mut native);
    assert_eq!(panes(&mut native, &view).0, 0, "the desk is bare");
    let drawn = window_count(key, "renders.empty");
    app.rail.update(&mut native, |rail, cx| {
        rail.read_off(crate::runtime::Roster::listing(&["bare-desk-view"]), cx)
    });
    native.run_until_parked();
    assert_eq!(
        window_count(key, "renders.empty"),
        drawn + 1,
        "the roster moved and the bare desk did not draw once"
    );
}

/// The keys going to an empty window's own box (a press on its
/// background, or the keys coming back to it with the window) go on to its
/// field, so typing starts at once: in front, and again when the window
/// comes back to the front; not while the keyboard holds the window.
#[gpui_kit::test]
fn the_command_field_takes_the_keys_when_its_pane_comes_to_the_front(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = console(cx);
    // gpui reports focus moves only in the active window
    native.update(|window, _| window.activate_window());
    key(&mut native, "secondary-n");
    settle(&mut native);
    let focused = |native: &mut VisualTestContext| native.update(|window, cx| window.focused(cx));
    let own = native.update(|_, cx| {
        let view = view.read(cx);
        let layout = view.layout(cx);
        assert!(
            layout.panes[layout.focused].is_empty(),
            "⌘N: an empty window"
        );
        view.panes.read(cx).pane_keys[&layout.panes[layout.focused].instance]
            .0
            .clone()
    });
    let field = focused(&mut native);
    assert!(
        field.is_some() && field.as_ref() != Some(&own),
        "⌘N: the field has the keys"
    );

    // a press on the window's background gives the keys to its box
    native.update(|window, cx| own.focus(window, cx));
    settle(&mut native);
    assert_eq!(focused(&mut native), field, "the box kept the keys");

    key(&mut native, "secondary-1");
    settle(&mut native);
    assert_ne!(focused(&mut native), field);
    key(&mut native, "secondary-2");
    settle(&mut native);
    assert_eq!(
        focused(&mut native),
        field,
        "back in front, the keys are not in the field"
    );

    // held by the keyboard (⌘⇧M), its box keeps them for the arrows
    key(&mut native, "secondary-shift-m");
    settle(&mut native);
    assert_eq!(
        focused(&mut native),
        Some(own),
        "the field took the keys from a hold"
    );
}

/// Help greets a new account and is plain help otherwise, and a Help
/// window already showing follows the model when that changes.
#[gpui_kit::test]
fn help_greets_a_new_account_and_titles_otherwise(cx: &mut TestAppContext) {
    let (app, _, _, mut native) = console(cx);
    let title = |native: &mut VisualTestContext| {
        let nodes = native.update(draw);
        let page = nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "console:help/page")
            .unwrap_or_else(|| panic!("no help page: {nodes}"));
        let heading = nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "console:heading" && node["role"] == "Heading")
            .unwrap_or_else(|| panic!("no heading: {nodes}"));
        assert_eq!(
            page["name"], heading["name"],
            "the page is named by its heading"
        );
        heading["name"].as_str().unwrap().to_owned()
    };
    open_help(&app, &mut native);
    assert_eq!(title(&mut native), "Ducktape help");
    // a new account lands on it greeted (`Account::welcome`)
    app.account.update(&mut native, |account, cx| {
        let mut state = account.get().clone();
        state.welcome = true;
        account.seed(state, None, None, cx);
    });
    assert_eq!(title(&mut native), "Welcome to Ducktape");
    // asked for again, it is just help
    open_help(&app, &mut native);
    assert_eq!(title(&mut native), "Ducktape help");
}

/// The window itself takes the keys once its last pane leaves, wherever
/// they were (the bar, here); while something open over the desk holds
/// them it keeps them, and they go to the window when it closes.
#[gpui_kit::test]
fn the_window_takes_the_keys_when_its_last_pane_leaves(cx: &mut TestAppContext) {
    let (app, _, view, mut native) = console(cx);
    let root_has_them = |native: &mut VisualTestContext| {
        native.update(|window, cx| view.read(cx).focus.is_focused(window))
    };
    settle(&mut native);
    native
        .update(|window, cx| super::pane_hold_tests::focus_control("rail-connection", window, cx));
    settle(&mut native);
    assert!(!root_has_them(&mut native));
    pane(&view, PaneMessage::Close(0), &mut native);
    settle(&mut native);
    assert_eq!(panes(&mut native, &view).0, 0);
    assert!(root_has_them(&mut native), "the keys stayed on the bar");

    // Settings open over the desk when its last pane leaves
    self::key(&mut native, "secondary-n");
    settle(&mut native);
    show(
        &view,
        Some(Overlay::Settings(SettingsPage::Appearance)),
        &mut native,
    );
    settle(&mut native);
    pane(&view, PaneMessage::Close(0), &mut native);
    settle(&mut native);
    native.update(|window, cx| {
        assert!(
            view.read(cx)
                .dialogs()
                .read(cx)
                .modal
                .contains_focused(window, cx),
            "the last pane leaving took the keys from Settings"
        )
    });
    show(&view, None, &mut native);
    settle(&mut native);
    assert!(
        root_has_them(&mut native),
        "Settings closed over a bare desk"
    );
    // and a later model notify leaves them where they went next
    native
        .update(|window, cx| super::pane_hold_tests::focus_control("rail-connection", window, cx));
    settle(&mut native);
    toast(&app, "saved", &mut native);
    settle(&mut native);
    assert!(
        !root_has_them(&mut native),
        "a later notify pulled the keys back to the root"
    );
}

/// A pane whose seat holds both a tree and a standin draws the standin: a
/// guest that traps after its tree mounted shows "This view stopped" and
/// its Retry, not the frozen tree.
#[gpui_kit::test]
fn a_trapped_view_shows_its_standin(cx: &mut TestAppContext) {
    const MODULE: &str = "pane-trapped-view";
    let (app, _, view, mut native) = console(cx);
    crate::runtime::seat_for_test(MODULE, 400);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    let (instance, before) = native.update(|window, cx| {
        let nodes = draw(window, cx);
        let layout = view.read(cx).layout(cx);
        let pane = layout
            .panes
            .iter()
            .find(|pane| pane.module == MODULE)
            .unwrap();
        let seat = app.seats.read(cx).seat(pane.instance).unwrap();
        (seat.read(cx).instance(), ids(&nodes))
    });
    let unavailable = format!("console:{MODULE}/view-unavailable");
    assert!(
        !before.contains(&unavailable),
        "a live view shows no standin: {before:?}"
    );
    crate::runtime::fault_for_test(MODULE, instance);
    native.run_until_parked();
    let after = native.update(|window, cx| ids(&draw(window, cx)));
    assert!(
        after.contains(&unavailable),
        "the trapped view shows its standin over the tree it keeps: {after:?}"
    );
    assert!(
        after.contains(&format!("console:{MODULE}/view-retry")),
        "{after:?}"
    );
}

/// A popped-out pane's seat is placed in the new window as soon as that
/// window opens, not at the next dispatch: a guest's Focus right after
/// the pop-out runs in the window the pane is in.
#[gpui_kit::test]
fn a_popped_out_seat_moves_into_its_window_when_it_opens(cx: &mut TestAppContext) {
    const MODULE: &str = "pane-popout-place-view";
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_for_test(MODULE, 400);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    let (index, instance) = native.update(|_, cx| {
        let layout = view.read(cx).layout(cx);
        let index = layout
            .panes
            .iter()
            .position(|pane| pane.module == MODULE)
            .unwrap();
        (index, layout.panes[index].instance)
    });
    let seat = native.update(|_, cx| app.seats.read(cx).seat(instance).unwrap());
    assert_eq!(
        seat.read_with(&native, |seat, _| seat.window()),
        Some(native.update(|_, cx| app.windows.read(cx).handles()[&key])),
        "the seat starts in the console"
    );
    // the pane leaves for a window of its own, which `Windows` opens
    native.update(|window, cx| press(&format!("pane/{index}/popout"), window, cx));
    native.run_until_parked();
    let (_, popped_handle, _) = popped(&app, key, &mut native);
    assert_eq!(
        seat.read_with(&native, |seat, _| seat.window()),
        Some(popped_handle),
        "the seat moved into the popped-out window as it opened"
    );
}

/// A pane picked in Spotlight hands the keys by its view's first frame, as
/// the draw that showed that frame did while the guest ticked on the draw
/// path: a view whose first frame has no control keeps them on the pane's
/// box even though its second frame, ticked before that draw (props, a
/// reply, a busy frame), has a field. The seat holds that second tick
/// until the first's frame has drawn (`Seat::turn`, one tick per draw).
#[gpui_kit::test]
fn a_pane_picked_in_spotlight_hands_the_keys_by_its_first_frame(cx: &mut TestAppContext) {
    use gpui_kit::Styled as _;
    const MODULE: &str = "pane-first-frame-view";
    let field = view_wire::Node::Input {
        options: view_wire::InputOptions {
            label: "Search".into(),
            ..Default::default()
        },
        id: view_wire::ElementIdWire::Name("search".into()),
        placeholder: String::new(),
        value: String::new(),
        on_input: Some(1),
        on_submit: None,
        secure: false,
        style: gpui_kit::div().w(px(200.)).h(px(24.)).style().clone(),
    };
    let (_, _, view, mut native) = console(cx);
    settle(&mut native);
    key(&mut native, "secondary-k");
    settle(&mut native);
    crate::runtime::seat_frames_for_test(MODULE, 200, view_wire::Node::empty(), field);
    run_spot(&view, Spot::Open(MODULE), &mut native);
    settle(&mut native);
    let nodes = native.update(draw);
    assert!(
        nodes
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == format!("console:{MODULE}/search")),
        "the second frame's field is up: {nodes}"
    );
    native.update(|window, cx| {
        let view = view.read(cx);
        let layout = view.layout(cx);
        let own = view.panes.read(cx).pane_keys[&layout.panes[layout.focused].instance]
            .0
            .clone();
        assert!(
            own.is_focused(window),
            "the box keeps the keys, its first frame had no control: {:?}",
            window.focused(cx)
        );
    });
}

/// A wake of one pane's seat (here a load landing: a fresh tree) draws that
/// pane's tree again and no other pane's. Each body is cached on its own
/// under the uncached layer and pane views (P6), so the sibling's tree
/// hits; the pane views themselves render with the window, so
/// `renders.pane.1` is not asserted flat.
#[gpui_kit::test]
fn a_pane_wake_re_renders_its_tree_and_not_the_siblings(cx: &mut TestAppContext) {
    const FIRST: &str = "pane-wake-first-view";
    const SECOND: &str = "pane-wake-second-view";
    let _on = crate::perf::on_for_test();
    let (_, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(FIRST, 400, line("first"));
    crate::runtime::seat_drawing_for_test(SECOND, 400, line("second"));
    pane(&view, PaneMessage::Select(FIRST), &mut native);
    pane(&view, PaneMessage::Split(SECOND), &mut native);
    for _ in 0..8 {
        frame(&mut native);
    }
    let (first, second) = (tree_renders(FIRST), tree_renders(SECOND));
    assert!(first > 0 && second > 0, "both trees drew");
    let panes = window_count(key, "renders.panes");
    // the first seat wakes: its view lands again, a fresh tree
    crate::runtime::seat_drawing_for_test(FIRST, 400, line("first again"));
    for _ in 0..4 {
        frame(&mut native);
    }
    assert_eq!(
        tree_renders(FIRST),
        first + 1,
        "the woken pane's tree drew once"
    );
    assert_eq!(
        tree_renders(SECOND),
        second,
        "the sibling's tree drew again"
    );
    assert!(
        window_count(key, "renders.panes") > panes,
        "the layer drew with the window"
    );
}

/// The desk's size reaches the model from the frame's callback, after the
/// frame that measured it, never from the draw itself; and a frame at a
/// size the model already has commits nothing.
#[gpui_kit::test]
fn the_desk_size_is_committed_after_the_frame_not_during_it(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = console(cx);
    let desk =
        |native: &mut VisualTestContext| native.update(|_, cx| view.read(cx).layout(cx).desk);
    // the fixture delivered the first frame's callback
    assert_eq!(desk(&mut native), Some((1280., 800. - layers::BAR)));
    native.simulate_resize(size(px(1000.), px(700.)));
    native.run_until_parked();
    // the window drew at its new size; the model still has the old one
    native.update(|window, cx| {
        window.render_frame(cx);
    });
    assert_eq!(
        desk(&mut native),
        Some((1280., 800. - layers::BAR)),
        "the draw itself moved the desk"
    );
    let ran = native.update(|window, cx| window.simulate_next_frame(cx));
    assert!(ran > 0, "the frame asked for no callback");
    assert_eq!(desk(&mut native), Some((1000., 700. - layers::BAR)));
    // the same size again: nothing to commit, no callback asked for
    native.update(|window, cx| {
        window.render_frame(cx);
    });
    assert_eq!(
        native.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "a frame at the same size asked for a callback"
    );
    assert_eq!(desk(&mut native), Some((1000., 700. - layers::BAR)));
}

/// What opens or closes over the desk reaches the panes by their window's
/// `Overlays` alone: the layer draws again, so the handoff after its draw
/// (`keys_move`) sees it, with nothing else drawing the window.
#[gpui_kit::test]
fn an_overlay_moving_draws_the_panes_by_its_slice_alone(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (app, key, _, mut native) = console(cx);
    native.run_until_parked();
    let overlays = app.windows.read_with(&native, |windows, _| {
        windows.own(key).expect("the console").overlays.clone()
    });
    for overlay in [Some(Overlay::Spotlight), None] {
        let before = window_count(key, "renders.panes");
        overlays.update(&mut native, |overlays, cx| match overlay {
            Some(overlay) => overlays.open(overlay, cx),
            None => overlays.close(Overlay::Spotlight, cx),
        });
        native.run_until_parked();
        assert!(
            window_count(key, "renders.panes") > before,
            "{overlay:?} over the desk and the panes did not draw"
        );
    }
}
