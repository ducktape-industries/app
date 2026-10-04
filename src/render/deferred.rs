//! A guest's layer: its deferred draws and its tooltips, drawn among the
//! window's deferred draws at a guest priority and inside its pane.
//! GPUI's generic Deferred intentionally passes no mask; that behavior would
//! let guest overlays draw above host chrome after the slot has been painted.
//!
//! [`Layer`] sets the pane's [`TooltipLayer`] around a view's tree: gpui
//! draws every tooltip registered inside it at [`GUEST_CEILING`], fitted
//! and clipped to the pane, and a guest `Deferred` takes the same mask. A
//! masked deferred draw paints and takes clicks only inside its mask
//! (gpui-pre `window.rs`, `prepaint_deferred_draws`, `Frame::hit_test`).
//!
//! A guest `Deferred` met inside another draws in place, so the depth
//! stays one whatever the guest nests (gpui asserts it under ten).
use super::*;
use gpui_kit::TooltipLayer;
use std::cell::Cell;

/// The host's band, over every window's rank and every priority a guest
/// asks for: each window draws at its rank (`PaneLayer`), and a guest's
/// draws inside its window's; the host's layers over the desk (its dialogs,
/// the bar's menus, the footer) defer at `HOST_BAND + n`, so no window and
/// no guest draw paints or takes a click above them.
pub(crate) const HOST_BAND: usize = 1000;
const _: () = assert!(crate::ui::layout::MAX_PANES <= HOST_BAND);

/// The highest priority a guest draw takes: a `Deferred` asks for at most
/// this, and a guest tooltip draws at it.
pub(super) const GUEST_CEILING: usize = 16;

thread_local! {
    /// A guest deferral is prepainting now: a `Deferred` met inside it
    /// draws in place.
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
    SlotDeferred {
        child: Some(Inside(child).into_any_element()),
        priority: priority.min(GUEST_CEILING),
    }
}

/// A guest `Deferred`: its child drawn after the window's other draws,
/// inside its pane (the [`Layer`]'s mask), or in place when met inside
/// another deferral.
pub(super) struct SlotDeferred {
    /// Held until prepaint, and on through paint when drawn in place.
    child: Option<AnyElement>,
    priority: usize,
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
        let pane = window
            .tooltip_layer()
            .expect("a guest deferral draws inside its view's layer")
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

/// A view's tree in its pane's layer: the tooltips registered and the
/// `Deferred`s met inside it draw at [`GUEST_CEILING`] at most, fitted and
/// clipped to the content mask the tree is drawn in (the pane's).
pub(super) struct Layer(pub(super) AnyElement);

impl IntoElement for Layer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Layer {
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
        let layer = TooltipLayer {
            priority: GUEST_CEILING,
            mask: window.content_mask(),
        };
        window.with_tooltip_layer(Some(layer), |window| self.0.prepaint(window, cx));
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

/// Runs `f` inside a guest deferral, then restores the outer state, on
/// unwind too: a panic caught mid-frame (the gpui test harness catches
/// one per test) leaves no later frame drawing in place.
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

/// A deferred guest child: prepaints as inside a deferral (`INSIDE`).
struct Inside(AnyElement);

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
        inside(|| self.0.prepaint(window, cx));
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
            ))
        }
    }

    #[gpui_kit::test]
    fn deferred_guest_paint_keeps_slot_mask(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), |_, _| Clipped);
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let quads: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background.as_solid() == Some(rgb(0xff00ff).into()))
                .collect();
            assert!(!quads.is_empty(), "the deferred guest child must paint");
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

    /// Eleven nested guest `Deferred`s, as the sanitizer passes them, draw
    /// without tripping gpui's deferred-depth assert, and the innermost
    /// paints: nested directly, and through list rows (drawn as the list
    /// prepaints, after the tree has rendered).
    #[gpui_kit::test]
    fn nested_guest_deferrals_draw_in_place(cx: &mut gpui_kit::TestAppContext) {
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
        // each level a list whose one row is the next level; the row, with
        // no id of its own, is filed under its index
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

    /// A panic caught inside a deferral leaves the thread outside it.
    #[test]
    fn a_caught_panic_leaves_no_deferral_inside() {
        let caught = std::panic::catch_unwind(|| inside(|| panic!("prepaint")));
        assert!(caught.is_err());
        assert!(!INSIDE.get(), "a caught panic left the thread inside");
    }
}
