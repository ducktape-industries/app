// Host-owned slot clipping cannot be replaced by guest refinements.
struct GuestSlot { tree: Entity<ViewTree> }
impl Render for GuestSlot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Intentionally no clipping here: ViewTree must impose its own boundary.
        div().relative().size(px(40.)).child(self.tree.clone())
    }
}

#[gpui_kit::test]
fn hostile_guest_position_size_and_overflow_remain_inside_host_slot(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let styles = [
        gpui_kit::StyleRefinement::default().absolute().left(px(30.)).size(px(100.)),
        gpui_kit::StyleRefinement::default().size(px(1e20)).m(px(-1e20)),
        gpui_kit::StyleRefinement::default().absolute().top(px(-100.)).size(px(100.)).opacity(5.),
    ];
    for mut style in styles {
        style.background = Some(rgb(0xff00ff).into());
        style.overflow.x = Some(gpui_kit::Overflow::Visible);
        style.overflow.y = Some(gpui_kit::Overflow::Visible);
        let root = wire::Node::Container {
            id: Some(wire::ElementIdWire::Integer(1)), style,
            interactivity: Default::default(), children: vec![],
        };
        let mut frame = wire::Frame { root: Some(root), ..Default::default() };
        wire::sanitize(&mut frame).unwrap();
        let root = frame.root.unwrap();
        let window = cx.open_window(size(px(200.), px(200.)), move |_, cx| {
            GuestSlot { tree: cx.new(|_| ViewTree::new(root)) }
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let quads: Vec<_> = window.painted_quads().into_iter()
                .filter(|quad| quad.background.as_solid() == Some(rgb(0xff00ff).into()))
                .collect();
            assert!(!quads.is_empty(), "guest content must still paint inside its slot");
            let bound = gpui_kit::ScaledPixels(40. * window.scale_factor());
            for quad in quads {
                let mask = quad.content_mask.bounds;
                assert!(mask.origin.x >= gpui_kit::ScaledPixels(0.));
                assert!(mask.origin.y >= gpui_kit::ScaledPixels(0.));
                assert!(mask.origin.x + mask.size.width <= bound, "{mask:?}");
                assert!(mask.origin.y + mask.size.height <= bound, "{mask:?}");
            }
        }).unwrap();
    }
}
