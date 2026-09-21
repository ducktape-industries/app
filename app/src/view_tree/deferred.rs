//! A deferred guest draw retains its slot's content mask.
//! GPUI's generic Deferred intentionally passes no mask; that behavior would
//! let guest overlays draw above host chrome after the slot has been painted.
use super::*;

pub(super) fn deferred(child: AnyElement, priority: usize) -> SlotDeferred {
    SlotDeferred {
        child: Some(child),
        priority: priority.min(16),
    }
}

pub(super) struct SlotDeferred {
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
        _cx: &mut App,
    ) {
        let child = self.child.take().expect("deferred child is drawn once");
        window.defer_draw(
            child,
            window.element_offset(),
            self.priority,
            Some(window.content_mask()),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Clipped;
    impl Render for Clipped {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size(px(40.)).overflow_hidden().child(deferred(
                div()
                    .absolute()
                    .left(px(30.))
                    .size(px(100.))
                    .bg(rgb(0xff00ff))
                    .into_any_element(),
                usize::MAX,
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
}
