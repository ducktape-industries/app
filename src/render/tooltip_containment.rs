//! Tooltips whose content is a guest node. gpui draws a tooltip as its own
//! view, in the source view's layer (`deferred::Layer`): under the host's
//! band, fitted and clipped to the view's pane, so it can neither paint nor
//! take clicks outside it. `build` renders the content in a child
//! `ViewTree` and re-emits the child's events from the source tree with its
//! one-shot user activation, so the runtime accepts a click inside the
//! tooltip as the user's.
use super::*;
use gpui_kit::{Subscription, WeakEntity};

pub(super) fn build(
    parent: WeakEntity<ViewTree>,
    content: wire::Node,
    cx: &mut App,
) -> gpui_kit::AnyView {
    // the source view's pictures and their rasters: a tooltip names them
    // as its source tree does, and the seat releases them
    let (pictures, images) = parent
        .upgrade()
        .map(|parent| {
            let parent = parent.read(cx);
            (parent.pictures.clone(), parent.images.clone())
        })
        .unwrap_or_default();
    let child = cx.new(|_| {
        let mut child = ViewTree::new(content);
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
    use crate::render::tests::emitted;
    use gpui_kit::test::TestWindowExt as _;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        time::Duration,
    };

    const TOOLTIP_COLOR: u32 = 0xff00ff;

    fn node(size: f32, handler: Option<u32>) -> wire::Node {
        let mut element = div().size(px(size)).bg(rgb(TOOLTIP_COLOR));
        let interactivity = wire::Interactivity {
            on_click: handler,
            ..Default::default()
        };
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("tooltip-content".into())),
            style: element.style().clone(),
            interactivity: Box::new(interactivity),
            children: Vec::new(),
        })
    }

    fn parent_node() -> wire::Node {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("source-content".into())),
            style: div().size_full().style().clone(),
            interactivity: Default::default(),
            children: Vec::new(),
        })
    }

    fn ordinary_source(request: u32) -> wire::Node {
        sized_source(request, 40.)
    }

    fn sized_source(request: u32, size: f32) -> wire::Node {
        let interactivity = wire::Interactivity {
            tooltip: Some(wire::Tooltip {
                request,
                hoverable: false,
                delay_ms: 10,
            }),
            ..Default::default()
        };
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("ordinary-source".into())),
            style: div().size(px(size)).style().clone(),
            interactivity: Box::new(interactivity),
            children: Vec::new(),
        })
    }

    /// The guest's answer to `request`: `content`.
    fn answer(request: u32, content: wire::Node) -> wire::TooltipResponse {
        wire::TooltipResponse {
            request,
            character_index: None,
            content: Some(Box::new(content)),
        }
    }

    fn tooltip_painted(window: &mut Window) -> bool {
        window
            .painted_quads()
            .iter()
            .any(|quad| quad.background.as_solid() == Some(rgb(TOOLTIP_COLOR).into()))
    }

    struct TooltipFixture {
        parent: Entity<ViewTree>,
        tooltip: wire::Node,
        hoverable: bool,
    }

    impl Render for TooltipFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let parent = self.parent.downgrade();
            let tooltip = self.tooltip.clone();
            let source = div()
                .id("tooltip-source")
                .relative()
                .size(px(40.))
                .overflow_hidden()
                .tooltip_show_delay(Duration::from_millis(10))
                .child(self.parent.clone());
            let source = if self.hoverable {
                source.hoverable_tooltip(move |_, cx| build(parent.clone(), tooltip.clone(), cx))
            } else {
                source.tooltip(move |_, cx| build(parent.clone(), tooltip.clone(), cx))
            };
            // the source in its pane's layer, as `Render for ViewTree` draws one
            div()
                .size(px(40.))
                .overflow_hidden()
                .child(crate::render::deferred::Layer(source.into_any_element()))
        }
    }

    fn show_tooltip(
        cx: &mut gpui_kit::TestAppContext,
        handler: u32,
        hoverable: bool,
    ) -> (
        gpui_kit::VisualTestContext,
        Entity<ViewTree>,
        Rc<RefCell<Vec<wire::Event>>>,
    ) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), move |_, cx| TooltipFixture {
            parent: cx.new(|_| ViewTree::new(parent_node())),
            tooltip: node(120.0, Some(handler)),
            hoverable,
        });
        let fixture = window.root(cx).unwrap();
        let parent = cx.update(|cx| fixture.read(cx).parent.clone());
        let any_window = window.into();
        let mut native = gpui_kit::VisualTestContext::from_window(any_window, cx);
        let (events, subscription) = emitted(&parent, &mut native);
        subscription.detach();
        native.update(|window, cx| window.render_frame(cx));
        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.executor().advance_clock(Duration::from_millis(11));
        native.run_until_parked();
        native.update(|window, cx| window.render_frame(cx));
        (native, parent, events)
    }

    #[gpui_kit::test]
    fn native_tooltip_paint_and_hitbox_stay_inside_source_slot(cx: &mut gpui_kit::TestAppContext) {
        let (mut native, _, events) = show_tooltip(cx, 91, false);
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
        assert!(
            events.borrow().is_empty(),
            "clipped tooltip accepted an outside click"
        );
    }

    #[gpui_kit::test]
    fn tooltip_click_forwards_once_with_one_use_parent_activation(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (mut native, parent, events) = show_tooltip(cx, 92, true);
        native.simulate_mouse_move(point(px(20.), px(20.)), None, Default::default());
        native.simulate_click(point(px(20.), px(20.)), Default::default());
        let events = events.borrow();
        let clicks: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, wire::Event::Click { handler: 92, .. }))
            .collect();
        assert_eq!(clicks.len(), 1, "tooltip click must reach the parent once");
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
                div()
                    .size(px(40.))
                    .overflow_hidden()
                    .child(self.tree.clone())
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
                        tree.tooltip_responses(vec![answer(71, node(120., None))], cx)
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
        native.update(|window, cx| window.render_frame(cx));

        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.run_until_parked();
        native.update(|window, cx| window.render_frame(cx));
        native.executor().advance_clock(Duration::from_millis(11));
        native.run_until_parked();
        native.update(|window, cx| window.render_frame(cx));

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

    /// A `Deferred` in a guest tooltip's content draws, and paints, instead
    /// of failing the frame.
    #[gpui_kit::test]
    fn a_deferred_inside_a_tooltip_draws(cx: &mut gpui_kit::TestAppContext) {
        struct Slot(Entity<ViewTree>);
        impl Render for Slot {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size(px(40.)).overflow_hidden().child(self.0.clone())
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
                tree.tooltip_responses(vec![answer(93, content)], cx)
            });
            Slot(tree)
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| window.render_frame(cx));
        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.executor().advance_clock(Duration::from_millis(11));
        native.run_until_parked();
        native.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .painted_quads()
                    .iter()
                    .any(|quad| quad.background.as_solid() == Some(rgb(TOOLTIP_COLOR).into())),
                "the tooltip's deferred content paints"
            );
        });
    }

    /// The tooltip's content lives beside the tree, by route: a frame that
    /// changes the hovered node's props (here its size) replaces the tree
    /// and the open tooltip stays, since the route stays.
    #[gpui_kit::test]
    fn a_props_change_on_the_hovered_node_keeps_its_tooltip(cx: &mut gpui_kit::TestAppContext) {
        struct Host(Entity<ViewTree>);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size(px(60.)).overflow_hidden().child(self.0.clone())
            }
        }
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), |_, cx| {
            let tree = cx.new(|_| ViewTree::new(ordinary_source(71)));
            tree.update(cx, |tree, cx| {
                tree.tooltip_responses(vec![answer(71, node(20., None))], cx)
            });
            Host(tree)
        });
        let tree = window
            .root(cx)
            .unwrap()
            .read_with(cx, |host, _| host.0.clone());
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| window.render_frame(cx));
        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.executor().advance_clock(Duration::from_millis(11));
        native.run_until_parked();
        native.update(|window, cx| {
            window.render_frame(cx);
            assert!(tooltip_painted(window), "the tooltip opened");
        });
        native.update(|_, cx| {
            tree.update(cx, |tree, cx| tree.replace(sized_source(71, 44.), &[], cx));
        });
        native.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                tooltip_painted(window),
                "the props change closed the tooltip"
            );
        });
    }

    /// The store behind the tree: a response is kept only for a route the
    /// tree holds, on every node kind that carries a tooltip, and the
    /// contents together stay within the frame's node budget.
    #[gpui_kit::test]
    fn tooltip_responses_hold_the_trees_routes_within_the_node_budget(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let tip = |request| {
            Box::new(wire::Interactivity {
                tooltip: Some(wire::Tooltip {
                    request,
                    hoverable: false,
                    delay_ms: 10,
                }),
                ..Default::default()
            })
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
                style: Default::default(),
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
                style: Default::default(),
                interactivity: tip(2),
            },
            wire::Node::UniformList {
                id: name("uniform"),
                path: vec![name("uniform")],
                route: 90,
                style: Default::default(),
                interactivity: tip(3),
                count: 0,
                measure_index: 0,
                sizing: Default::default(),
                horizontal_sizing: Default::default(),
                y_flipped: false,
                scroll_request: None,
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
                style: Default::default(),
                interactivity: tip(4),
                children: Vec::new(),
            },
            wire::Node::ResizeHandle {
                id: name("grip"),
                style: Default::default(),
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
                style: Default::default(),
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
            tree.tooltip_responses(vec![answer(9, node(20., None))], cx)
        });
        assert_eq!(
            held(cx),
            [0u32; 0],
            "a route the tree does not hold took content"
        );

        let answers = (1..=5).map(|request| answer(request, node(20., None)));
        tree.update(cx, |tree, cx| tree.tooltip_responses(answers.collect(), cx));
        assert_eq!(
            held(cx),
            [1, 2, 3, 4, 5],
            "image, svg, uniform list, list and grip each keep their tooltip"
        );

        // with the four others it would hold one frame's budget and more
        let budget = wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: Default::default(),
            interactivity: Default::default(),
            children: vec![wire::Node::empty(); wire::MAX_NODES - 1],
        });
        tree.update(cx, |tree, cx| {
            tree.tooltip_responses(vec![answer(3, budget)], cx)
        });
        assert_eq!(held(cx), [3], "the store went past the node budget");
    }
}
