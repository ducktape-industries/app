//! A view's layer and a desk window's. A view's deferred draws are drawn by
//! its portal, the last child of its pane's clip, by priority: over the view,
//! under the pane above it and under every host layer. Desk windows draw masked.
use super::*;
use gpui_kit::AvailableSpace;
use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

type Scopes = Rc<RefCell<Option<Vec<super::pointer::ScopeKey>>>>;
type Mask = Rc<Cell<Option<gpui_kit::ContentMask<Pixels>>>>;
pub(super) type Portal = Rc<RefCell<Vec<Placed>>>;

/// The host's band, over every window's rank and every priority a view
/// asks for: each window draws at its rank (`PaneLayer`), and a view's
/// draws inside its window's; the host's layers over the desk (its dialogs,
/// the bar's menus, the footer) defer at `HOST_BAND + n`, so no window and
/// no view draw paints or takes a click above them.
pub const HOST_BAND: usize = usize::MAX / 2;

pub(super) const VIEW_CEILING: usize = 16;

thread_local! {
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

pub(super) struct Placed {
    key: u64,
    priority: usize,
    bounds: Bounds<Pixels>,
    element: AnyElement,
}

impl ViewTree {
    pub(super) fn deferred(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Deferred {
            content, priority, ..
        } = node
        else {
            unreachable!()
        };
        let Some(portal) = self.portal.clone() else {
            return self.node(content, window, cx);
        };
        deferred(self.node(content, window, cx), *priority, &portal).into_any_element()
    }
}

pub(super) fn deferred(child: AnyElement, priority: usize, portal: &Portal) -> SlotDeferred {
    let scopes = Scopes::default();
    SlotDeferred {
        child: Some(Inside(child, scopes.clone()).into_any_element()),
        priority: priority.min(VIEW_CEILING),
        scopes,
        portal: portal.clone(),
    }
}

pub(super) struct SlotDeferred {
    child: Option<AnyElement>,
    priority: usize,
    scopes: Scopes,
    portal: Portal,
}

fn filling() -> gpui_kit::Style {
    gpui_kit::Style {
        position: gpui_kit::Position::Absolute,
        inset: gpui_kit::Edges::all(gpui_kit::Length::Definite(px(0.).into())),
        ..Default::default()
    }
}

impl IntoElement for SlotDeferred {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for SlotDeferred {
    type RequestLayoutState = bool;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, bool) {
        let child = self.child.as_mut().expect("deferred child before prepaint");
        if INSIDE.get() {
            return (child.request_layout(window, cx), true);
        }
        (window.request_layout(filling(), None, cx), false)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        in_place: &mut bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if *in_place {
            self.child
                .as_mut()
                .expect("deferred child before prepaint")
                .prepaint(window, cx);
            return;
        }
        let element = self.child.take().expect("deferred child is drawn once");
        *self.scopes.borrow_mut() = super::pointer::stack(window, cx);
        let key = window.with_global_id(ElementId::Name("deferred".into()), |id, _| {
            let mut hasher = std::hash::DefaultHasher::new();
            id.hash(&mut hasher);
            hasher.finish()
        });
        self.portal.borrow_mut().push(Placed {
            key,
            priority: self.priority,
            bounds,
            element,
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _in_place: &mut bool,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child) = &mut self.child {
            child.paint(window, cx);
        }
    }
}

pub(super) struct Portaled(pub(super) Portal, pub(super) Option<AnyElement>);

impl IntoElement for Portaled {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Portaled {
    type RequestLayoutState = ();
    type PrepaintState = Vec<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (window.request_layout(filling(), None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let tip = self.1.take();
        let mut placed = std::mem::take(&mut *self.0.borrow_mut());
        placed.sort_by_key(|placed| placed.priority);
        let mut seen = std::collections::HashMap::<u64, u64>::new();
        inside(|| {
            let mut drawn: Vec<_> = placed
                .into_iter()
                .map(|placed| {
                    let nth = seen.entry(placed.key).or_default();
                    let mut hasher = std::hash::DefaultHasher::new();
                    (placed.key, *nth).hash(&mut hasher);
                    *nth += 1;
                    let mut element = div()
                        .id(ElementId::Integer(hasher.finish()))
                        .w(placed.bounds.size.width)
                        .h(placed.bounds.size.height)
                        .child(placed.element)
                        .into_any_element();
                    element.prepaint_as_root(
                        placed.bounds.origin,
                        placed.bounds.size.map(AvailableSpace::Definite),
                        window,
                        cx,
                    );
                    element
                })
                .collect();
            drawn.extend(tip.map(|mut tip| {
                tip.prepaint_as_root(
                    bounds.origin,
                    bounds.size.map(AvailableSpace::Definite),
                    window,
                    cx,
                );
                tip
            }));
            drawn
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        drawn: &mut Vec<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        for element in drawn {
            element.paint(window, cx);
        }
    }
}

/// A desk window's layer (`PaneLayer`): `child` drawn after the window's
/// other draws at `priority`, and only inside the content mask it is laid
/// out in (the desk's), so a window dragged past the desk's edge never
/// paints or takes a press over the chrome beside it.
pub fn clipped(child: AnyElement, priority: usize) -> Clipped {
    let mask = Mask::default();
    Clipped {
        child: Some(Masked(child, mask.clone()).into_any_element()),
        priority,
        mask,
    }
}

/// See [`clipped`].
pub struct Clipped {
    child: Option<AnyElement>,
    priority: usize,
    mask: Mask,
}

impl IntoElement for Clipped {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Clipped {
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
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (
            self.child
                .as_mut()
                .expect("deferred child before prepaint")
                .request_layout(window, cx),
            (),
        )
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) {
        let child = self.child.take().expect("deferred child is drawn once");
        self.mask.set(Some(window.content_mask()));
        window.defer_draw(child, window.element_offset(), self.priority, None);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

struct Masked(AnyElement, Mask);

impl IntoElement for Masked {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Masked {
    type RequestLayoutState = ();
    type PrepaintState = Option<gpui_kit::ContentMask<Pixels>>;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.0.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<gpui_kit::ContentMask<Pixels>> {
        let mask = self.1.get();
        window.with_content_mask(mask, |window| self.0.prepaint(window, cx));
        mask
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        mask: &mut Option<gpui_kit::ContentMask<Pixels>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(*mask, |window| self.0.paint(window, cx));
    }
}

pub(super) struct Layer(pub(super) AnyElement, pub(super) Option<crate::perf::Key>);

impl Layer {
    fn time(&self, stage: &'static str) -> Option<crate::perf::Timer> {
        self.1.and_then(|key| crate::perf::time(key, stage))
    }
}

impl IntoElement for Layer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Layer {
    type RequestLayoutState = Option<crate::perf::Timer>;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Option<crate::perf::Timer>) {
        let requesting = self.time("layout.request");
        let id = self.0.request_layout(window, cx);
        drop(requesting);
        (id, self.time("layout.solve"))
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        solving: &mut Option<crate::perf::Timer>,
        window: &mut Window,
        cx: &mut App,
    ) {
        drop(solving.take());
        let _timed = self.time("prepaint");
        self.0.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Option<crate::perf::Timer>,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let _timed = self.time("paint");
        self.0.paint(window, cx);
    }
}

pub(super) fn inside<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            INSIDE.set(self.0);
        }
    }
    let _outer = Restore(INSIDE.replace(true));
    f()
}

struct Inside(AnyElement, Scopes);

impl IntoElement for Inside {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Inside {
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
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.0.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let scopes = self.1.borrow_mut().take();
        super::pointer::within(scopes, window, cx, |window, cx| {
            inside(|| self.0.prepaint(window, cx))
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Clipped(Portal);
    impl Render for Clipped {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let overlay = div()
                .absolute()
                .left(px(30.))
                .size(px(100.))
                .bg(rgb(0xff00ff))
                .into_any_element();
            div()
                .size(px(40.))
                .overflow_hidden()
                .child(Layer(
                    deferred(overlay, usize::MAX, &self.0).into_any_element(),
                    None,
                ))
                .child(Portaled(self.0.clone(), None))
        }
    }

    #[gpui_kit::test]
    fn deferred_view_paint_keeps_slot_mask(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), |_, _| Clipped(Portal::default()));
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let quads: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background.as_solid() == Some(rgb(0xff00ff).into()))
                .collect();
            assert!(!quads.is_empty(), "the deferred view child must paint");
            let bound = gpui_kit::ScaledPixels(40. * window.scale_factor());
            for quad in quads {
                assert!(
                    quad.content_mask.bounds.size.width <= bound,
                    "{:?}",
                    quad.content_mask
                );
                assert!(
                    quad.content_mask.bounds.size.height <= bound,
                    "{:?}",
                    quad.content_mask
                );
            }
        })
        .unwrap();
    }

    const SPILL: u32 = 0x00ffff;

    struct Spilled(Rc<RefCell<Vec<&'static str>>>);
    impl Render for Spilled {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            use gpui_kit::{InteractiveElement as _, StatefulInteractiveElement as _};
            let log = self.0.clone();
            let spill = div()
                .id("spill")
                .absolute()
                .left(px(30.))
                .size(px(100.))
                .bg(rgb(SPILL))
                .occlude()
                .on_click(move |_, _, _| log.borrow_mut().push("spill"));
            div()
                .size(px(40.))
                .overflow_hidden()
                .child(clipped(spill.into_any_element(), 0))
        }
    }

    #[gpui_kit::test]
    fn a_clipped_draw_paints_and_takes_clicks_only_inside_its_mask(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let log = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(200.), px(200.)), {
            let log = log.clone();
            move |_, _| Spilled(log)
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            let quads: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background.as_solid() == Some(rgb(SPILL).into()))
                .collect();
            assert!(!quads.is_empty(), "the clipped draw paints");
            let bound = gpui_kit::ScaledPixels(40. * window.scale_factor());
            for quad in quads {
                let mask = quad.content_mask.bounds;
                assert!(mask.origin.x + mask.size.width <= bound, "{mask:?}");
            }
        });
        native.simulate_click(point(px(100.), px(10.)), Default::default());
        assert!(
            log.borrow().is_empty(),
            "a click outside the mask reached the draw: {:?}",
            log.borrow()
        );
        native.simulate_click(point(px(35.), px(10.)), Default::default());
        assert_eq!(*log.borrow(), ["spill"]);
    }

    #[gpui_kit::test]
    fn a_draw_times_gpuis_passes_over_the_tree(cx: &mut gpui_kit::TestAppContext) {
        let _on = crate::perf::on_for_test();
        let key = crate::perf::Key::View {
            program: "layer-test",
            instance: 1,
        };
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), |_, cx| {
            crate::render::tests::Seat(
                cx.new(|_| ViewTree::new(wire::Node::Space).with_perf_key(key)),
            )
        });
        let seat = window.root(cx).unwrap();
        let samples = || {
            let view = crate::perf::snapshot(false)["views"]["layer-test"].clone();
            [
                "render",
                "layout.request",
                "layout.solve",
                "prepaint",
                "paint",
            ]
            .map(|stage| view[stage]["n"].as_u64().unwrap_or(0))
        };
        cx.update_window(window.into(), |_, window, cx| {
            crate::perf::reset();
            seat.update(cx, |seat, cx| seat.0.update(cx, |_, cx| cx.notify()));
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert_eq!(samples(), [1; 5]);
        cx.update_window(window.into(), |_, window, cx| {
            seat.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert_eq!(samples(), [1; 5], "a cached draw runs no pass");
    }

    #[gpui_kit::test]
    fn nested_view_deferrals_draw_in_place(cx: &mut gpui_kit::TestAppContext) {
        const DEPTH: usize = 11;
        fn leaf() -> wire::Node {
            let style = gpui_kit::StyleRefinement::default()
                .size(px(20.))
                .bg(rgb(0xff00ff));
            wire::Node::Container(view_wire::ContainerNode {
                id: None,
                style: crate::render::test_style(style),
                interactivity: Default::default(),
                children: Vec::new(),
            })
        }
        fn deferred(content: wire::Node) -> wire::Node {
            wire::Node::Deferred {
                priority: 16,
                content: Box::new(content),
            }
        }
        fn listed(level: usize, path: &mut Vec<wire::ElementIdWire>) -> wire::Node {
            if level == DEPTH {
                return leaf();
            }
            let id = wire::ElementIdWire::Name(format!("list-{level}").into());
            path.push(id.clone());
            let own = path.clone();
            path.push(wire::ElementIdWire::Integer(0));
            let row = listed(level + 1, path);
            path.pop();
            path.pop();
            deferred(wire::Node::UniformList {
                id,
                path: own,
                route: 1,
                style: crate::render::test_style(
                    gpui_kit::StyleRefinement::default().size(px(40.)),
                ),
                interactivity: Default::default(),
                count: 1,
                measure_index: 0,
                sizing: wire::list::UniformListSizing::Auto,
                horizontal_sizing: wire::list::UniformListHorizontalSizing::FitList,
                y_flipped: false,
                scroll_request: None,
                revision: 0,
                indices: vec![0],
                children: vec![row],
            })
        }
        cx.update(gpui_kit::init);
        let direct = (0..DEPTH).fold(leaf(), |node, _| deferred(node));
        for root in [direct, listed(0, &mut Vec::new())] {
            let mut frame = wire::Frame {
                root: Some(root),
                ..Default::default()
            };
            crate::render::sanitize_whole(&mut frame).unwrap();
            let root = frame.root.unwrap();
            let window = cx.open_window(size(px(200.), px(200.)), move |_, _| ViewTree::new(root));
            cx.update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                assert!(
                    window
                        .painted_quads()
                        .iter()
                        .any(|quad| quad.background.as_solid() == Some(rgb(0xff00ff).into())),
                    "the innermost content paints"
                );
            })
            .unwrap();
        }
    }

    const BELOW: u32 = 0xff00ff;
    const ABOVE: u32 = 0x00ff00;

    fn boxed(
        id: &str,
        style: gpui_kit::StyleRefinement,
        interactivity: Option<Box<wire::Interactivity>>,
        children: Vec<wire::Node>,
    ) -> wire::Node {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name(id.to_owned().into())),
            style: crate::render::test_style(style),
            interactivity,
            children,
        })
    }

    fn overlay(style: gpui_kit::StyleRefinement) -> wire::Node {
        let clicks = wire::Interactivity {
            on_click: Some(1),
            occlude: true,
            ..Default::default()
        };
        wire::Node::Deferred {
            priority: 16,
            content: Box::new(boxed(
                "overlay",
                style.bg(rgb(BELOW)),
                Some(Box::new(clicks)),
                Vec::new(),
            )),
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

    struct Desk {
        below: Entity<ViewTree>,
        log: Rc<RefCell<Vec<&'static str>>>,
    }
    impl Render for Desk {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
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
            div()
                .relative()
                .size(px(200.))
                .child(clipped(below.into_any_element(), 0))
                .child(clipped(above.into_any_element(), 1))
        }
    }

    #[gpui_kit::test]
    fn a_views_overlay_stays_under_the_pane_above_its_pane(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let root = boxed(
            "slot",
            gpui_kit::StyleRefinement::default().relative().size_full(),
            None,
            vec![overlay(
                gpui_kit::StyleRefinement::default().absolute().inset_0(),
            )],
        );
        let log = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(200.), px(200.)), {
            let log = log.clone();
            move |_, cx| Desk {
                below: cx.new(|_| ViewTree::new(root)),
                log,
            }
        });
        let below = window
            .root(cx)
            .unwrap()
            .read_with(cx, |desk, _| desk.below.clone());
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let (events, _subscription) = crate::render::tests::emitted(&below, &mut native);
        for pass in ["drawn", "replayed from the view's cache"] {
            native.update(|window, cx| {
                window.refresh();
                crate::render::draw_frame(window, cx);
                let (below, above) = (order(window, BELOW), order(window, ABOVE));
                assert!(
                    below < above,
                    "{pass}: the overlay {below} over the pane above {above}"
                );
            });
            native.simulate_click(point(px(75.), px(50.)), Default::default());
            native.simulate_click(point(px(25.), px(50.)), Default::default());
            let clicks = events
                .borrow_mut()
                .drain(..)
                .filter(|event| matches!(event, wire::Event::Click { handler: 1, .. }))
                .count();
            let heard: Vec<_> = log.borrow_mut().drain(..).collect();
            assert_eq!((heard, clicks), (vec!["above"], 1), "{pass}");
        }
    }

    struct Pane(Entity<ViewTree>);
    impl Render for Pane {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size(px(60.)).overflow_hidden().child(self.0.clone())
        }
    }

    #[gpui_kit::test]
    fn a_views_overlay_leaves_its_clip_at_its_place_and_stays_in_its_pane(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let inner = boxed(
            "inner",
            gpui_kit::StyleRefinement::default()
                .relative()
                .left(px(5.))
                .size(px(20.))
                .overflow_hidden(),
            None,
            vec![overlay(
                gpui_kit::StyleRefinement::default()
                    .absolute()
                    .left(px(10.))
                    .size(px(100.)),
            )],
        );
        let root = boxed(
            "page",
            gpui_kit::StyleRefinement::default().size_full(),
            None,
            vec![inner],
        );
        let window = cx.open_window(size(px(200.), px(200.)), move |_, cx| {
            Pane(cx.new(|_| ViewTree::new(root)))
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            let scale = window.scale_factor();
            let quad = window
                .painted_quads()
                .into_iter()
                .find(|quad| quad.background.as_solid() == Some(rgb(BELOW).into()))
                .expect("the overlay paints");
            assert_eq!(quad.bounds.origin.x.0, 15. * scale, "{:?}", quad.bounds);
            let mask = quad.content_mask.bounds;
            let right = (mask.origin.x + mask.size.width).0;
            assert!(right > 25. * scale, "kept in the inner clip: {mask:?}");
            assert!(right <= 60. * scale, "out of its pane: {mask:?}");
        });
    }

    #[test]
    fn a_caught_panic_leaves_no_deferral_inside() {
        let caught = std::panic::catch_unwind(|| inside(|| panic!("prepaint")));
        assert!(caught.is_err());
        assert!(!INSIDE.get(), "a caught panic left the thread inside");
    }
}
