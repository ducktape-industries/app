use super::selection::{Window2, paragraph};
use super::*;

/// A message's words and, under them, the button that opens its thread
/// (chat's "Open thread →"): on no paragraph, consuming its click or not.
fn message(consumes: bool) -> wire::Node {
    let place = div().w(px(40.)).h(px(20.)).style().clone();
    let mut open = container_with_style("open-thread", place, []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut open {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.role = Some(gpui_kit::Role::Button);
        interactivity.aria.label = Some("Open thread".into());
        interactivity.on_click = Some(31);
        interactivity.consumes_click = consumes;
    }
    let column = div().w(px(150.)).flex().flex_col().style().clone();
    container_with_style("message", column, [paragraph("alpha", "alpha beta"), open])
}

/// A click, consumed by the view or not, ends the press it came from, so
/// the pointer moving over words after it with no button held selects
/// nothing. A consumed click stops the release's bubble, which gpui-base's
/// window selection ends the press on.
#[gpui_kit::test]
fn a_click_leaves_no_press_for_the_pointer_to_drag(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    for consumes in [true, false] {
        let window = cx.open_window(size(px(300.), px(100.)), |_, cx| Window2 {
            tree: cx.new(|_| ViewTree::new(message(consumes))),
        });
        let tree = window
            .root(cx)
            .unwrap()
            .read_with(cx, |w, _| w.tree.clone());
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let (events, _subscription) = emitted(&tree, &mut native);
        native.update(|window, cx| window.render_frame(cx));
        let button = point(px(20.), px(30.));
        native.simulate_mouse_move(button, None, Default::default());
        native.simulate_mouse_down(button, MouseButton::Left, Default::default());
        native.simulate_mouse_up(button, MouseButton::Left, Default::default());
        let clicks = events
            .borrow()
            .iter()
            .filter(|event| matches!(event, wire::Event::Click { .. }))
            .count();
        assert_eq!(clicks, 1, "the button heard its click");
        for at in [point(px(100.), px(10.)), point(px(2.), px(10.))] {
            native.simulate_mouse_move(at, None, Default::default());
            native.update(|window, cx| window.render_frame(cx));
        }
        let selected = native.update(gpui_kit::base::TextSelection::selected_text);
        assert_eq!(selected, "", "consumed: {consumes}");
    }
}
