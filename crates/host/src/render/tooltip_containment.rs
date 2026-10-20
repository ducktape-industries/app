//! A tooltip's content, drawn in a child `ViewTree` whose events the source
//! tree emits again with its one-shot activation, so the runtime takes a click
//! inside the tooltip as the person's. `tips.rs` places it in the portal.
use super::*;
use gpui_kit::{Subscription, WeakEntity};

pub(super) fn build(
    parent: WeakEntity<ViewTree>,
    content: Tree,
    (pictures, images): (Arc<PictureBytes>, SharedRasters),
    cx: &mut App,
) -> gpui_kit::AnyView {
    let child = cx.new(|_| {
        let mut child = ViewTree::new(content);
        child.portal = None;
        child.set_pictures(pictures);
        child.images = images;
        child
    });
    cx.new(|cx| {
        let subscription = cx.subscribe(&child, move |_, source, event: &wire::Event, cx| {
            let activation = source.read(cx).take_activation();
            let _ = parent.update(cx, |parent, cx| {
                if let Some(at) = activation {
                    parent.activation.set(Some(at));
                }
                cx.emit(event.clone());
            });
        });
        TooltipHost {
            child,
            _subscription: subscription,
        }
    })
    .into()
}

struct TooltipHost {
    child: Entity<ViewTree>,
    _subscription: Subscription,
}
impl Render for TooltipHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.child.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::tests::{emitted, routed};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    const TOOLTIP_COLOR: u32 = 0xff00ff;

    fn node(size: f32, handler: Option<u32>) -> wire::Node {
        colored(size, TOOLTIP_COLOR, handler)
    }

    fn colored(size: f32, color: u32, handler: Option<u32>) -> wire::Node {
        let mut element = div().size(px(size)).bg(rgb(color));
        let interactivity = wire::Interactivity {
            on_click: handler,
            ..Default::default()
        };
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("tooltip-content".into())),
            style: crate::render::test_style(element.style().clone()),
            interactivity: Some(Box::new(interactivity)),
            children: Vec::new(),
        })
    }

    fn ordinary_source(request: u32) -> wire::Node {
        sized_source(request, 40.)
    }

    fn sized_source(request: u32, size: f32) -> wire::Node {
        named_source("ordinary-source", request, size)
    }

    fn hovered_over(mut source: wire::Node, hoverable: bool) -> wire::Node {
        let tooltip = match &mut source {
            wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) => interactivity
                .as_mut()
                .and_then(|interactivity| interactivity.tooltip.as_mut()),
            _ => None,
        };
        if let Some(tooltip) = tooltip {
            tooltip.hoverable = hoverable;
        }
        source
    }

    fn named_source(id: &str, request: u32, size: f32) -> wire::Node {
        let interactivity = wire::Interactivity {
            tooltip: Some(wire::Tooltip {
                request,
                hoverable: false,
                delay_ms: 10,
            }),
            ..Default::default()
        };
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name(id.to_owned().into())),
            style: crate::render::test_style(div().size(px(size)).style().clone()),
            interactivity: Some(Box::new(interactivity)),
            children: Vec::new(),
        })
    }

    fn answer(request: u32, content: wire::Node) -> wire::TooltipResponse {
        wire::TooltipResponse {
            request,
            character_index: None,
            content: Some(Box::new(content)),
        }
    }

    fn tooltip_painted(window: &mut Window) -> bool {
        painted(window, TOOLTIP_COLOR)
    }

    fn painted(window: &mut Window, color: u32) -> bool {
        window
            .painted_quads()
            .iter()
            .any(|quad| quad.background.as_solid() == Some(rgb(color).into()))
    }

    struct TooltipFixture(Entity<ViewTree>, f32);

    impl Render for TooltipFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            routed(
                div()
                    .size(px(self.1))
                    .overflow_hidden()
                    .child(self.0.clone()),
            )
        }
    }

    fn clicks(events: &RefCell<Vec<wire::Event>>) -> Vec<u32> {
        events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                wire::Event::Click { handler, .. } => Some(*handler),
                _ => None,
            })
            .collect()
    }

    fn show_tooltip(
        cx: &mut gpui_kit::TestAppContext,
        handler: u32,
        hoverable: bool,
        pane: f32,
    ) -> (
        gpui_kit::VisualTestContext,
        Entity<ViewTree>,
        Rc<RefCell<Vec<wire::Event>>>,
    ) {
        cx.update(gpui_kit::init);
        let source = hovered_over(sized_source(9, 40.), hoverable);
        let window = cx.open_window(size(px(200.), px(200.)), move |_, cx| {
            let tree = cx.new(|_| ViewTree::new(source));
            tree.update(cx, |tree, cx| {
                tree.tooltip_responses(answered([answer(9, node(120.0, Some(handler)))]), cx)
            });
            TooltipFixture(tree, pane)
        });
        let fixture = window.root(cx).unwrap();
        let tree = cx.update(|cx| fixture.read(cx).0.clone());
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let (events, subscription) = emitted(&tree, &mut native);
        subscription.detach();
        native.update(crate::render::draw_frame);
        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.run_until_parked();
        native.update(crate::render::draw_frame);
        (native, tree, events)
    }

    #[gpui_kit::test]
    fn native_tooltip_paint_and_hitbox_stay_inside_source_slot(cx: &mut gpui_kit::TestAppContext) {
        let (mut native, _, events) = show_tooltip(cx, 91, false, 40.);
        native.update(|window, _| {
            let quads: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background.as_solid() == Some(rgb(TOOLTIP_COLOR).into()))
                .collect();
            assert!(!quads.is_empty(), "native tooltip did not paint");
            let bound = gpui_kit::ScaledPixels(40. * window.scale_factor());
            for quad in quads {
                let mask = quad.content_mask.bounds;
                assert!(mask.origin.x >= gpui_kit::ScaledPixels(0.));
                assert!(mask.origin.y >= gpui_kit::ScaledPixels(0.));
                assert!(mask.origin.x + mask.size.width <= bound, "{mask:?}");
                assert!(mask.origin.y + mask.size.height <= bound, "{mask:?}");
            }
        });
        native.simulate_mouse_move(point(px(80.), px(20.)), None, Default::default());
        native.simulate_click(point(px(80.), px(20.)), Default::default());
        assert_eq!(
            clicks(&events),
            [0u32; 0],
            "clipped tooltip accepted an outside click"
        );
    }

    #[gpui_kit::test]
    fn tooltip_click_forwards_once_with_one_use_parent_activation(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (mut native, parent, events) = show_tooltip(cx, 92, true, 40.);
        native.simulate_mouse_move(point(px(20.), px(20.)), None, Default::default());
        native.simulate_click(point(px(20.), px(20.)), Default::default());
        assert_eq!(
            clicks(&events),
            [92],
            "tooltip click must reach the parent once"
        );
        parent.read_with(&native, |parent, _| {
            assert!(parent.take_activation().is_some());
            assert!(parent.take_activation().is_none());
        });
    }

    #[gpui_kit::test]
    fn ordinary_lazy_tooltip_opens_after_one_pointer_move(cx: &mut gpui_kit::TestAppContext) {
        struct Host {
            tree: Entity<ViewTree>,
            requests: Rc<Cell<usize>>,
            _subscription: Subscription,
        }
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                routed(
                    div()
                        .size(px(40.))
                        .overflow_hidden()
                        .child(self.tree.clone()),
                )
            }
        }

        cx.update(gpui_kit::init);
        let requests = Rc::new(Cell::new(0));
        let observed = requests.clone();
        let window = cx.open_window(size(px(200.), px(200.)), |_, cx| {
            let tree = cx.new(|_| ViewTree::new(ordinary_source(71)));
            let subscription = cx.subscribe(&tree, move |_, source, event: &wire::Event, cx| {
                if !matches!(
                    event,
                    wire::Event::TooltipRequest {
                        request: 71,
                        character_index: None
                    }
                ) {
                    return;
                }
                observed.set(observed.get() + 1);
                let source = source.downgrade();
                cx.defer(move |cx| {
                    let _ = source.update(cx, |tree, cx| {
                        tree.tooltip_responses(answered([answer(71, node(120., None))]), cx)
                    });
                });
            });
            Host {
                tree,
                requests: requests.clone(),
                _subscription: subscription,
            }
        });
        let host = window.root(cx).unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(crate::render::draw_frame);

        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.run_until_parked();
        native.update(crate::render::draw_frame);
        native.run_until_parked();
        native.update(crate::render::draw_frame);

        assert_eq!(host.read_with(&native, |host, _| host.requests.get()), 1);
        native.update(|window, _| {
            let quads: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background.as_solid() == Some(rgb(TOOLTIP_COLOR).into()))
                .collect();
            assert!(!quads.is_empty(), "lazy tooltip did not paint");
            let bound = gpui_kit::ScaledPixels(40. * window.scale_factor());
            assert!(quads.iter().all(|quad| {
                let mask = quad.content_mask.bounds;
                mask.origin.x >= gpui_kit::ScaledPixels(0.)
                    && mask.origin.y >= gpui_kit::ScaledPixels(0.)
                    && mask.origin.x + mask.size.width <= bound
                    && mask.origin.y + mask.size.height <= bound
            }));
        });
    }

    #[gpui_kit::test]
    fn a_deferred_inside_a_tooltip_draws(cx: &mut gpui_kit::TestAppContext) {
        struct Slot(Entity<ViewTree>);
        impl Render for Slot {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                routed(div().size(px(40.)).overflow_hidden().child(self.0.clone()))
            }
        }
        cx.update(gpui_kit::init);
        let content = wire::Node::Deferred {
            priority: 16,
            content: Box::new(node(20., None)),
        };
        let window = cx.open_window(size(px(200.), px(200.)), move |_, cx| {
            let tree = cx.new(|_| ViewTree::new(ordinary_source(93)));
            tree.update(cx, |tree, cx| {
                tree.tooltip_responses(answered([answer(93, content)]), cx)
            });
            Slot(tree)
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(crate::render::draw_frame);
        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.run_until_parked();
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            assert!(
                window
                    .painted_quads()
                    .iter()
                    .any(|quad| quad.background.as_solid() == Some(rgb(TOOLTIP_COLOR).into())),
                "the tooltip's deferred content paints"
            );
        });
    }

    #[gpui_kit::test]
    fn a_props_change_on_the_hovered_node_keeps_its_tooltip(cx: &mut gpui_kit::TestAppContext) {
        struct Host(Entity<ViewTree>);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                routed(div().size(px(60.)).overflow_hidden().child(self.0.clone()))
            }
        }
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), |_, cx| {
            let tree = cx.new(|_| ViewTree::new(ordinary_source(71)));
            tree.update(cx, |tree, cx| {
                tree.tooltip_responses(answered([answer(71, node(20., None))]), cx)
            });
            Host(tree)
        });
        let tree = window
            .root(cx)
            .unwrap()
            .read_with(cx, |host, _| host.0.clone());
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(crate::render::draw_frame);
        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.run_until_parked();
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            assert!(tooltip_painted(window), "the tooltip opened");
        });
        native.update(|_, cx| {
            tree.update(cx, |tree, cx| tree.replace(sized_source(71, 44.), &[], cx));
        });
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            assert!(
                tooltip_painted(window),
                "the props change closed the tooltip"
            );
        });
    }

    #[gpui_kit::test]
    fn a_tooltip_shows_the_answer_that_landed_before_it_drew(cx: &mut gpui_kit::TestAppContext) {
        const OLD: u32 = 0x00ff00;
        const NEW: u32 = 0x0000ff;
        struct Host(Entity<ViewTree>);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                routed(div().size(px(60.)).overflow_hidden().child(self.0.clone()))
            }
        }
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), |_, cx| {
            let tree = cx.new(|_| ViewTree::new(ordinary_source(71)));
            tree.update(cx, |tree, cx| {
                tree.tooltip_responses(answered([answer(71, colored(20., OLD, None))]), cx)
            });
            Host(tree)
        });
        let tree = window
            .root(cx)
            .unwrap()
            .read_with(cx, |host, _| host.0.clone());
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(crate::render::draw_frame);
        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.update(|window, cx| {
            tree.update(cx, |tree, cx| {
                tree.tooltip_responses(answered([answer(71, colored(20., NEW, None))]), cx)
            });
            crate::render::draw_frame(window, cx);
        });
        native.run_until_parked();
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            assert!(painted(window, NEW), "the tooltip shows the answer");
            assert!(!painted(window, OLD), "the tooltip shows what it held");
        });
    }

    #[gpui_kit::test]
    fn the_first_hover_after_a_tooltip_changed_shows_the_new_one(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        const FIRST: u32 = 0x00ff00;
        const SECOND: u32 = 0x0000ff;
        struct Host {
            tree: Entity<ViewTree>,
            _view: Subscription,
        }
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                routed(
                    div()
                        .size(px(40.))
                        .overflow_hidden()
                        .child(self.tree.clone()),
                )
            }
        }
        cx.update(gpui_kit::init);
        let says = Rc::new(Cell::new(FIRST));
        let view = says.clone();
        let window = cx.open_window(size(px(200.), px(200.)), move |_, cx| {
            let tree = cx.new(|_| ViewTree::new(ordinary_source(71)));
            let subscription = cx.subscribe(&tree, move |_, source, event: &wire::Event, cx| {
                if !matches!(event, wire::Event::TooltipRequest { request: 71, .. }) {
                    return;
                }
                let (source, color) = (source.downgrade(), view.get());
                cx.defer(move |cx| {
                    let _ = source.update(cx, |tree, cx| {
                        let content = colored(20., color, None);
                        tree.tooltip_responses(answered([answer(71, content)]), cx)
                    });
                });
            });
            Host {
                tree,
                _view: subscription,
            }
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let hover = |native: &mut gpui_kit::VisualTestContext, at: f32| {
            native.update(crate::render::draw_frame);
            native.simulate_mouse_move(point(px(at), px(at)), None, Default::default());
            native.run_until_parked();
            native.update(crate::render::draw_frame);
            native.run_until_parked();
            native.update(crate::render::draw_frame);
        };
        hover(&mut native, 10.);
        native.update(|window, _| assert!(painted(window, FIRST), "the first tooltip shows"));
        hover(&mut native, 150.);
        native.update(|window, _| assert!(!painted(window, FIRST), "it closes off the source"));
        says.set(SECOND);
        hover(&mut native, 10.);
        native.update(|window, _| {
            assert!(
                painted(window, SECOND),
                "the tooltip shows what it says now"
            );
            assert!(
                !painted(window, FIRST),
                "the tooltip shows what it said before"
            );
        });
    }

    #[gpui_kit::test]
    fn a_tooltip_is_drawn_with_the_table_it_came_with(cx: &mut gpui_kit::TestAppContext) {
        struct Host(Entity<ViewTree>);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                routed(div().size(px(60.)).overflow_hidden().child(self.0.clone()))
            }
        }
        let tree = |tip: u32| {
            let source = div().size(px(40.)).style().clone();
            let tip = div().size(px(20.)).bg(rgb(tip)).style().clone();
            let mut styles = wire::Styles::default();
            styles
                .extend(vec![wire::Style::new(&source), wire::Style::new(&tip)])
                .unwrap();
            let mut root = ordinary_source(71);
            let wire::Node::Container(view_wire::ContainerNode { style, .. }) = &mut root else {
                unreachable!()
            };
            *style = wire::StyleId(0);
            Tree { root, styles }
        };
        let mut content = node(20., None);
        let wire::Node::Container(view_wire::ContainerNode { style, .. }) = &mut content else {
            unreachable!()
        };
        *style = wire::StyleId(1);
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), |_, cx| {
            let first = tree(TOOLTIP_COLOR);
            let came_with = first.styles.clone();
            let view = cx.new(|_| ViewTree::new(first));
            view.update(cx, |view, cx| {
                view.tooltip_responses(vec![(answer(71, content), came_with)], cx);
                view.replace(tree(0x00ff00), &[], cx);
            });
            Host(view)
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(crate::render::draw_frame);
        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.run_until_parked();
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            assert!(
                tooltip_painted(window),
                "the tooltip wears the style its id names in a later table"
            );
        });
    }

    fn painted_at(native: &mut gpui_kit::VisualTestContext, at: f32) -> bool {
        native.simulate_mouse_move(point(px(at), px(at)), None, Default::default());
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            tooltip_painted(window)
        })
    }

    #[gpui_kit::test]
    fn a_tooltip_closes_off_its_source_and_a_hoverable_one_once_off_its_tooltip_too(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        for hoverable in [false, true] {
            let (mut native, _, _) = show_tooltip(cx, 94, hoverable, 200.);
            let case = format!("hoverable {hoverable}");
            assert!(
                painted_at(&mut native, 10.),
                "{case}: it opens on its source"
            );
            assert!(
                painted_at(&mut native, 30.),
                "{case}: over the tooltip on the source"
            );
            assert_eq!(
                painted_at(&mut native, 60.),
                hoverable,
                "{case}: over the tooltip off the source"
            );
            assert!(!painted_at(&mut native, 150.), "{case}: off both");
        }
    }

    #[gpui_kit::test]
    fn back_on_an_outer_source_from_an_inner_one_the_outer_tooltip_shows(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        const OUTER: u32 = 0x00ff00;
        const INNER: u32 = 0x0000ff;
        cx.update(gpui_kit::init);
        let mut outer = named_source("outer-source", 1, 40.);
        if let wire::Node::Container(view_wire::ContainerNode { children, .. }) = &mut outer {
            children.push(named_source("inner-source", 2, 20.));
        }
        let page_style = div().size_full().pt(px(20.)).pl(px(20.)).style().clone();
        let page = wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("page".into())),
            style: crate::render::test_style(page_style),
            interactivity: None,
            children: vec![outer],
        });
        let window = cx.open_window(size(px(200.), px(200.)), move |_, cx| {
            let tree = cx.new(|_| ViewTree::new(page));
            tree.update(cx, |tree, cx| {
                let answers = [
                    answer(1, colored(10., OUTER, None)),
                    answer(2, colored(10., INNER, None)),
                ];
                tree.tooltip_responses(answered(answers), cx)
            });
            TooltipFixture(tree, 200.)
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            let shown = painted(window, OUTER) || painted(window, INNER);
            assert!(!shown, "the pointer starts off both sources");
        });
        for (at, shows, hides, place) in [
            (50., OUTER, INNER, "on the outer source"),
            (30., INNER, OUTER, "on the inner source"),
            (50., OUTER, INNER, "back on the outer source"),
        ] {
            native.simulate_mouse_move(point(px(at), px(at)), None, Default::default());
            native.update(|window, cx| {
                crate::render::draw_frame(window, cx);
                assert!(painted(window, shows), "{place}: its tooltip shows");
                assert!(!painted(window, hides), "{place}: the other one shows");
            });
        }
    }

    #[gpui_kit::test]
    fn a_tooltip_paints_and_takes_clicks_under_the_pane_above_its_pane(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        const ABOVE: u32 = 0x00ff00;
        struct Desk {
            below: Entity<ViewTree>,
            log: Rc<RefCell<Vec<&'static str>>>,
        }
        impl Render for Desk {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                use gpui_kit::StatefulInteractiveElement as _;
                let log = self.log.clone();
                let below = div().absolute().size(px(100.)).child(
                    gpui_kit::AnyView::from(self.below.clone())
                        .cached(gpui_kit::StyleRefinement::default().size_full()),
                );
                let above = div()
                    .id("above")
                    .absolute()
                    .left(px(50.))
                    .size(px(100.))
                    .bg(rgb(ABOVE))
                    .occlude()
                    .on_click(move |_, _, _| log.borrow_mut().push("above"));
                routed(
                    div()
                        .relative()
                        .size(px(200.))
                        .child(crate::render::deferred::clipped(
                            below.into_any_element(),
                            0,
                        ))
                        .child(crate::render::deferred::clipped(
                            above.into_any_element(),
                            1,
                        )),
                )
            }
        }
        fn order(window: &mut Window, color: u32) -> gpui_kit::DrawOrder {
            window
                .painted_quads()
                .into_iter()
                .find(|quad| quad.background.as_solid() == Some(rgb(color).into()))
                .unwrap_or_else(|| panic!("{color:06x} paints"))
                .order
        }
        cx.update(gpui_kit::init);
        for hoverable in [false, true] {
            let source = hovered_over(sized_source(9, 100.), hoverable);
            let log = Rc::new(RefCell::new(Vec::new()));
            let window = cx.open_window(size(px(200.), px(200.)), {
                let log = log.clone();
                move |_, cx| {
                    let below = cx.new(|_| ViewTree::new(source));
                    below.update(cx, |tree, cx| {
                        tree.tooltip_responses(answered([answer(9, node(120., Some(95)))]), cx)
                    });
                    Desk { below, log }
                }
            });
            let below = window
                .root(cx)
                .unwrap()
                .read_with(cx, |desk, _| desk.below.clone());
            let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
            let (events, _subscription) = emitted(&below, &mut native);
            native.update(crate::render::draw_frame);
            native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
            native.update(|window, cx| {
                crate::render::draw_frame(window, cx);
                let (tip, above) = (order(window, TOOLTIP_COLOR), order(window, ABOVE));
                assert!(
                    tip < above,
                    "hoverable {hoverable}: the tooltip {tip} paints under the pane above {above}"
                );
            });
            native.simulate_click(point(px(75.), px(50.)), Default::default());
            let heard: Vec<_> = log.borrow_mut().drain(..).collect();
            assert_eq!(
                (heard, clicks(&events)),
                (vec!["above"], vec![]),
                "hoverable {hoverable}: the click on the spot they share"
            );
        }
    }

    fn drawn_twice(native: &mut gpui_kit::VisualTestContext) -> bool {
        native.update(crate::render::draw_frame);
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            tooltip_painted(window)
        })
    }

    #[gpui_kit::test]
    fn a_tooltip_closes_when_its_source_moves_from_under_a_still_pointer(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (mut native, tree, _) = show_tooltip(cx, 96, false, 200.);
        native.update(|window, _| assert!(tooltip_painted(window), "it opens on its source"));
        let page = wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("page".into())),
            style: crate::render::test_style(div().pt(px(100.)).style().clone()),
            interactivity: None,
            children: vec![sized_source(9, 40.)],
        });
        native.update(|_, cx| tree.update(cx, |tree, cx| tree.replace(page, &[], cx)));
        assert!(
            !drawn_twice(&mut native),
            "the source moved from under a still pointer, the tooltip stays open"
        );
    }

    #[gpui_kit::test]
    fn a_tooltip_closes_as_the_pointer_leaves_the_window(cx: &mut gpui_kit::TestAppContext) {
        for hoverable in [false, true] {
            let (mut native, _, _) = show_tooltip(cx, 97, hoverable, 200.);
            native.simulate_event(gpui_kit::MouseExitEvent {
                position: point(px(10.), px(10.)),
                ..Default::default()
            });
            assert!(
                !drawn_twice(&mut native),
                "hoverable {hoverable}: the pointer left the window, the tooltip stays open"
            );
        }
    }

    #[gpui_kit::test]
    fn a_press_closes_a_plain_tooltip_and_keeps_a_hoverable_one(cx: &mut gpui_kit::TestAppContext) {
        for hoverable in [false, true] {
            let (mut native, _, _) = show_tooltip(cx, 98, hoverable, 200.);
            let at = point(px(10.), px(10.));
            native.simulate_mouse_down(at, gpui_kit::MouseButton::Left, Default::default());
            native.simulate_mouse_up(at, gpui_kit::MouseButton::Left, Default::default());
            assert_eq!(
                drawn_twice(&mut native),
                hoverable,
                "hoverable {hoverable}: a press on the source"
            );
        }
    }

    #[gpui_kit::test]
    fn tooltip_responses_hold_the_trees_routes(cx: &mut gpui_kit::TestAppContext) {
        let tip = |request| {
            Some(Box::new(wire::Interactivity {
                tooltip: Some(wire::Tooltip {
                    request,
                    hoverable: false,
                    delay_ms: 10,
                }),
                ..Default::default()
            }))
        };
        let name = |name: &str| wire::ElementIdWire::Name(name.into());
        let kinds = vec![
            wire::Node::Image {
                id: Some(name("image")),
                hash: 0,
                data: None,
                label: None,
                image_style: wire::ImageStyle {
                    grayscale: false,
                    object_fit: wire::ImageObjectFit::Contain,
                },
                loading: false,
                fallback: false,
                state_children: Vec::new(),
                style: crate::render::plain_style(),
                interactivity: tip(1),
            },
            wire::Node::Svg {
                id: Some(name("svg")),
                source: wire::SvgSource::None,
                transformation: wire::SvgTransformation {
                    scale: [1.0, 1.0],
                    translate: [0.0, 0.0],
                    rotate: 0.0,
                },
                label: None,
                style: crate::render::plain_style(),
                interactivity: tip(2),
            },
            wire::Node::UniformList {
                id: name("uniform"),
                path: vec![name("uniform")],
                route: 90,
                style: crate::render::plain_style(),
                interactivity: tip(3),
                count: 0,
                measure_index: 0,
                sizing: Default::default(),
                horizontal_sizing: Default::default(),
                y_flipped: false,
                scroll_request: None,
                revision: 0,
                indices: Vec::new(),
                children: Vec::new(),
            },
            wire::Node::List {
                id: name("rows"),
                path: vec![name("list"), name("rows")],
                item_count: 0,
                alignment: wire::ListAlignment::Top,
                overdraw: 0.,
                sizing: wire::ListSizingBehavior::Infer,
                following_tail: false,
                revision: 0,
                commands: Vec::new(),
                request_handler: 91,
                scroll_handler: None,
                range_start: 0,
                style: crate::render::plain_style(),
                interactivity: tip(4),
                children: Vec::new(),
            },
            wire::Node::ResizeHandle {
                id: name("grip"),
                style: crate::render::plain_style(),
                interactivity: tip(5),
                on_press: None,
                on_release: None,
                on_drag: None,
                cursor: None,
                content: Box::new(wire::Node::empty()),
            },
        ];
        let tree = cx.new(|_| {
            ViewTree::new(wire::Node::Container(view_wire::ContainerNode {
                id: None,
                style: crate::render::plain_style(),
                interactivity: Default::default(),
                children: kinds,
            }))
        });
        let held = |cx: &mut gpui_kit::TestAppContext| {
            let mut routes: Vec<u32> =
                tree.read_with(cx, |tree, _| tree.tooltips.keys().copied().collect());
            routes.sort();
            routes
        };

        tree.update(cx, |tree, cx| {
            tree.tooltip_responses(answered([answer(9, node(20., None))]), cx)
        });
        assert_eq!(
            held(cx),
            [0u32; 0],
            "a route the tree does not hold took content"
        );

        let answers = (1..=5).map(|request| answer(request, node(20., None)));
        tree.update(cx, |tree, cx| tree.tooltip_responses(answered(answers), cx));
        assert_eq!(
            held(cx),
            [1, 2, 3, 4, 5],
            "image, svg, uniform list, list and grip each keep their tooltip"
        );
    }
}
