//! The pane keyboard operations: ⌘⇧↩ fills, ⌘⇧M holds a window for the
//! arrows, Search offers both, and Shift+Return on a bar tab shows its
//! program in the window in front.
use super::panes_tests::{console, draw, in_front, key, panes, settle};
use super::*;
use gpui_kit::{Keystroke, TestAppContext, VisualTestContext};

type Setup = (
    Entity<Desktop>,
    WindowKey,
    Entity<DesktopWindow>,
    VisualTestContext,
);

/// A console with two windows on a drawn desk, the second in front.
fn desk_of_two(cx: &mut TestAppContext) -> Setup {
    let (model, key, view, mut native) = console(cx);
    key_split(&mut native);
    settle(&mut native);
    (model, key, view, native)
}

/// The keyboard focus on the element with `id`, as the AX door gives it.
pub(super) fn focus_control(id: &str, window: &mut Window, cx: &mut gpui_kit::App) {
    use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
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
                    path.iter().any(|element| {
                        matches!(element, gpui_kit::ElementId::Name(name) if name.as_ref() == id)
                    })
                })
                .then_some(*node)
        })
        .unwrap_or_else(|| panic!("missing AX control {id}"));
    window.dispatch_a11y_action(
        ActionRequest {
            action: Action::Focus,
            target_tree: TreeId::ROOT,
            target_node: node,
            data: None,
        },
        cx,
    );
}

fn key_split(native: &mut VisualTestContext) {
    key(native, "secondary-n");
}

fn frame(
    native: &mut VisualTestContext,
    view: &Entity<DesktopWindow>,
    index: usize,
) -> layout::Frame {
    native.update(|_, cx| view.read(cx).layout(cx).panes[index].frame.unwrap())
}

fn held(native: &mut VisualTestContext, view: &Entity<DesktopWindow>) -> bool {
    native.update(|_, cx| view.read(cx).layout(cx).held.is_some())
}

fn focused(native: &mut VisualTestContext) -> Option<gpui_kit::FocusHandle> {
    native.update(|window, cx| window.focused(cx))
}

fn stroke(native: &mut VisualTestContext, stroke: &str) {
    key(native, stroke);
    settle(native);
}

/// The control with `id` has the keys, as the AX door reports it.
fn has_keys(native: &mut VisualTestContext, id: &str) -> bool {
    native.update(draw).as_array().unwrap().iter().any(|node| {
        node["id"] == id
            && node["state"]
                .as_array()
                .unwrap()
                .contains(&"focused".into())
    })
}

#[gpui_kit::test]
fn the_fill_chord_fills_the_window_in_front_and_again_puts_it_back(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = desk_of_two(cx);
    let before = frame(&mut native, &view, 1);
    stroke(&mut native, "secondary-shift-enter");
    let filled = frame(&mut native, &view, 1);
    assert_ne!(filled, before);
    assert_eq!(
        native.update(|_, cx| view.read(cx).layout(cx).panes[1].restore),
        Some(before)
    );
    assert!(in_front(&mut native, &view), "the keys stay in the window");
    stroke(&mut native, "secondary-shift-enter");
    assert_eq!(frame(&mut native, &view, 1), before);
    assert_eq!(panes(&mut native, &view), (2, 1));
}

#[gpui_kit::test]
fn the_hold_chord_gives_the_arrows_to_the_window(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = desk_of_two(cx);
    let start = frame(&mut native, &view, 1);
    let previous = focused(&mut native);
    stroke(&mut native, "secondary-shift-m");
    assert!(held(&mut native, &view));
    assert!(in_front(&mut native, &view));
    assert_ne!(focused(&mut native), previous, "the box has the keys");

    // 8 px an arrow, 32 with Shift
    stroke(&mut native, "right");
    stroke(&mut native, "down");
    let moved = frame(&mut native, &view, 1);
    assert_eq!((moved.x, moved.y), (start.x + 8., start.y + 8.));
    assert_eq!(
        (moved.w, moved.h),
        (start.w, start.h),
        "moving does not size"
    );
    stroke(&mut native, "shift-left");
    stroke(&mut native, "shift-up");
    let moved = frame(&mut native, &view, 1);
    assert_eq!((moved.x, moved.y), (start.x - 24., start.y - 24.));

    // Alt sizes from the right and bottom edges, the same steps
    stroke(&mut native, "alt-right");
    stroke(&mut native, "alt-shift-down");
    let sized = frame(&mut native, &view, 1);
    assert_eq!(
        (sized.x, sized.y),
        (moved.x, moved.y),
        "sizing does not move"
    );
    assert_eq!((sized.w, sized.h), (start.w + 8., start.h + 32.));
    stroke(&mut native, "alt-left");
    stroke(&mut native, "alt-up");
    let sized = frame(&mut native, &view, 1);
    assert_eq!((sized.w, sized.h), (start.w, start.h + 24.));
    assert!(held(&mut native, &view), "arrows do not end the hold");

    // Return keeps it there, and the keys go back to what had them
    stroke(&mut native, "enter");
    assert!(!held(&mut native, &view));
    assert_eq!(frame(&mut native, &view, 1), sized);
    assert_eq!(focused(&mut native), previous, "the keys came back");
}

#[gpui_kit::test]
fn a_hold_stops_at_the_desk_and_at_the_smallest_window(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = desk_of_two(cx);
    let desk = native.update(|_, cx| view.read(cx).layout(cx).desk());
    stroke(&mut native, "secondary-shift-m");
    // far past the desk's right edge: the window keeps a grip of itself on it
    for _ in 0..60 {
        key(&mut native, "shift-right");
    }
    settle(&mut native);
    let moved = frame(&mut native, &view, 1);
    assert_eq!(moved.x, desk.0 - layout::KEEP, "{moved:?} on {desk:?}");
    // sized past the desk's width and down to the floors
    for _ in 0..60 {
        key(&mut native, "alt-shift-right");
    }
    settle(&mut native);
    assert!(frame(&mut native, &view, 1).w <= desk.0);
    for _ in 0..60 {
        key(&mut native, "alt-shift-left");
        key(&mut native, "alt-shift-up");
    }
    settle(&mut native);
    let small = frame(&mut native, &view, 1);
    assert_eq!((small.w, small.h), (layout::MIN_WIDTH, layout::MIN_HEIGHT));
}

#[gpui_kit::test]
fn escape_puts_the_window_back_and_a_fill_with_it(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = desk_of_two(cx);
    let previous = focused(&mut native);
    // moved and sized, then put back
    let start = frame(&mut native, &view, 1);
    stroke(&mut native, "secondary-shift-m");
    stroke(&mut native, "shift-right");
    stroke(&mut native, "alt-shift-down");
    stroke(&mut native, "escape");
    assert!(!held(&mut native, &view));
    assert_eq!(frame(&mut native, &view, 1), start);
    assert_eq!(focused(&mut native), previous);

    // a filled window moved, then put back filled
    stroke(&mut native, "secondary-shift-enter");
    let (filled, restore) = native.update(|_, cx| {
        let pane = view.read(cx).layout(cx).panes[1].clone();
        (pane.frame, pane.restore)
    });
    stroke(&mut native, "secondary-shift-m");
    stroke(&mut native, "shift-left");
    assert_ne!(frame(&mut native, &view, 1), filled.unwrap());
    stroke(&mut native, "escape");
    native.update(|_, cx| {
        let pane = view.read(cx).layout(cx).panes[1].clone();
        assert_eq!((pane.frame, pane.restore), (filled, restore));
    });
}

#[gpui_kit::test]
fn space_keeps_and_any_other_key_ends_the_hold_keeping_the_window(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = desk_of_two(cx);
    let previous = focused(&mut native);
    for ending in ["space", "a", "tab", "secondary-shift-m"] {
        stroke(&mut native, "secondary-shift-m");
        assert!(held(&mut native, &view), "held before {ending}");
        stroke(&mut native, "shift-right");
        let kept = frame(&mut native, &view, 1);
        stroke(&mut native, ending);
        assert!(!held(&mut native, &view), "{ending} left the hold on");
        assert_eq!(frame(&mut native, &view, 1), kept, "{ending} kept it");
        // Tab's own move stands (`keys_moved_elsewhere_end_the_hold_and_stay_there`)
        if ending != "tab" {
            assert_eq!(
                focused(&mut native),
                previous,
                "{ending}: the keys came back"
            );
        }
    }
}

/// Keys a press, Tab or assistive technology moves during a hold end it
/// and stay where they went; they are not taken back to what had them.
#[gpui_kit::test]
fn keys_moved_elsewhere_end_the_hold_and_stay_there(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = desk_of_two(cx);
    // assistive technology moves them from the window to the bar
    stroke(&mut native, "secondary-shift-m");
    native.update(|window, cx| focus_control("rail-connection", window, cx));
    settle(&mut native);
    assert!(!held(&mut native, &view));
    assert!(has_keys(&mut native, "console:rail-connection"));
    // held from the bar, Tab moves them on from the window's box, into it
    stroke(&mut native, "secondary-shift-m");
    assert!(held(&mut native, &view));
    stroke(&mut native, "tab");
    assert!(!held(&mut native, &view));
    assert!(in_front(&mut native, &view), "Tab left them in the window");
}

/// Something opening over the desk ends the hold; when it closes, the keys
/// go back to what had them before the hold, not to the window's box.
#[gpui_kit::test]
fn after_an_overlay_the_keys_go_back_to_what_had_them_before_the_hold(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = desk_of_two(cx);
    native.update(|window, cx| focus_control("rail-connection", window, cx));
    settle(&mut native);
    let previous = focused(&mut native);
    stroke(&mut native, "secondary-shift-m");
    assert!(held(&mut native, &view));
    stroke(&mut native, "secondary-k");
    assert!(!held(&mut native, &view));
    stroke(&mut native, "escape");
    assert_eq!(focused(&mut native), previous);
    assert!(has_keys(&mut native, "console:rail-connection"));
}

/// A window let go of as another comes forward still remembers what had
/// the keys in it before the hold, not its own box.
#[gpui_kit::test]
fn a_held_window_sent_back_remembers_what_had_its_keys(cx: &mut TestAppContext) {
    let (_, _, view, mut native) = desk_of_two(cx);
    let previous = focused(&mut native);
    let remembered = |native: &mut VisualTestContext| {
        native.update(|_, cx| {
            let view = view.read(cx);
            let instance = view.layout(cx).panes[1].instance;
            view.panes.read(cx).pane_keys[&instance].1.clone()
        })
    };
    stroke(&mut native, "secondary-shift-m");
    stroke(&mut native, "secondary-1");
    assert!(!held(&mut native, &view));
    assert_eq!(remembered(&mut native), previous);
    stroke(&mut native, "secondary-2");
    assert_eq!(focused(&mut native), previous);
}

#[gpui_kit::test]
fn a_press_something_opening_or_another_window_ends_the_hold(cx: &mut TestAppContext) {
    let (model, _, view, mut native) = desk_of_two(cx);
    let previous = focused(&mut native);
    // a press on the held window's title bar, which takes no keys itself
    let title = frame(&mut native, &view, 1);
    stroke(&mut native, "secondary-shift-m");
    assert!(held(&mut native, &view));
    native.simulate_click(
        gpui_kit::point(
            gpui_kit::px(title.x + 100.),
            gpui_kit::px(title.y + desk::BAR + 12.),
        ),
        gpui_kit::Modifiers::none(),
    );
    settle(&mut native);
    assert!(!held(&mut native, &view), "a press");
    assert_eq!(focused(&mut native), previous, "the keys came back");
    // Search opening
    stroke(&mut native, "secondary-shift-m");
    assert!(held(&mut native, &view));
    stroke(&mut native, "secondary-k");
    assert!(!held(&mut native, &view), "Search opened");
    stroke(&mut native, "escape");
    // another window in front
    stroke(&mut native, "secondary-shift-m");
    assert!(held(&mut native, &view));
    stroke(&mut native, "secondary-1");
    assert!(!held(&mut native, &view), "another window came forward");
    assert!(in_front(&mut native, &view), "and it has the keys");
    // the OS window losing the keys
    stroke(&mut native, "secondary-shift-m");
    assert!(held(&mut native, &view));
    let key = view.read_with(&native, |view, _| view.key);
    model.update(&mut native, |model, cx| {
        model.dispatch(Message::WindowUnfocused(key), cx)
    });
    settle(&mut native);
    assert!(!held(&mut native, &view), "the window lost the keys");
}

/// Nor does the Window menu, whose items reach the actions past the key
/// context (macOS).
#[gpui_kit::test]
fn the_chords_and_the_menu_do_nothing_under_an_overlay(cx: &mut TestAppContext) {
    let (model, _, view, mut native) = desk_of_two(cx);
    let before = frame(&mut native, &view, 1);
    model.update(&mut native, |model, cx| {
        model.state.overlay = Some(crate::Overlay::Menu(crate::Popover::Node));
        model.bridge(false, cx);
    });
    stroke(&mut native, "secondary-shift-m");
    stroke(&mut native, "secondary-shift-enter");
    native.update(|window, cx| {
        window.dispatch_action(Box::new(keys::HoldPane), cx);
        window.dispatch_action(Box::new(keys::FillPane), cx);
    });
    settle(&mut native);
    assert!(!held(&mut native, &view));
    assert_eq!(frame(&mut native, &view, 1), before);
}

#[gpui_kit::test]
fn the_hold_is_told_in_words_once_while_it_lasts(cx: &mut TestAppContext) {
    let (_, _, _, mut native) = desk_of_two(cx);
    let count = |native: &mut VisualTestContext| {
        native
            .update(draw)
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| {
                node["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("Moving "))
            })
            .count()
    };
    assert_eq!(count(&mut native), 0);
    stroke(&mut native, "secondary-shift-m");
    assert_eq!(count(&mut native), 1, "one announcement while held");
    stroke(&mut native, "escape");
    assert_eq!(count(&mut native), 0);
}

#[test]
fn the_announcement_is_worded_for_the_platform() {
    let words = pane_hold::hold_words(&[], "");
    assert!(words.starts_with("Moving Empty window. Arrows move, Shift further, "));
    let (size, keep) = match cfg!(target_os = "macos") {
        true => ("Option", "Return"),
        false => ("Alt", "Enter"),
    };
    assert!(words.ends_with(&format!("{size} sizes, {keep} keeps, Escape puts back")));
}

#[test]
fn chords_with_shift_are_written_for_the_platform() {
    let (fill, hold) = (chord_label("⇧↩"), chord_label("⇧M"));
    match cfg!(target_os = "macos") {
        true => assert_eq!((fill.as_str(), hold.as_str()), ("⌘⇧↩", "⌘⇧M")),
        false => assert_eq!(
            (fill.as_str(), hold.as_str()),
            ("Ctrl Shift Enter", "Ctrl Shift M")
        ),
    }
    assert_eq!(chord_label("K").replace("Ctrl ", "⌘"), "⌘K");
}

/// Shift+Return on a bar tab shows its program in the window in front,
/// as Shift-click does; a plain Return opens it, as a click does. The
/// keys reach the tab as the rail's one Tab stop, on the front window's
/// program, and the arrow moves them to the next.
#[gpui_kit::test]
fn shift_return_on_a_bar_tab_shows_it_in_this_window(cx: &mut TestAppContext) {
    let (model, _, view, mut native) = console(cx);
    model.update(&mut native, |model, cx| {
        let roster = crate::runtime::Roster::listing(&["hold-tab-a", "hold-tab-b"]);
        model.state.roster = roster.clone();
        model
            .entities
            .rail
            .update(cx, |rail, cx| rail.read_off(roster, cx));
    });
    native.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.pane_message(PaneMessage::Open("hold-tab-a"), window, cx);
            view.pane_message(PaneMessage::Split("hold-tab-a"), window, cx);
        })
    });
    settle(&mut native);
    let front = |native: &mut VisualTestContext| {
        native.update(|_, cx| {
            let layout = view.read(cx).layout(cx);
            (layout.panes.len(), layout.panes[layout.focused].module)
        })
    };
    assert_eq!(front(&mut native), (3, "hold-tab-a"));
    native.update(|window, cx| {
        let nodes = draw(window, cx);
        let node = nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "console:rail/hold-tab-b")
            .unwrap_or_else(|| panic!("no tab in {nodes}"))
            .clone();
        assert!(
            node["description"].as_str().unwrap().starts_with("Shift+"),
            "{node}"
        );
    });
    native.update(|window, cx| {
        focus_control("rail/hold-tab-a", window, cx);
        window.dispatch_keystroke(Keystroke::parse("right").unwrap(), cx);
    });
    settle(&mut native);
    native.update(|window, cx| {
        window.dispatch_keystroke(Keystroke::parse("shift-enter").unwrap(), cx)
    });
    settle(&mut native);
    assert_eq!(
        front(&mut native),
        (3, "hold-tab-b"),
        "shown in the window in front, no window added"
    );
}

/// Search offers Fill and Move or size while a window has a frame, and
/// running them is the chords' messages.
#[gpui_kit::test]
fn search_rows_fill_and_hold_the_window_in_front(cx: &mut TestAppContext) {
    let (model, _, view, mut native) = desk_of_two(cx);
    let rows = |native: &mut VisualTestContext| {
        native.update(|_, cx| {
            model
                .read(cx)
                .state
                .spotlight_rows()
                .into_iter()
                .map(|row| row.title)
                .collect::<Vec<_>>()
        })
    };
    let titles = rows(&mut native);
    assert!(titles.contains(&"Fill window".to_owned()), "{titles:?}");
    assert!(
        titles.contains(&"Move or size window".to_owned()),
        "{titles:?}"
    );
    let before = frame(&mut native, &view, 1);
    let run = |native: &mut VisualTestContext, spot: crate::ui::Spot| {
        model.update(native, |model, cx| {
            model.dispatch(Message::OpenSpotlight, cx);
            model.dispatch(Message::Spot(spot), cx)
        });
        settle(native);
    };
    run(&mut native, crate::ui::Spot::FillWindow);
    assert_ne!(frame(&mut native, &view, 1), before, "filled");
    run(&mut native, crate::ui::Spot::HoldWindow);
    assert!(held(&mut native, &view));
    assert!(in_front(&mut native, &view), "the window has the arrows");
    let filled = frame(&mut native, &view, 1);
    stroke(&mut native, "right");
    assert_eq!(frame(&mut native, &view, 1).x, filled.x + 8.);
    stroke(&mut native, "escape");
    assert!(!held(&mut native, &view));
}
