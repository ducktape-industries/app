//! Tooltips whose content is a guest node. The content is the view's own
//! drawing, so it ranks as the view's other draws do: the source's tree
//! opens the tooltip as the pointer rests on its source (`hover_tooltip`)
//! and draws it as a guest deferral at the guest ceiling, last in its slot
//! (`tooltip_layer`), clipped to the slot for paint and hit (`Contained`).
//! The host's band stays over it, so a host layer over the pane paints over
//! it and takes its clicks. gpui's own tooltip is drawn after every deferred
//! draw, over that band, so a view's content never goes there. It shows,
//! sits and closes as gpui's does (gpui-pre `div.rs`,
//! `register_tooltip_mouse_handlers`; `window.rs`, `prepaint_tooltip`).
//!
//! The content renders in a child `ViewTree` whose events the source tree
//! re-emits with its one-shot user activation, so the runtime accepts a
//! click inside the tooltip as the user's.
use super::*;
use gpui_kit::{AvailableSpace, ContentMask, ScrollWheelEvent, Subscription, Task, WeakEntity};
use std::{cell::Cell, rc::Rc, time::Duration};

/// The source view's clip, shared with the tooltips it opens.
pub(super) type SlotMask = Rc<Cell<ContentMask<Pixels>>>;

/// gpui's delays: a rich text's tooltip shows after the default
/// (`DEFAULT_TOOLTIP_SHOW_DELAY`); a hoverable one closes this long after
/// the pointer left it and its source (`HOVERABLE_TOOLTIP_HIDE_DELAY`).
pub(super) const RICH_TEXT_DELAY: Duration = Duration::from_millis(500);
const HOVERABLE_HIDE_DELAY: Duration = Duration::from_millis(500);

/// A tooltip's source: its node's authored path and its request in the
/// frame. A frame without it closes the tooltip, as gpui closes one whose
/// element is not drawn.
pub(super) type Source = (AuthoredPath, u32);

/// How a source's tooltip behaves.
#[derive(Clone, Copy)]
pub(super) struct Kind {
    pub(super) hoverable: bool,
    pub(super) delay: Duration,
}

/// The tooltip a view has open: waiting for its delay, then shown.
pub(super) struct Open {
    source: Source,
    kind: Kind,
    /// A rich text's: the character the pointer rests on as it shows.
    character_index: Option<u32>,
    /// The pointer as the delay passed: where the tooltip shows.
    at: Option<Point<Pixels>>,
    /// The content, built as the tooltip shows (or as its lazy content
    /// comes) and kept until it closes, as gpui keeps its tooltip's view.
    content: Option<(Entity<ViewTree>, Subscription)>,
    /// Where it drew last.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Where its source drew last (`SourceBounds`).
    source_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The show delay, or a hoverable tooltip's hide delay.
    timer: Option<Task<()>>,
}

impl ViewTree {
    /// The pointer came onto `source` (`at` its character, for a rich
    /// text) or left it.
    pub(super) fn hover_tooltip(
        &mut self,
        source: Source,
        kind: Kind,
        hovered: bool,
        character_index: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = self.tooltip.as_mut().filter(|open| open.source == source);
        match (open, hovered) {
            // on its source: a hoverable tooltip's hide is off
            (Some(open), true) if open.at.is_some() => open.timer = None,
            (Some(open), true) => open.character_index = character_index,
            (Some(open), false) if open.at.is_some() => self.check_tooltip(window, cx),
            (Some(_), false) => self.close_tooltip(cx),
            (None, true) => {
                let timer = cx.spawn_in(window, async move |tree, cx| {
                    cx.background_executor().timer(kind.delay).await;
                    let _ = tree.update_in(cx, |tree, window, cx| {
                        if let Some(open) = &mut tree.tooltip
                            && open.at.is_none()
                        {
                            open.at = Some(window.mouse_position());
                            open.timer = None;
                            cx.notify();
                        }
                    });
                });
                self.close_tooltip(cx);
                self.tooltip = Some(Open {
                    source,
                    kind,
                    character_index,
                    at: None,
                    content: None,
                    bounds: Default::default(),
                    source_bounds: Default::default(),
                    timer: Some(timer),
                });
            }
            (None, false) => {}
        }
    }

    /// gpui's check of a shown tooltip, on each pointer move: it stays
    /// while the pointer is inside its source's bounds (by bounds, not by
    /// hitbox, so the tooltip itself over its source does not close it)
    /// and is not typing, or inside a hoverable tooltip. A hoverable one
    /// closes `HOVERABLE_HIDE_DELAY` after the pointer left both, unless
    /// it comes back first.
    fn check_tooltip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &mut self.tooltip else {
            return;
        };
        if open.at.is_none() {
            return;
        }
        let mouse = window.mouse_position();
        let on_source = !window.last_input_was_keyboard()
            && open
                .source_bounds
                .get()
                .is_some_and(|bounds| bounds.contains(&mouse));
        let on_tooltip = open.kind.hoverable
            && open
                .bounds
                .get()
                .is_some_and(|bounds| bounds.contains(&mouse));
        if on_source || on_tooltip {
            open.timer = None;
        } else if !open.kind.hoverable {
            self.close_tooltip(cx);
        } else if open.timer.is_none() {
            open.timer = Some(cx.spawn_in(window, async move |tree, cx| {
                cx.background_executor().timer(HOVERABLE_HIDE_DELAY).await;
                let _ = tree.update(cx, |tree, cx| tree.close_tooltip(cx));
            }));
        }
    }

    /// A press or a scroll off a shown tooltip closes it, unless it is
    /// hoverable.
    fn tooltip_pressed_off(&mut self, cx: &mut Context<Self>) {
        if self
            .tooltip
            .as_ref()
            .is_some_and(|open| !open.kind.hoverable)
        {
            self.close_tooltip(cx);
        }
    }

    pub(super) fn close_tooltip(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.tooltip.take()
            && open.at.is_some()
        {
            cx.notify();
        }
    }

    /// A new frame keeps the open tooltip only if it still has its source.
    pub(super) fn retain_tooltip(&mut self, sources: &std::collections::HashSet<Source>) {
        if self
            .tooltip
            .as_ref()
            .is_some_and(|open| !sources.contains(&open.source))
        {
            self.tooltip = None;
        }
    }

    /// The open tooltip once shown with its content: a guest deferral at
    /// the guest ceiling, clipped to the slot, for the last child of the
    /// slot's clip box.
    pub(super) fn tooltip_layer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.tooltip.as_mut()?;
        let at = open.at?;
        if open.content.is_none() {
            let content = content(&self.root, &open.source, open.character_index)?;
            let child = cx.new(|_| ViewTree::new(content));
            let subscription = cx.subscribe(&child, |tree, child, event: &wire::Event, cx| {
                let child = child.read(cx);
                let handler = child.user_activation.get();
                if child.take_user_activation(event).is_some() {
                    tree.user_activation.set(handler);
                }
                cx.emit(event.clone());
            });
            open.content = Some((child, subscription));
        }
        let (child, _) = open.content.as_ref()?;
        let placed = Placed {
            content: child.clone().into_any_element(),
            at,
            bounds: open.bounds.clone(),
            tree: cx.entity().downgrade(),
        };
        let contained = Contained {
            child: placed.into_any_element(),
            mask: self.slot_mask.clone(),
        };
        Some(super::deferred::deferred(contained.into_any_element(), 16).into_any_element())
    }
}

impl ViewTree {
    /// `element`, drawn for `node`: the open tooltip's source records its
    /// bounds as it prepaints.
    pub(super) fn tooltip_source(&self, node: &wire::Node, element: AnyElement) -> AnyElement {
        match &self.tooltip {
            Some(open)
                if request(node) == Some(open.source.1) && open.source.0 == self.authored_path =>
            {
                SourceBounds {
                    child: element,
                    bounds: open.source_bounds.clone(),
                }
                .into_any_element()
            }
            _ => element,
        }
    }
}

/// The request of `node`'s tooltip, if it has one.
fn request(node: &wire::Node) -> Option<u32> {
    match node {
        wire::Node::RichText { tooltip, .. } => tooltip.as_ref().map(|tooltip| tooltip.request),
        _ => node
            .interactivity()
            .and_then(|interactivity| interactivity.tooltip.as_ref())
            .map(|tooltip| tooltip.request),
    }
}

/// The content of `source`'s tooltip in `root` (for a rich text, the
/// content for the character `character_index`), once the view sent it.
fn content(root: &wire::Node, source: &Source, character_index: Option<u32>) -> Option<wire::Node> {
    let mut found = None;
    commands::walk_authored_paths(root, &mut Vec::new(), &mut |node, path| {
        if found.is_some() || path != &source.0 {
            return;
        }
        found = match node {
            wire::Node::RichText {
                tooltip: Some(tooltip),
                ..
            } if tooltip.request == source.1
                && character_index.is_some()
                && tooltip.character_index == character_index =>
            {
                tooltip.content.as_deref().cloned()
            }
            _ => node
                .interactivity()
                .and_then(|interactivity| interactivity.tooltip.as_ref())
                .filter(|tooltip| tooltip.request == source.1)
                .and_then(|tooltip| tooltip.content.as_deref().cloned()),
        };
    });
    found
}

/// Collects `node`'s tooltip source, if it is one, into `sources`.
pub(super) fn sources(
    node: &wire::Node,
    path: &AuthoredPath,
    sources: &mut std::collections::HashSet<Source>,
) {
    if let Some(request) = request(node) {
        sources.insert((path.clone(), request));
    }
}

/// A tooltip's source, recording the bounds it drew at: layout, paint and
/// hit are its child's alone.
struct SourceBounds {
    child: AnyElement,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}
impl IntoElement for SourceBounds {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for SourceBounds {
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
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.bounds.set(Some(bounds));
        self.child.prepaint(window, cx);
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
        self.child.paint(window, cx);
    }
}

/// The content laid out as gpui lays out its own tooltip (as a root, at
/// its smallest), one pixel past the pointer and turned back inside the
/// window (gpui-pre `window.rs`, `prepaint_tooltip`). It takes no room
/// in its slot.
struct Placed {
    content: AnyElement,
    at: Point<Pixels>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    tree: WeakEntity<ViewTree>,
}
impl IntoElement for Placed {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Placed {
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
        let style = gpui_kit::Style {
            position: gpui_kit::Position::Absolute,
            ..Default::default()
        };
        (window.request_layout(style, None, cx), ())
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
        let size = self
            .content
            .layout_as_root(AvailableSpace::min_size(), window, cx);
        let mut bounds = Bounds::new(self.at + point(px(1.), px(1.)), size);
        let window_bounds = Bounds {
            origin: Point::default(),
            size: window.viewport_size(),
        };
        if bounds.right() > window_bounds.right() {
            let x = self.at.x - bounds.size.width - px(1.);
            bounds.origin.x = if x >= Pixels::ZERO {
                x
            } else {
                std::cmp::max(
                    Pixels::ZERO,
                    bounds.origin.x - bounds.right() - window_bounds.right(),
                )
            };
        }
        if bounds.bottom() > window_bounds.bottom() {
            let y = self.at.y - bounds.size.height - px(1.);
            bounds.origin.y = if y >= Pixels::ZERO {
                y
            } else {
                std::cmp::max(
                    Pixels::ZERO,
                    bounds.origin.y - bounds.bottom() - window_bounds.bottom(),
                )
            };
        }
        self.bounds.set(Some(bounds));
        self.content.prepaint_at(bounds.origin, window, cx);
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
        self.content.paint(window, cx);
        let tree = self.tree.clone();
        window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
            if phase == gpui_kit::DispatchPhase::Capture {
                let _ = tree.update(cx, |tree, cx| tree.check_tooltip(window, cx));
            }
        });
        let off = |tree: WeakEntity<ViewTree>, bounds: Rc<Cell<Option<Bounds<Pixels>>>>| {
            move |position: Point<Pixels>, cx: &mut App| {
                if !bounds
                    .get()
                    .is_some_and(|bounds| bounds.contains(&position))
                {
                    let _ = tree.update(cx, |tree, cx| tree.tooltip_pressed_off(cx));
                }
            }
        };
        let pressed = off(self.tree.clone(), self.bounds.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
            if phase == gpui_kit::DispatchPhase::Capture {
                pressed(event.position, cx);
            }
        });
        let scrolled = off(self.tree.clone(), self.bounds.clone());
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
            if phase == gpui_kit::DispatchPhase::Capture {
                scrolled(event.position, cx);
            }
        });
    }
}

/// The tooltip clipped to its source view's slot, for paint and for hit.
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
        window.with_content_mask(Some(self.mask.get()), |window| {
            self.child.prepaint(window, cx);
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

    fn source(request: u32, content: Option<wire::Node>, hoverable: bool) -> wire::Node {
        let interactivity = wire::Interactivity {
            tooltip: Some(wire::Tooltip {
                request,
                content: content.map(Box::new),
                hoverable,
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

    /// A view in a 40px slot of a 200px window.
    struct Slot {
        tree: Entity<ViewTree>,
    }
    impl Render for Slot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size(px(40.))
                .overflow_hidden()
                .child(self.tree.clone())
        }
    }

    /// `root` in a slot, the pointer rested on it past the tooltip's delay.
    fn hovered(
        cx: &mut gpui_kit::TestAppContext,
        root: wire::Node,
    ) -> (
        gpui_kit::VisualTestContext,
        Entity<ViewTree>,
        Rc<RefCell<Vec<wire::Event>>>,
    ) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), move |_, cx| Slot {
            tree: cx.new(|_| ViewTree::new(root)),
        });
        let slot = window.root(cx).unwrap();
        let tree = cx.update(|cx| slot.read(cx).tree.clone());
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let (events, subscription) = emitted(&tree, &mut native);
        subscription.detach();
        native.update(|window, cx| window.render_frame(cx));
        native.simulate_mouse_move(point(px(10.), px(10.)), None, Default::default());
        native.executor().advance_clock(Duration::from_millis(11));
        native.run_until_parked();
        native.update(|window, cx| window.render_frame(cx));
        (native, tree, events)
    }

    fn tooltip_quads(native: &mut gpui_kit::VisualTestContext) -> Vec<gpui_kit::Quad> {
        native.update(|window, _| {
            window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background.as_solid() == Some(rgb(TOOLTIP_COLOR).into()))
                .collect()
        })
    }

    #[gpui_kit::test]
    fn tooltip_paint_and_hitbox_stay_inside_source_slot(cx: &mut gpui_kit::TestAppContext) {
        let (mut native, _, events) = hovered(cx, source(91, Some(node(120., Some(91))), true));
        let quads = tooltip_quads(&mut native);
        assert!(!quads.is_empty(), "the tooltip did not paint");
        let bound = native.update(|window, _| gpui_kit::ScaledPixels(40. * window.scale_factor()));
        for quad in quads {
            let mask = quad.content_mask.bounds;
            assert!(mask.origin.x >= gpui_kit::ScaledPixels(0.));
            assert!(mask.origin.y >= gpui_kit::ScaledPixels(0.));
            assert!(mask.origin.x + mask.size.width <= bound, "{mask:?}");
            assert!(mask.origin.y + mask.size.height <= bound, "{mask:?}");
        }
        // over the tooltip's box, outside the slot: a hoverable tooltip
        // stays up there, and takes no click
        native.simulate_mouse_move(point(px(80.), px(20.)), None, Default::default());
        native.update(|window, cx| window.render_frame(cx));
        assert!(!tooltip_quads(&mut native).is_empty(), "the tooltip closed");
        native.simulate_click(point(px(80.), px(20.)), Default::default());
        assert!(
            !events
                .borrow()
                .iter()
                .any(|event| matches!(event, wire::Event::Click { .. })),
            "clipped tooltip accepted an outside click"
        );
    }

    #[gpui_kit::test]
    fn tooltip_click_forwards_once_with_one_use_parent_activation(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (mut native, parent, events) =
            hovered(cx, source(92, Some(node(120., Some(92))), true));
        native.simulate_mouse_move(point(px(20.), px(20.)), None, Default::default());
        native.simulate_click(point(px(20.), px(20.)), Default::default());
        let events = events.borrow();
        let clicks: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, wire::Event::Click { handler: 92, .. }))
            .collect();
        assert_eq!(clicks.len(), 1, "tooltip click must reach the parent once");
        parent.read_with(&native, |parent, _| {
            assert!(parent.take_user_activation(clicks[0]).is_some());
            assert!(parent.take_user_activation(clicks[0]).is_none());
        });
    }

    /// A tooltip's content is a guest deferral: a `Deferred` in it draws in
    /// place, and paints.
    #[gpui_kit::test]
    fn a_deferred_inside_a_tooltip_draws_in_place(cx: &mut gpui_kit::TestAppContext) {
        let content = wire::Node::Deferred {
            priority: 16,
            content: Box::new(node(20., None)),
        };
        let (mut native, _, _) = hovered(cx, source(93, Some(content), false));
        assert!(
            !tooltip_quads(&mut native).is_empty(),
            "the tooltip's deferred content paints"
        );
    }

    #[gpui_kit::test]
    fn a_plain_tooltip_closes_off_its_source(cx: &mut gpui_kit::TestAppContext) {
        let (mut native, _, _) = hovered(cx, source(94, Some(node(120., None)), false));
        assert!(!tooltip_quads(&mut native).is_empty(), "the tooltip opened");
        // on the tooltip's box, off the source
        native.simulate_mouse_move(point(px(100.), px(100.)), None, Default::default());
        native.update(|window, cx| window.render_frame(cx));
        assert!(tooltip_quads(&mut native).is_empty(), "it stayed");
    }

    #[gpui_kit::test]
    fn a_hoverable_tooltip_closes_a_delay_after_the_pointer_left_it(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (mut native, _, _) = hovered(cx, source(95, Some(node(120., None)), true));
        let shown = |native: &mut gpui_kit::VisualTestContext, wait: u64| {
            native.executor().advance_clock(Duration::from_millis(wait));
            native.run_until_parked();
            native.update(|window, cx| window.render_frame(cx));
            !tooltip_quads(native).is_empty()
        };
        // on the tooltip's box, off the source
        native.simulate_mouse_move(point(px(100.), px(100.)), None, Default::default());
        assert!(shown(&mut native, 600), "it closed with the pointer on it");
        native.simulate_mouse_move(point(px(180.), px(180.)), None, Default::default());
        assert!(shown(&mut native, 0), "it closed before its delay");
        assert!(!shown(&mut native, 501), "it stayed past its delay");
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
            let tree = cx.new(|_| ViewTree::new(source(71, None, false)));
            let subscription =
                cx.subscribe(&tree, move |_, source_tree, event: &wire::Event, cx| {
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
                    let source_tree = source_tree.downgrade();
                    cx.defer(move |cx| {
                        let _ = source_tree.update(cx, |tree, cx| {
                            tree.replace(source(71, Some(node(120., None)), false), cx)
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
        let quads = tooltip_quads(&mut native);
        assert!(!quads.is_empty(), "lazy tooltip did not paint");
        let bound = native.update(|window, _| gpui_kit::ScaledPixels(40. * window.scale_factor()));
        assert!(quads.iter().all(|quad| {
            let mask = quad.content_mask.bounds;
            mask.origin.x >= gpui_kit::ScaledPixels(0.)
                && mask.origin.y >= gpui_kit::ScaledPixels(0.)
                && mask.origin.x + mask.size.width <= bound
                && mask.origin.y + mask.size.height <= bound
        }));
    }
}
