//! A selection drag through the window as a desk draws it: the tree cached
//! under a root that redraws for other views, and two scrolling panes.
use super::selection::paragraph;
use super::*;

struct Other;

impl Render for Other {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

struct Desk {
    tree: Entity<ViewTree>,
    other: Entity<Other>,
    raises: bool,
}

impl Render for Desk {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (other, raises) = (self.other.clone(), self.raises);
        let raise = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let other = other.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                    if raises && phase == gpui_kit::DispatchPhase::Capture {
                        other.update(cx, |_, cx| cx.notify());
                    }
                });
            },
        )
        .absolute()
        .size_full();
        div()
            .size_full()
            .child(gpui_kit::base::TextSelectionLayer)
            .child(crate::render::pointer::Layer)
            .child(raise)
            .child(self.other.clone())
            .child(
                self.tree
                    .clone()
                    .cached(gpui_kit::StyleRefinement::default().size_full()),
            )
    }
}

fn panes() -> wire::Node {
    let pane = |key: &str, children: Vec<wire::Node>| {
        let mut style = div().w(px(150.)).h_full().flex().flex_col().style().clone();
        style.overflow.y = Some(gpui_kit::Overflow::Scroll);
        container_with_style(key, style, children)
    };
    let gap = |key: &str| container_with_style(key, div().h(px(10.)).style().clone(), []);
    container_with_style(
        "panes",
        div().flex().flex_row().size_full().style().clone(),
        [
            pane(
                "room",
                vec![
                    paragraph("alpha", "alpha"),
                    gap("gap-1"),
                    gap("gap-2"),
                    gap("gap-3"),
                    gap("gap-4"),
                    paragraph("gamma", "gamma"),
                ],
            ),
            pane(
                "thread",
                vec![
                    gap("gap-5"),
                    gap("gap-6"),
                    gap("gap-7"),
                    paragraph("beta", "beta"),
                ],
            ),
        ],
    )
}

fn desk(
    root: wire::Node,
    raises: bool,
    cx: &mut gpui_kit::TestAppContext,
) -> (gpui_kit::VisualTestContext, Entity<Other>) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(100.)), |_, cx| Desk {
        tree: cx.new(|_| ViewTree::new(root)),
        other: cx.new(|_| Other),
        raises,
    });
    let other = window
        .root(cx)
        .unwrap()
        .read_with(cx, |desk, _| desk.other.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::refresh_and_draw);
    (native, other)
}

fn frame(native: &mut gpui_kit::VisualTestContext) {
    native.update(crate::render::draw_frame);
}

fn press(native: &mut gpui_kit::VisualTestContext, at: Point<Pixels>) {
    native.simulate_mouse_move(at, None, Default::default());
    native.simulate_mouse_down(at, MouseButton::Left, Default::default());
}

fn drag_to(native: &mut gpui_kit::VisualTestContext, at: Point<Pixels>) {
    native.simulate_mouse_move(at, Some(MouseButton::Left), Default::default());
}

fn release(native: &mut gpui_kit::VisualTestContext, at: Point<Pixels>) -> String {
    native.simulate_mouse_up(at, MouseButton::Left, Default::default());
    frame(native);
    native.update(crate::render::pointer::selected_text)
}

fn in_the_room_only(copied: &str) {
    assert!(
        copied.contains("alpha") && copied.contains("gamma"),
        "{copied:?}"
    );
    assert!(!copied.contains("beta"), "the thread's line: {copied:?}");
}

#[gpui_kit::test]
fn a_press_that_raises_its_pane_starts_a_drag(cx: &mut gpui_kit::TestAppContext) {
    let (mut native, _) = desk(panes(), true, cx);
    let to = point(px(140.), px(70.));
    press(&mut native, point(px(1.), px(10.)));
    frame(&mut native);
    drag_to(&mut native, to);
    frame(&mut native);
    in_the_room_only(&release(&mut native, to));
}

#[gpui_kit::test]
fn a_frame_another_view_draws_keeps_the_drag(cx: &mut gpui_kit::TestAppContext) {
    let (mut native, other) = desk(panes(), false, cx);
    let to = point(px(140.), px(70.));
    press(&mut native, point(px(1.), px(10.)));
    drag_to(&mut native, point(px(140.), px(30.)));
    frame(&mut native);
    frame(&mut native);
    native.update(|window, cx| {
        other.update(cx, |_, cx| cx.notify());
        window.draw(cx).clear(cx);
    });
    drag_to(&mut native, to);
    in_the_room_only(&release(&mut native, to));
}

#[gpui_kit::test]
fn a_drag_let_go_over_the_next_pane_before_a_frame_stays_in_its_own(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (mut native, _) = desk(panes(), false, cx);
    let over = point(px(200.), px(40.));
    press(&mut native, point(px(1.), px(10.)));
    drag_to(&mut native, over);
    let copied = release(&mut native, over);
    assert!(copied.contains("alpha"), "{copied:?}");
    assert!(!copied.contains("beta"), "the thread's line: {copied:?}");
}

#[gpui_kit::test]
fn a_press_between_lines_selects_in_its_own_pane(cx: &mut gpui_kit::TestAppContext) {
    let (mut native, _) = desk(panes(), false, cx);
    let to = point(px(140.), px(70.));
    press(&mut native, point(px(1.), px(40.)));
    frame(&mut native);
    drag_to(&mut native, to);
    frame(&mut native);
    let copied = release(&mut native, to);
    assert!(copied.contains("gamma"), "{copied:?}");
    assert!(!copied.contains("beta"), "the thread's line: {copied:?}");
}

fn column(key: &str, children: Vec<wire::Node>) -> wire::Node {
    let mut style = div().w(px(150.)).h_full().flex().flex_col().style().clone();
    style.overflow.y = Some(gpui_kit::Overflow::Scroll);
    container_with_style(key, style, children)
}

#[gpui_kit::test]
fn a_drag_past_the_bottom_scrolls_its_pane_and_takes_the_lines_it_brings(
    cx: &mut gpui_kit::TestAppContext,
) {
    let lines = (0..10)
        .map(|line| paragraph(&format!("line-{line}"), &format!("line{line}")))
        .collect();
    let (mut native, _) = desk(column("room", lines), false, cx);
    press(&mut native, point(px(1.), px(10.)));
    for below in [150., 151., 152.] {
        drag_to(&mut native, point(px(140.), px(below)));
        frame(&mut native);
    }
    let copied = release(&mut native, point(px(140.), px(152.)));
    assert!(copied.starts_with("line0"), "{copied:?}");
    assert!(
        copied.contains("line6"),
        "the lines below the fold: {copied:?}"
    );
}

#[gpui_kit::test]
fn a_drag_in_a_dialog_selects_its_words_not_the_page_under_it(cx: &mut gpui_kit::TestAppContext) {
    let card = container_with_style(
        "card",
        div().w(px(150.)).flex().flex_col().style().clone(),
        [paragraph("delta", "delta"), paragraph("epsilon", "epsilon")],
    );
    let dialog = wire::Node::Overlay {
        id: named_id("dialog"),
        label: None,
        on_dismiss: None,
        children: vec![panes(), card],
        style: crate::render::test_style(div().size_full().style().clone()),
    };
    let (mut native, _) = desk(dialog, false, cx);
    let to = point(px(100.), px(30.));
    press(&mut native, point(px(1.), px(10.)));
    drag_to(&mut native, to);
    frame(&mut native);
    let copied = release(&mut native, to);
    assert!(
        copied.contains("delta") && copied.contains("epsilon"),
        "{copied:?}"
    );
    assert!(
        !copied.contains("alpha"),
        "the page under the dialog: {copied:?}"
    );
}

#[gpui_kit::test]
fn a_press_on_a_control_leaves_the_next_drag_its_own(cx: &mut gpui_kit::TestAppContext) {
    let label = wire::Node::Text(view_wire::TextNode {
        id: Some(named_id("send-label")),
        style: crate::render::test_style(div().style().clone()),
        content: "Send".into(),
        transform: None,
    });
    let mut send = container_with_style("send", div().h(px(20.)).style().clone(), [label]);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut send {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.role = Some(gpui_kit::Role::Button);
        interactivity.on_click = Some(5);
        interactivity.consumes_click = true;
    }
    let root = column(
        "room",
        vec![
            send,
            paragraph("alpha", "alpha"),
            paragraph("gamma", "gamma"),
        ],
    );
    let (mut native, _) = desk(root, false, cx);
    let button = point(px(5.), px(10.));
    press(&mut native, button);
    release(&mut native, button);
    let to = point(px(140.), px(50.));
    press(&mut native, point(px(1.), px(30.)));
    drag_to(&mut native, to);
    frame(&mut native);
    assert_eq!(release(&mut native, to), "alpha\ngamma");
}

#[gpui_kit::test]
fn a_drag_in_a_deferred_popover_selects_its_words(cx: &mut gpui_kit::TestAppContext) {
    let card = container_with_style(
        "card",
        div().w(px(150.)).flex().flex_col().style().clone(),
        [paragraph("delta", "delta"), paragraph("epsilon", "epsilon")],
    );
    let popover = wire::Node::Deferred {
        priority: 1,
        content: Box::new(column("popover", vec![card])),
    };
    let root = container_with_style("page", div().size_full().style().clone(), [popover]);
    let (mut native, _) = desk(root, false, cx);
    let to = point(px(100.), px(30.));
    press(&mut native, point(px(1.), px(10.)));
    drag_to(&mut native, to);
    frame(&mut native);
    let copied = release(&mut native, to);
    assert!(
        copied.contains("delta") && copied.contains("epsilon"),
        "{copied:?}"
    );
}
