use super::*;

const OPEN_THREAD: u32 = 31;

fn line(key: &str, text: &str) -> wire::Node {
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

/// A message as chat draws it: its words, and under them the button that
/// opens its thread (chat's "N replies · Open thread →"), which consumes
/// its click or not. The button lies on no paragraph, so its press is no
/// press on words for the host to weigh (`RichParagraph::paint`).
fn message(consumes: bool) -> wire::Node {
    let place = div().w(px(40.)).h(px(20.)).style().clone();
    let mut open = container_with_style("open-thread", place, []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut open {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.role = Some(gpui_kit::Role::Button);
        interactivity.aria.label = Some("Open thread".into());
        interactivity.on_click = Some(OPEN_THREAD);
        interactivity.consumes_click = consumes;
    }
    let row = div().flex().flex_col().style().clone();
    container_with_style("message", row, [line("alpha", "alpha beta"), open])
}

/// The room: its message at y 0 with the button at y 20, and another line
/// at y 80; once the thread is open, its pane beside the room.
fn room(consumes: bool, thread: bool) -> wire::Node {
    let pane = |key: &str, children: Vec<wire::Node>| {
        let style = div()
            .w(px(150.))
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .style()
            .clone();
        container_with_style(key, style, children)
    };
    let gap = |key: &str| container_with_style(key, div().h(px(10.)).style().clone(), []);
    let mut panes = vec![pane(
        "room",
        vec![
            message(consumes),
            gap("gap-1"),
            gap("gap-2"),
            gap("gap-3"),
            gap("gap-4"),
            line("gamma", "gamma"),
        ],
    )];
    if thread {
        panes.push(pane(
            "thread",
            vec![
                gap("gap-5"),
                gap("gap-6"),
                gap("gap-7"),
                line("reply", "reply"),
            ],
        ));
    }
    let style = div().flex().flex_row().size_full().style().clone();
    container_with_style("panes", style, panes)
}

/// The window as the shell draws it: gpui-base's `Root` paints the
/// selection layer before the view, so the layer's mouse listeners hear
/// a bubbling event after every element of the view has.
struct Desk {
    tree: Entity<ViewTree>,
}
impl Render for Desk {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(gpui_kit::base::TextSelectionLayer)
            .child(self.tree.clone())
    }
}

/// msg 259's steps: a click on "Open thread", the thread's pane opens,
/// and the pointer moves on over the room with no button held. What the
/// window has selected then.
fn selected_after_opening_and_moving(consumes: bool, cx: &mut gpui_kit::TestAppContext) -> String {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(100.)), |_, cx| Desk {
        tree: cx.new(|_| ViewTree::new(room(consumes, false))),
    });
    let tree = window
        .root(cx)
        .unwrap()
        .read_with(cx, |desk, _| desk.tree.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    let button = point(px(20.), px(30.));
    native.simulate_mouse_move(button, None, Default::default());
    native.simulate_mouse_down(button, MouseButton::Left, Default::default());
    native.simulate_mouse_up(button, MouseButton::Left, Default::default());
    let clicked = events.borrow().iter().any(|event| {
        matches!(
            event,
            wire::Event::Click {
                handler: OPEN_THREAD,
                ..
            }
        )
    });
    assert!(clicked, "the button heard its click");
    tree.update(&mut native, |tree, cx| {
        tree.replace(room(consumes, true), &[], cx)
    });
    native.update(|window, cx| window.render_frame(cx));
    let path = [
        point(px(100.), px(50.)),
        point(px(60.), px(70.)),
        point(px(20.), px(90.)),
    ];
    for at in path {
        native.simulate_mouse_move(at, None, Default::default());
        native.update(|window, cx| window.render_frame(cx));
    }
    native.update(gpui_kit::base::TextSelection::selected_text)
}

/// J-024 (msg 259): a click the view consumes still ends the press it
/// came from. The press began a gesture in gpui-base's window selection,
/// which ends it only on hearing the release bubble, and a consumed click
/// stopped that bubble: the gesture stayed live, and every move with no
/// button held went on selecting, as if the button were still down.
#[gpui_kit::test]
fn a_consumed_click_leaves_no_press_for_the_pointer_to_drag(cx: &mut gpui_kit::TestAppContext) {
    let selected = selected_after_opening_and_moving(true, cx);
    assert_eq!(selected, "", "a move with no button held selected words");
}

/// The same steps with a click nobody consumes: the release reaches the
/// selection, which ends the press. What tells the case above apart.
#[gpui_kit::test]
fn an_unconsumed_click_leaves_no_press_for_the_pointer_to_drag(cx: &mut gpui_kit::TestAppContext) {
    let selected = selected_after_opening_and_moving(false, cx);
    assert_eq!(selected, "", "a move with no button held selected words");
}
