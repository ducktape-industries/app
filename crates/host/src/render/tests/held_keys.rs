//! A node that heard a key go down hears it come up, once, whatever takes
//! the keyboard from it first: the window going to the back, or focus
//! moving to another element or another pane.

use super::*;
use gpui_kit::{KeyUpEvent, PlatformInput, VisualTestContext};

const DOWN: u32 = 1;
const UP: u32 = 2;

/// A board that hears keys go down on `DOWN` and come up on `up`.
fn board(up: u32) -> wire::Node {
    let mut node = container_with_style("board", div().w(px(60.)).h(px(40.)).style().clone(), []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut node {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.on_key_down = Some(DOWN);
        interactivity.on_key_up = Some(up);
    }
    node
}

/// The board beside a button that hears no keys.
fn screen(up: u32) -> wire::Node {
    container_with_style(
        "screen",
        div().size_full().flex().style().clone(),
        [board(up), button("other", "Other")],
    )
}

/// The keys `handler` heard come up, in order.
fn came_up(events: &RefCell<Vec<wire::Event>>, handler: u32) -> Vec<String> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::KeyUp {
                handler: heard,
                event,
                ..
            } if *heard == handler => Some(event.clone().into_gpui().keystroke.key),
            _ => None,
        })
        .collect()
}

/// The view moves the keys to the node at `target`, and the window draws.
fn focus(tree: &Entity<ViewTree>, target: &[&str], native: &mut VisualTestContext) {
    let target = target.iter().map(|key| named_id(key)).collect();
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(wire::WidgetCommand::Focus { target }, window, cx)
        })
        .unwrap();
        window.render_frame(cx);
    });
}

fn activate(native: &mut VisualTestContext) {
    native.update(|window, _| window.activate_window());
    native.run_until_parked();
    native.update(|window, cx| {
        assert!(window.is_window_active());
        window.render_frame(cx);
    });
}

fn press(native: &mut VisualTestContext, key: &str) {
    native.update(|window, cx| {
        window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
    });
}

fn release(native: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).unwrap();
    native.update(|window, cx| {
        window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
    });
}

/// Alt-tab with an arrow held: the board hears it come up as the window
/// goes to the back. A key let go in the board comes up once, as before.
#[gpui_kit::test]
fn a_held_key_comes_up_when_the_window_goes_to_the_back(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(board(UP)));
    let tree = window.root(cx).unwrap();
    let mut native = VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    activate(&mut native);
    focus(&tree, &["board"], &mut native);
    press(&mut native, "a");
    release(&mut native, "a");
    press(&mut native, "left");
    assert_eq!(came_up(&events, UP), ["a"], "a key let go comes up once");
    native.deactivate_window();
    native.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        came_up(&events, UP),
        ["a", "left"],
        "the held key comes up as the window goes to the back"
    );
    activate(&mut native);
    native.deactivate_window();
    native.update(|window, cx| window.render_frame(cx));
    assert_eq!(came_up(&events, UP), ["a", "left"], "and only once");
}

/// The tree's watch on the window going to the back is heard before this
/// one, which reports what the tree still holds then.
struct Watch {
    _to_back: Subscription,
}

/// A window that goes to the back can draw no frame before it is in front
/// again (minimized, or covered: the platform stops drawing it), and
/// gpui's focus event comes from a draw that sees the window's state
/// change, so it would never come. The held key comes up as the platform
/// says the window went to the back. gpui's test platform draws after
/// every change of the window's state, so the test reads the tree at that
/// moment instead.
#[gpui_kit::test]
fn a_held_key_comes_up_as_the_window_goes_to_the_back_not_at_a_frame(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(board(UP)));
    let tree = window.root(cx).unwrap();
    let mut native = VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    activate(&mut native);
    focus(&tree, &["board"], &mut native);
    press(&mut native, "left");
    let held = Rc::new(std::cell::Cell::new(None));
    let _watch = native.update(|window, cx| {
        let (tree, held) = (tree.clone(), held.clone());
        cx.new(|cx| {
            let to_back = cx.observe_window_activation(window, move |_, window, cx| {
                if !window.is_window_active() {
                    held.set(Some(tree.read(cx).held_keys.len()));
                }
            });
            Watch { _to_back: to_back }
        })
    });
    native.deactivate_window();
    assert_eq!(
        held.get(),
        Some(0),
        "nothing is held as the window goes to the back"
    );
    assert_eq!(came_up(&events, UP), ["left"], "the board heard it once");
}

/// An arrow held while the view moves the keys to its button: the board
/// hears it come up as focus leaves it, and the real KeyUp, which goes to
/// the button, tells it nothing more.
#[gpui_kit::test]
fn a_held_key_comes_up_when_focus_moves_to_another_element(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(200.), px(80.)), |_, _| ViewTree::new(screen(UP)));
    let tree = window.root(cx).unwrap();
    let mut native = VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    activate(&mut native);
    focus(&tree, &["screen", "board"], &mut native);
    press(&mut native, "left");
    focus(&tree, &["screen", "other"], &mut native);
    assert_eq!(came_up(&events, UP), ["left"], "focus left the board");
    release(&mut native, "left");
    assert_eq!(came_up(&events, UP), ["left"], "and only once");
}

/// The held key comes up on the handler the board has when focus leaves
/// it: a listener the view dropped and set again has a new route, and
/// the one the key went down under names nothing by then.
#[gpui_kit::test]
fn a_held_key_comes_up_on_the_handler_the_node_has_now(cx: &mut gpui_kit::TestAppContext) {
    const RENUMBERED: u32 = 7;
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(200.), px(80.)), |_, _| ViewTree::new(screen(UP)));
    let tree = window.root(cx).unwrap();
    let mut native = VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    activate(&mut native);
    focus(&tree, &["screen", "board"], &mut native);
    press(&mut native, "left");
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| tree.replace(screen(RENUMBERED), &[], cx));
        window.render_frame(cx);
    });
    focus(&tree, &["screen", "other"], &mut native);
    assert_eq!(came_up(&events, UP), Vec::<String>::new());
    assert_eq!(came_up(&events, RENUMBERED), ["left"]);
}

struct Panes {
    left: Entity<ViewTree>,
    right: Entity<ViewTree>,
}

impl Render for Panes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(self.left.clone())
            .child(self.right.clone())
    }
}

/// An arrow held in one pane while the person clicks into the next: the
/// board hears it come up as the other pane takes the keys.
#[gpui_kit::test]
fn a_held_key_comes_up_when_focus_moves_to_another_pane(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(240.), px(80.)), |_, cx| Panes {
        left: cx.new(|_| ViewTree::new(board(UP))),
        right: cx.new(|_| ViewTree::new(button("other", "Other"))),
    });
    let (left, right) = window
        .root(cx)
        .unwrap()
        .read_with(cx, |panes, _| (panes.left.clone(), panes.right.clone()));
    let mut native = VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&left, &mut native);
    activate(&mut native);
    focus(&left, &["board"], &mut native);
    press(&mut native, "left");
    focus(&right, &["other"], &mut native);
    assert_eq!(came_up(&events, UP), ["left"], "focus left the pane");
    release(&mut native, "left");
    assert_eq!(came_up(&events, UP), ["left"], "and only once");
}
