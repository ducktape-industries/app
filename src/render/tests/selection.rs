use super::*;

fn paragraph(key: &str, text: &str) -> wire::Node {
    wire::Node::RichText {
        id: Some(named_id(key)),
        style: div().h(px(20.)).style().clone(),
        text: text.into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    }
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
    let window = cx.open_window(size(px(300.), px(100.)), |_, cx| Window2 {
        tree: cx.new(|_| ViewTree::new(panes())),
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
