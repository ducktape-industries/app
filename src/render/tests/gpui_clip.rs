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
        // negative margins and insets keep their sign (bounded): these
        // leave part of the box over the slot, which must clip it
        gpui_kit::StyleRefinement::default()
            .size(px(1e20))
            .mr(px(-1e20))
            .mb(px(-1e20)),
        gpui_kit::StyleRefinement::default()
            .absolute()
            .top(px(-50.))
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

// A pane smaller than the window, a list scrolled in it: a tooltip and a
// `Deferred` popup on the list's last visible row draw whole, inside the
// pane. The pane is their mask, not the list's clip, and the tooltip fits
// the pane, not the window.
#[gpui_kit::test]
fn a_tooltip_and_a_popup_on_a_scrolled_lists_last_row_draw_whole(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::{ScrollDelta, ScrollWheelEvent};
    use std::time::Duration;
    const MAGENTA: u32 = 0xff00ff;
    struct Pane {
        tree: Entity<ViewTree>,
    }
    impl Render for Pane {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(div().size(px(200.)).child(self.tree.clone()))
        }
    }
    fn container(
        id: &str,
        mut style: Div,
        interactivity: wire::Interactivity,
        children: Vec<wire::Node>,
    ) -> wire::Node {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(named_id(id)),
            style: style.style().clone(),
            interactivity: Box::new(interactivity),
            children,
        })
    }
    let magenta = |id: &str, style: Div| {
        container(id, style.bg(rgb(MAGENTA)), Default::default(), Vec::new())
    };
    let tooltip = |row: usize| wire::Interactivity {
        tooltip: Some(wire::Tooltip {
            request: row as u32,
            hoverable: false,
            delay_ms: 10,
        }),
        ..Default::default()
    };
    // 100 by 120: below the pointer it runs past the pane's foot
    let tip = |row: u32| wire::TooltipResponse {
        request: row,
        character_index: None,
        content: Some(Box::new(magenta("tip", div().w(px(100.)).h(px(120.))))),
    };
    // 100 by 60, under its row: past the list's foot, inside the pane
    let popup = || wire::Node::Deferred {
        priority: 16,
        content: Box::new(magenta(
            "popup",
            div().absolute().top(px(40.)).w(px(100.)).h(px(60.)),
        )),
    };
    // five rows of 40 in a list 100 high, scrolled by one row: the fourth
    // row is the last in view, at 80..120, cut at 100
    let list = |tooltips: bool| {
        let rows = (0..5)
            .map(|row| {
                let interactivity = match tooltips {
                    true => tooltip(row),
                    false => Default::default(),
                };
                let children = match (tooltips, row) {
                    (false, 3) => vec![popup()],
                    _ => Vec::new(),
                };
                container(
                    &format!("row-{row}"),
                    div().relative().w_full().h(px(40.)).flex_none(),
                    interactivity,
                    children,
                )
            })
            .collect();
        let mut scrolling = div().w_full().h(px(100.)).flex().flex_col();
        scrolling.style().overflow.y = Some(gpui_kit::Overflow::Scroll);
        container(
            "pane",
            div().size_full(),
            Default::default(),
            vec![container("list", scrolling, Default::default(), rows)],
        )
    };
    cx.update(gpui_kit::init);
    for (variant, tooltips) in [("tooltip", true), ("popup", false)] {
        let root = list(tooltips);
        let window = cx.open_window(size(px(400.), px(400.)), move |_, cx| {
            let tree = cx.new(|_| ViewTree::new(root));
            if tooltips {
                tree.update(cx, |tree, cx| {
                    tree.tooltip_responses((0..5).map(tip).collect(), cx)
                });
            }
            Pane { tree }
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| window.render_frame(cx));
        native.simulate_event(ScrollWheelEvent {
            position: point(px(20.), px(50.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });
        native.update(|window, cx| window.render_frame(cx));
        native.simulate_mouse_move(point(px(20.), px(90.)), None, Default::default());
        native.executor().advance_clock(Duration::from_millis(11));
        native.run_until_parked();
        native.update(|window, cx| {
            window.render_frame(cx);
            let scale = window.scale_factor();
            let quads: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background.as_solid() == Some(rgb(MAGENTA).into()))
                .collect();
            assert!(!quads.is_empty(), "{variant}: it paints");
            let pane = gpui_kit::ScaledPixels(200. * scale);
            for quad in quads {
                let (bounds, mask) = (quad.bounds, quad.content_mask.bounds);
                assert!(
                    bounds.origin.y.0 + bounds.size.height.0 <= pane.0,
                    "{variant}: past the pane's foot: {bounds:?}"
                );
                assert_eq!(
                    mask.intersect(&bounds),
                    bounds,
                    "{variant}: cut by its mask {mask:?}"
                );
                assert!(
                    mask.origin.y.0 + mask.size.height.0 <= pane.0,
                    "{variant}: masked past the pane: {mask:?}"
                );
            }
        });
    }
}
