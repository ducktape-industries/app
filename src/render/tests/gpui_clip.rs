use super::*;

// Host-owned slot clipping cannot be replaced by guest refinements.
struct GuestSlot {
    tree: Entity<ViewTree>,
}
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
        gpui_kit::StyleRefinement::default()
            .absolute()
            .left(px(30.))
            .size(px(100.)),
        gpui_kit::StyleRefinement::default()
            .size(px(1e20))
            .m(px(-1e20)),
        gpui_kit::StyleRefinement::default()
            .absolute()
            .top(px(-100.))
            .size(px(100.))
            .opacity(5.),
    ];
    for mut style in styles {
        style.background = Some(rgb(0xff00ff).into());
        style.overflow.x = Some(gpui_kit::Overflow::Visible);
        style.overflow.y = Some(gpui_kit::Overflow::Visible);
        let root = wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Integer(1)),
            style,
            interactivity: Default::default(),
            children: vec![],
        });
        let mut frame = wire::Frame {
            root: Some(root),
            ..Default::default()
        };
        wire::sanitize(&mut frame).unwrap();
        let root = frame.root.unwrap();
        let window = cx.open_window(size(px(200.), px(200.)), move |_, cx| GuestSlot {
            tree: cx.new(|_| ViewTree::new(root)),
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let quads: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background.as_solid() == Some(rgb(0xff00ff).into()))
                .collect();
            assert!(
                !quads.is_empty(),
                "guest content must still paint inside its slot"
            );
            let bound = gpui_kit::ScaledPixels(40. * window.scale_factor());
            for quad in quads {
                let mask = quad.content_mask.bounds;
                assert!(mask.origin.x >= gpui_kit::ScaledPixels(0.));
                assert!(mask.origin.y >= gpui_kit::ScaledPixels(0.));
                assert!(mask.origin.x + mask.size.width <= bound, "{mask:?}");
                assert!(mask.origin.y + mask.size.height <= bound, "{mask:?}");
            }
        })
        .unwrap();
    }
}

// A pane narrower than the window: a popup asked for near its right edge
// fits the pane, whole, instead of fitting the window and being cut off.
#[gpui_kit::test]
fn an_anchored_popup_fits_its_slot_not_the_window(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    struct Pane {
        tree: Entity<ViewTree>,
    }
    impl Render for Pane {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(div().w(px(200.)).h_full().child(self.tree.clone()))
        }
    }
    let mut style = div().w(px(100.)).h(px(50.)).style().clone();
    style.background = Some(rgb(0xff00ff).into());
    let popup = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Integer(1)),
        style,
        interactivity: Default::default(),
        children: vec![],
    });
    let root = wire::Node::Anchored {
        anchor: wire::Anchor::TopLeft,
        fit: wire::AnchoredFitMode::SnapToWindowWithMargin([8.; 4]),
        position: Some([180., 170.]),
        position_mode: wire::AnchoredPositionMode::Window,
        offset: None,
        children: vec![popup],
    };
    let window = cx.open_window(size(px(400.), px(200.)), move |_, cx| Pane {
        tree: cx.new(|_| ViewTree::new(root)),
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let scale = window.scale_factor();
        let quad = window
            .painted_quads()
            .into_iter()
            .find(|quad| quad.background.as_solid() == Some(rgb(0xff00ff).into()))
            .expect("the popup paints");
        let (x, y) = (
            quad.bounds.origin.x.0 / scale,
            quad.bounds.origin.y.0 / scale,
        );
        // flipped to the left of and above the point, whole inside the pane
        assert_eq!((x, y), (80., 120.), "{quad:?}");
        assert!(quad.content_mask.bounds.size.width.0 / scale >= 100.);
    })
    .unwrap();
}
