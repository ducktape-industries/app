//! Tooltips whose content is a guest node. gpui draws a tooltip as its own
//! view on a top layer; `build` renders the content in a child `ViewTree`,
//! clips it to the source view's slot (`SlotMask`, recorded each frame in
//! `Render for ViewTree`) so it can neither paint nor take clicks outside the
//! view's box, and re-emits the child's events from the source tree with its
//! one-shot user activation, so the runtime accepts a click inside the
//! tooltip as the user's.
use super::*;
use gpui_kit::{ContentMask, Subscription, WeakEntity};
use std::{cell::Cell, rc::Rc};

/// The source view's clip, shared with the tooltips it opens.
pub(super) type SlotMask = Rc<Cell<ContentMask<Pixels>>>;

pub(super) fn build(
    parent: WeakEntity<ViewTree>,
    content: wire::Node,
    cx: &mut App,
) -> gpui_kit::AnyView {
    // the source view's clip and its pictures: a tooltip names them as
    // its source tree does
    let (mask, pictures) = parent
        .upgrade()
        .map(|parent| {
            let parent = parent.read(cx);
            (parent.slot_mask.clone(), parent.pictures.clone())
        })
        .unwrap_or_default();
    let child = cx.new(|_| {
        let mut child = ViewTree::new(content);
        child.set_pictures(pictures);
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
            mask,
            _subscription: subscription,
        }
    })
    .into()
}

struct TooltipHost {
    child: Entity<ViewTree>,
    mask: SlotMask,
    _subscription: Subscription,
}
impl Render for TooltipHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Contained {
            child: self.child.clone().into_any_element(),
            mask: self.mask.clone(),
        }
    }
}

pub(super) struct Contained {
    pub(super) child: AnyElement,
    pub(super) mask: SlotMask,
}
impl IntoElement for Contained {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Contained {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // gpui prepaints a tooltip after its deferred draws: a `Deferred` in
        // the content draws in place, not deferred too late to be drawn
        window.with_content_mask(Some(self.mask.get()), |window| {
            super::deferred::inside(|| self.child.prepaint(window, cx));
        });
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(self.mask.get()), |window| self.child.paint(window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::tests::emitted;
    use gpui_kit::test::TestWindowExt as _;
    use std::{cell::RefCell, time::Duration};

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
            interactivity,
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

    fn ordinary_source(request: u32, content: Option<wire::Node>) -> wire::Node {
        let interactivity = wire::Interactivity {
            tooltip: Some(wire::Tooltip {
                request,
                content: content.map(Box::new),
                hoverable: false,
                delay_ms: 10,
            }),
            ..Default::default()
        };
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("ordinary-source".into())),
            style: div().size(px(40.)).style().clone(),
            interactivity,
            children: Vec::new(),
        })
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
            if self.hoverable {
                source.hoverable_tooltip(move |_, cx| build(parent.clone(), tooltip.clone(), cx))
            } else {
                source.tooltip(move |_, cx| build(parent.clone(), tooltip.clone(), cx))
            }
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
            let tree = cx.new(|_| ViewTree::new(ordinary_source(71, None)));
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
                        tree.replace(ordinary_source(71, Some(node(120., None))), cx)
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

    /// A `Deferred` in a guest tooltip's content draws in place, and paints,
    /// instead of failing the frame.
    #[gpui_kit::test]
    fn a_deferred_inside_a_tooltip_draws_in_place(cx: &mut gpui_kit::TestAppContext) {
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
            Slot(cx.new(|_| ViewTree::new(ordinary_source(93, Some(content)))))
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
}
