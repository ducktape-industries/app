//! A view's layer: its deferred draws and its tooltips, drawn among the
//! window's deferred draws at a view priority and inside its pane.
//! GPUI's generic Deferred intentionally passes no mask; that behavior would
//! let view overlays draw above host chrome after the slot has been painted.
//!
//! [`Layer`] sets the pane's [`TooltipLayer`] around a view's tree: gpui
//! draws every tooltip registered inside it at [`VIEW_CEILING`], fitted
//! and clipped to the pane, and a view `Deferred` takes the same mask. A
//! masked deferred draw paints and takes clicks only inside its mask
//! (gpui-pre `window.rs`, `prepaint_deferred_draws`, `Frame::hit_test`).
//!
//! A view `Deferred` met inside another draws in place, so the depth
//! stays one whatever the view nests (gpui asserts it under ten).
use super::*;
use gpui_kit::TooltipLayer;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

type Scopes = Rc<RefCell<Option<Vec<super::pointer::ScopeKey>>>>;
type Mask = Rc<Cell<Option<gpui_kit::ContentMask<Pixels>>>>;

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
        deferred(self.node(content, window, cx), *priority).into_any_element()
    }
}

pub(super) fn deferred(child: AnyElement, priority: usize) -> SlotDeferred {
    let scopes = Scopes::default();
    SlotDeferred {
        child: Some(Inside(child, scopes.clone()).into_any_element()),
        priority: priority.min(VIEW_CEILING),
        scopes,
    }
}

pub(super) struct SlotDeferred {
    child: Option<AnyElement>,
    priority: usize,
    scopes: Scopes,
}

impl IntoElement for SlotDeferred {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for SlotDeferred {
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
        cx: &mut App,
    ) {
        if INSIDE.get() {
            self.child
                .as_mut()
                .expect("deferred child before prepaint")
                .prepaint(window, cx);
            return;
        }
        let child = self.child.take().expect("deferred child is drawn once");
        *self.scopes.borrow_mut() = super::pointer::stack(window, cx);
        let pane = window
            .tooltip_layer()
            .expect("a view deferral draws inside its view's layer")
            .mask;
        window.defer_draw(child, window.element_offset(), self.priority, Some(pane));
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
        if let Some(child) = &mut self.child {
            child.paint(window, cx);
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
        let layer = TooltipLayer {
            priority: VIEW_CEILING,
            mask: window.content_mask(),
        };
        window.with_tooltip_layer(Some(layer), |window| self.0.prepaint(window, cx));
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

    struct Clipped;
    impl Render for Clipped {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size(px(40.)).overflow_hidden().child(Layer(
                deferred(
                    div()
                        .absolute()
                        .left(px(30.))
                        .size(px(100.))
                        .bg(rgb(0xff00ff))
                        .into_any_element(),
                    usize::MAX,
                )
                .into_any_element(),
                None,
            ))
        }
    }

    #[gpui_kit::test]
    fn deferred_view_paint_keeps_slot_mask(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), |_, _| Clipped);
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

    #[test]
    fn a_caught_panic_leaves_no_deferral_inside() {
        let caught = std::panic::catch_unwind(|| inside(|| panic!("prepaint")));
        assert!(caught.is_err());
        assert!(!INSIDE.get(), "a caught panic left the thread inside");
    }
}
