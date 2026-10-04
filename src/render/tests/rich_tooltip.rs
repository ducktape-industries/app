use super::*;

#[gpui_kit::test]
fn rich_text_tooltip_dispatches_character_index_and_stays_in_slot(
    cx: &mut gpui_kit::TestAppContext,
) {
    use std::time::Duration;

    const TIP_COLOR: u32 = 0xff00ff;

    struct Host {
        tree: Entity<ViewTree>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size(px(40.))
                .overflow_hidden()
                .child(self.tree.clone())
        }
    }

    let mut tip = div().size(px(120.)).bg(rgb(TIP_COLOR));
    let tip = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("rich-tip-content")),
        style: tip.style().clone(),
        interactivity: Default::default(),
        children: Vec::new(),
    });
    fn rich(tooltip: u32) -> wire::Node {
        let mut style = div().size(px(40.)).text_color(rgb(0xffffff));
        wire::Node::RichText {
            id: Some(named_id("rich-tip-source")),
            style: style.style().clone(),
            text: "A".into(),
            runs: wire::RichTextRuns::Highlights(Vec::new()),
            font_family_overrides: Vec::new(),
            clickable_ranges: Vec::new(),
            on_click: None,
            on_hover: None,
            tooltip: Some(tooltip),
        }
    }

    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(200.), px(200.)), move |_, cx| Host {
        tree: cx.new(|_| ViewTree::new(rich(41))),
    });
    let host = window.root(cx).unwrap();
    let tree = cx.update(|cx| host.read(cx).tree.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, subscription) = emitted(&tree, &mut native);
    subscription.detach();
    native.update(|window, cx| window.render_frame(cx));
    native.simulate_mouse_move(point(px(2.), px(8.)), None, Default::default());
    let character_index = events.borrow().iter().find_map(|event| match event {
        wire::Event::TooltipRequest {
            request: 41,
            character_index,
        } => *character_index,
        _ => None,
    });
    assert_eq!(character_index, Some(0));
    native.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.tooltip_responses(
                vec![wire::TooltipResponse {
                    request: 41,
                    character_index,
                    content: Some(Box::new(tip)),
                }],
                cx,
            );
        });
    });
    native.update(|window, cx| window.render_frame(cx));
    native.executor().advance_clock(Duration::from_millis(501));
    native.run_until_parked();
    native.update(|window, cx| window.render_frame(cx));

    native.update(|window, _| {
        let quads: Vec<_> = window
            .painted_quads()
            .into_iter()
            .filter(|quad| quad.background.as_solid() == Some(rgb(TIP_COLOR).into()))
            .collect();
        assert!(!quads.is_empty(), "native rich-text tooltip did not paint");
        let bound = gpui_kit::ScaledPixels(40. * window.scale_factor());
        for quad in quads {
            let mask = quad.content_mask.bounds;
            assert!(mask.origin.x + mask.size.width <= bound, "{mask:?}");
            assert!(mask.origin.y + mask.size.height <= bound, "{mask:?}");
        }
    });
}
