use super::*;

fn paragraph(key: &str, text: &str) -> wire::Node {
    wire::Node::RichText {
        id: Some(named_id(key)),
        style: crate::render::test_style(div().h(px(20.)).style().clone()),
        text: text.into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    }
}

/// `paragraph`, every word of it a link.
fn linked(key: &str, text: &str) -> wire::Node {
    let mut node = paragraph(key, text);
    if let wire::Node::RichText {
        clickable_ranges,
        on_click,
        ..
    } = &mut node
    {
        clickable_ranges.push(0..text.len());
        *on_click = Some(9);
    }
    node
}

/// Two clipped panes side by side, as chat's room and thread: the room's
/// lines at y 0 and 60, the thread's between them at y 30.
fn panes() -> wire::Node {
    let pane = |key: &str, children: Vec<wire::Node>| {
        container_with_style(
            key,
            div()
                .w(px(150.))
                .h_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .style()
                .clone(),
            children,
        )
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

struct Window2 {
    tree: Entity<ViewTree>,
}
impl Render for Window2 {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(gpui_kit::base::TextSelectionLayer)
            .child(self.tree.clone())
    }
}

/// Drags from `from` through `through` and lets go; what the window copies.
fn drag(
    from: Point<Pixels>,
    through: &[Point<Pixels>],
    cx: &mut gpui_kit::TestAppContext,
) -> String {
    drag_over(panes(), from, through, cx)
}

fn drag_over(
    root: wire::Node,
    from: Point<Pixels>,
    through: &[Point<Pixels>],
    cx: &mut gpui_kit::TestAppContext,
) -> String {
    let window = cx.open_window(size(px(300.), px(100.)), |_, cx| Window2 {
        tree: cx.new(|_| ViewTree::new(root)),
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.simulate_mouse_move(from, None, Default::default());
    native.simulate_mouse_down(from, MouseButton::Left, Default::default());
    let mut last = from;
    for &at in through {
        native.simulate_mouse_move(at, Some(MouseButton::Left), Default::default());
        native.update(|window, cx| window.render_frame(cx));
        last = at;
    }
    native.simulate_mouse_up(last, MouseButton::Left, Default::default());
    native.update(|window, cx| {
        window.render_frame(cx);
        gpui_kit::base::TextSelection::selected_text(window, cx)
    })
}

/// A paragraph with links is a Tab stop, and still a drag selects its
/// words: the box takes no pointer press for itself.
#[gpui_kit::test]
fn a_drag_across_a_linked_paragraph_selects_its_words(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = container_with_style(
        "pane",
        div().w(px(150.)).h_full().flex().flex_col().style().clone(),
        [linked("alpha", "alpha beta"), paragraph("gamma", "gamma")],
    );
    let copied = drag_over(
        root,
        point(px(1.), px(10.)),
        &[point(px(140.), px(30.))],
        cx,
    );
    assert!(
        copied.contains("alpha") && copied.contains("gamma"),
        "{copied:?}"
    );
}

/// Enter on a picked link is its click while words stay selected elsewhere:
/// a selection tells a click that ended a drag from a click, and a key
/// ends no drag.
#[gpui_kit::test]
fn enter_presses_the_picked_link_while_words_are_selected(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = container_with_style(
        "pane",
        div().w(px(150.)).h_full().flex().flex_col().style().clone(),
        [paragraph("gamma", "gamma"), linked("alpha", "alpha beta")],
    );
    let window = cx.open_window(size(px(300.), px(100.)), |_, cx| Window2 {
        tree: cx.new(|_| ViewTree::new(root)),
    });
    let tree = window
        .root(cx)
        .unwrap()
        .read_with(cx, |window, _| window.tree.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    let (from, to) = (point(px(1.), px(10.)), point(px(40.), px(10.)));
    native.simulate_mouse_move(from, None, Default::default());
    native.simulate_mouse_down(from, MouseButton::Left, Default::default());
    native.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
    native.update(|window, cx| window.render_frame(cx));
    native.simulate_mouse_up(to, MouseButton::Left, Default::default());
    native.update(|window, cx| {
        window.render_frame(cx);
        assert!(gpui_kit::base::TextSelection::has_selection(window, cx));
        window.focus_next(cx);
        window.render_frame(cx);
        window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
    });
    assert_eq!(
        events.borrow().as_slice(),
        [wire::Event::Select {
            handler: 9,
            index: 0
        }]
    );
}

#[gpui_kit::test]
fn a_drag_down_a_pane_selects_its_lines_not_the_pane_beside_it(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let copied = drag(point(px(1.), px(10.)), &[point(px(140.), px(70.))], cx);
    assert!(
        copied.contains("alpha") && copied.contains("gamma"),
        "{copied:?}"
    );
    assert!(!copied.contains("beta"), "the thread's line: {copied:?}");
}

#[gpui_kit::test]
fn a_drag_up_a_pane_selects_its_lines_not_the_pane_beside_it(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let copied = drag(point(px(140.), px(70.)), &[point(px(1.), px(10.))], cx);
    assert!(
        copied.contains("alpha") && copied.contains("gamma"),
        "{copied:?}"
    );
    assert!(!copied.contains("beta"), "the thread's line: {copied:?}");
}

#[gpui_kit::test]
fn a_drag_that_wanders_into_the_next_pane_stays_in_its_own(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let copied = drag(
        point(px(1.), px(10.)),
        &[point(px(100.), px(40.)), point(px(200.), px(40.))],
        cx,
    );
    assert!(copied.contains("alpha"), "{copied:?}");
    assert!(!copied.contains("beta"), "the thread's line: {copied:?}");
}

/// A plain text (an author, a caption, a heading) is as selectable as a
/// paragraph: a drag down the pane copies the label and the line under it.
#[gpui_kit::test]
fn a_drag_across_a_plain_text_copies_it_too(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let label = wire::Node::Text(view_wire::TextNode {
        id: Some(named_id("author")),
        style: crate::render::test_style(div().h(px(20.)).style().clone()),
        content: "ada".into(),
    });
    let root = container_with_style(
        "pane",
        div().w(px(150.)).h_full().flex().flex_col().style().clone(),
        [label, paragraph("gamma", "gamma")],
    );
    let copied = drag_over(
        root,
        point(px(1.), px(10.)),
        &[point(px(140.), px(30.))],
        cx,
    );
    assert!(
        copied.contains("ada") && copied.contains("gamma"),
        "{copied:?}"
    );
}

/// A control's words are its label, not text to read: a drag down the pane
/// copies the lines around a button but not its word, and a double-click
/// on the word selects nothing, as on a browser's button.
#[gpui_kit::test]
fn a_controls_label_is_no_text_to_select(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let text = |key: &str, words: &str| {
        wire::Node::Text(view_wire::TextNode {
            id: Some(named_id(key)),
            style: crate::render::test_style(div().h(px(20.)).style().clone()),
            content: words.into(),
        })
    };
    let mut button = container_with_style(
        "send",
        div().h(px(20.)).style().clone(),
        [text("send-label", "Send")],
    );
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut button {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.role = Some(gpui_kit::Role::Button);
        interactivity.on_click = Some(5);
    }
    let root = container_with_style(
        "pane",
        div().w(px(150.)).h_full().flex().flex_col().style().clone(),
        [text("author", "ada"), button, paragraph("gamma", "gamma")],
    );
    let window = cx.open_window(size(px(300.), px(100.)), |_, cx| Window2 {
        tree: cx.new(|_| ViewTree::new(root)),
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let word = point(px(5.), px(30.));
    for click_count in [1, 2] {
        native.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: word,
            modifiers: Default::default(),
            click_count,
            first_mouse: false,
        });
        native.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: word,
            modifiers: Default::default(),
            click_count,
        });
    }
    let doubled = native.update(|window, cx| {
        window.render_frame(cx);
        gpui_kit::base::TextSelection::selected_text(window, cx)
    });
    assert_eq!(doubled, "", "a double-click on the button's word");
    let (from, to) = (point(px(1.), px(10.)), point(px(140.), px(50.)));
    native.simulate_mouse_move(from, None, Default::default());
    native.simulate_mouse_down(from, MouseButton::Left, Default::default());
    native.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
    native.update(|window, cx| window.render_frame(cx));
    native.simulate_mouse_up(to, MouseButton::Left, Default::default());
    let copied = native.update(|window, cx| {
        window.render_frame(cx);
        gpui_kit::base::TextSelection::selected_text(window, cx)
    });
    assert!(
        copied.contains("ada") && copied.contains("gamma"),
        "{copied:?}"
    );
    assert!(!copied.contains("Send"), "the button's word: {copied:?}");
}

/// A drag down a file's lines copies its code and not its line numbers: the
/// gutter is each line's list marker, drawn beside the content, as a
/// browser leaves an ordered list's numbers out of a copy.
#[gpui_kit::test]
fn a_drag_down_numbered_lines_copies_the_code_not_the_numbers(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let line = |number: &str, code: &str| {
        let mut gutter = container_with_style(
            &format!("gutter-{number}"),
            div().w(px(30.)).h(px(20.)).style().clone(),
            [wire::Node::Text(view_wire::TextNode {
                id: Some(named_id(&format!("number-{number}"))),
                style: crate::render::test_style(div().style().clone()),
                content: number.into(),
            })],
        );
        if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut gutter {
            interactivity.get_or_insert_default().role = Some(gpui_kit::Role::ListMarker);
        }
        // the code fills the line, as a code view's text does
        let mut code = paragraph(&format!("code-{number}"), code);
        if let wire::Node::RichText { style, .. } = &mut code {
            *style = crate::render::test_style(div().h(px(20.)).flex_1().style().clone());
        }
        container_with_style(
            &format!("line-{number}"),
            div().h(px(20.)).flex().flex_row().style().clone(),
            [gutter, code],
        )
    };
    let root = container_with_style(
        "file",
        div().w(px(280.)).h_full().flex().flex_col().style().clone(),
        [line("1", "fn a() {}"), line("2", "fn b() {}")],
    );
    let copied = drag_over(
        root,
        point(px(1.), px(10.)),
        &[point(px(270.), px(30.))],
        cx,
    );
    assert_eq!(copied, "fn a() {}\nfn b() {}");
}

/// A click on words leaves nothing behind it in the selection: gpui-base
/// would keep an empty anchor for a shift-click to extend from, and while it
/// holds one every frame publishes every paragraph to every other. So a
/// shift-click after a click selects nothing, and a double-click still
/// selects its word.
#[gpui_kit::test]
fn a_click_on_words_leaves_no_anchor_behind(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = container_with_style(
        "pane",
        div().w(px(150.)).h_full().flex().flex_col().style().clone(),
        [
            paragraph("alpha", "alpha beta"),
            paragraph("gamma", "gamma"),
        ],
    );
    let window = cx.open_window(size(px(300.), px(100.)), |_, cx| Window2 {
        tree: cx.new(|_| ViewTree::new(root)),
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let (word, below) = (point(px(5.), px(10.)), point(px(140.), px(30.)));
    let press = |native: &mut gpui_kit::VisualTestContext,
                 at: Point<Pixels>,
                 click_count: usize,
                 modifiers: gpui_kit::Modifiers| {
        native.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers,
            click_count,
            first_mouse: false,
        });
        native.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers,
            click_count,
        });
        native.update(|window, cx| {
            window.render_frame(cx);
            gpui_kit::base::TextSelection::selected_text(window, cx)
        })
    };
    press(&mut native, word, 1, gpui_kit::Modifiers::default());
    native.simulate_mouse_move(below, None, Default::default());
    assert_eq!(
        press(&mut native, below, 1, gpui_kit::Modifiers::shift()),
        "",
        "a shift-click after a click"
    );
    press(&mut native, word, 1, gpui_kit::Modifiers::default());
    assert_eq!(
        press(&mut native, word, 2, gpui_kit::Modifiers::default()),
        "alpha",
        "a double-click"
    );
}

/// A drag selecting words inside a clickable box (a chat message's cell)
/// is not a click on it: the words stay selected, the box hears nothing.
/// A plain click on the same box is still its click.
#[gpui_kit::test]
fn a_drag_selecting_words_in_a_clickable_box_is_not_its_click(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let mut root = container_with_style(
        "cell",
        div().w(px(150.)).h_full().flex().flex_col().style().clone(),
        [
            paragraph("alpha", "alpha beta"),
            paragraph("gamma", "gamma"),
        ],
    );
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut root {
        interactivity.get_or_insert_default().on_click = Some(5);
    }
    let window = cx.open_window(size(px(300.), px(100.)), |_, cx| Window2 {
        tree: cx.new(|_| ViewTree::new(root)),
    });
    let tree = window
        .root(cx)
        .unwrap()
        .read_with(cx, |window, _| window.tree.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    let clicks = |events: &Rc<RefCell<Vec<wire::Event>>>| {
        events
            .borrow()
            .iter()
            .filter(|event| matches!(event, wire::Event::Click { handler: 5, .. }))
            .count()
    };
    let (from, to) = (point(px(1.), px(10.)), point(px(140.), px(30.)));
    native.simulate_mouse_move(from, None, Default::default());
    native.simulate_mouse_down(from, MouseButton::Left, Default::default());
    native.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
    native.update(|window, cx| window.render_frame(cx));
    native.simulate_mouse_up(to, MouseButton::Left, Default::default());
    let copied = native.update(|window, cx| {
        window.render_frame(cx);
        gpui_kit::base::TextSelection::selected_text(window, cx)
    });
    assert!(
        copied.contains("alpha") && copied.contains("gamma"),
        "{copied:?}"
    );
    assert_eq!(clicks(&events), 0, "the drag was not a click");
    native.simulate_mouse_move(from, None, Default::default());
    native.simulate_mouse_down(from, MouseButton::Left, Default::default());
    native.simulate_mouse_up(from, MouseButton::Left, Default::default());
    native.update(|window, cx| window.render_frame(cx));
    assert_eq!(clicks(&events), 1, "a plain click is still a click");
    // a trackpad press drifts a little between down and up, on the words
    // themselves: no word selected, so still a click
    let drifted = point(px(1.5), px(10.));
    native.simulate_mouse_move(from, None, Default::default());
    native.simulate_mouse_down(from, MouseButton::Left, Default::default());
    native.simulate_mouse_move(drifted, Some(MouseButton::Left), Default::default());
    native.update(|window, cx| window.render_frame(cx));
    native.simulate_mouse_up(drifted, MouseButton::Left, Default::default());
    native.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        clicks(&events),
        2,
        "a press that drifted half a pixel is a click"
    );
    // the same drift with no frame between it and the release (a trackpad
    // lifts within a frame of its last move): until it paints again, a
    // paragraph the selection took in copies its whole text, however
    // little of it the drift covered
    press_drift_and_lift(&mut native, from, drifted);
    assert_eq!(
        clicks(&events),
        3,
        "a drift released before the next frame is a click"
    );
}

/// A press at `from`, then a move to `to` and the release there in one
/// dispatch: no frame paints between the last move and the lift, as on a
/// platform whose release lands before the next display refresh (the test
/// window otherwise draws after every simulated event).
fn press_drift_and_lift(
    native: &mut gpui_kit::VisualTestContext,
    from: Point<Pixels>,
    to: Point<Pixels>,
) {
    native.simulate_mouse_move(from, None, Default::default());
    native.simulate_mouse_down(from, MouseButton::Left, Default::default());
    native.update(|window, cx| {
        let moved = MouseMoveEvent {
            position: to,
            pressed_button: Some(MouseButton::Left),
            modifiers: Default::default(),
        };
        let lifted = MouseUpEvent {
            button: MouseButton::Left,
            position: to,
            modifiers: Default::default(),
            click_count: 1,
        };
        window.dispatch_event(moved.to_platform_input(), cx);
        window.dispatch_event(lifted.to_platform_input(), cx);
    });
    native.update(|window, cx| window.render_frame(cx));
}

/// A press on a link that drifts half a pixel and lifts before the next
/// frame is the link's press, as the same press on a clickable box is.
#[gpui_kit::test]
fn a_press_on_a_link_that_drifts_is_its_press(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = container_with_style(
        "pane",
        div().w(px(150.)).h_full().flex().flex_col().style().clone(),
        [linked("alpha", "alpha beta"), paragraph("gamma", "gamma")],
    );
    let window = cx.open_window(size(px(300.), px(100.)), |_, cx| Window2 {
        tree: cx.new(|_| ViewTree::new(root)),
    });
    let tree = window
        .root(cx)
        .unwrap()
        .read_with(cx, |window, _| window.tree.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    let (from, drifted) = (point(px(5.), px(10.)), point(px(5.5), px(10.)));
    press_drift_and_lift(&mut native, from, drifted);
    assert_eq!(
        events.borrow().as_slice(),
        [wire::Event::Select {
            handler: 9,
            index: 0
        }]
    );
}

#[test]
fn only_the_clip_a_drag_began_in_joins_it() {
    let room = Bounds::new(point(px(0.), px(0.)), size(px(150.), px(100.)));
    let thread = Bounds::new(point(px(150.), px(0.)), size(px(150.), px(100.)));
    assert!(
        super::text::joins_drag(None, thread),
        "no drag: all take part"
    );
    assert!(super::text::joins_drag(Some(room), room));
    assert!(!super::text::joins_drag(Some(room), thread));
}
