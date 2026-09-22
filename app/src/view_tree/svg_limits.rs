use super::*;

const MAX_SVG_RASTER_BYTES: u64 = 64 << 20;
const SVG_BYTES_PER_PIXEL: u64 = 5; // tiny-skia RGBA plus GPUI's alpha mask.
const SMOOTH_SVG_SCALE: f64 = 2.0;

pub(super) fn svg_data_allowed(bytes: &[u8]) -> bool {
    !bytes.starts_with(&[0x1f, 0x8b])
}

pub(super) fn svg_raster_fits(width: f32, height: f32, device_scale: f32) -> bool {
    if !width.is_finite()
        || !height.is_finite()
        || !device_scale.is_finite()
        || width < 0.0
        || height < 0.0
        || device_scale <= 0.0
    {
        return false;
    }
    // Native Size scales from width while a guest controls the source aspect
    // ratio. The other side may reach GPUI's 8192px cap even in a short box.
    let raster_width = (f64::from(width) * f64::from(device_scale) * SMOOTH_SVG_SCALE).ceil();
    raster_width <= 8192.0
        && raster_width * 8192.0 * SVG_BYTES_PER_PIXEL as f64 <= MAX_SVG_RASTER_BYTES as f64
}

pub(super) fn guarded_svg_paint(child: impl IntoElement) -> SvgPaintGuard {
    SvgPaintGuard {
        child: child.into_any_element(),
    }
}

pub(super) struct SvgPaintGuard {
    child: AnyElement,
}

impl IntoElement for SvgPaintGuard {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SvgPaintGuard {
    type RequestLayoutState = ();
    type PrepaintState = bool;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
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
    ) -> bool {
        let allowed = svg_raster_fits(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            window.scale_factor(),
        );
        if allowed {
            self.child.prepaint(window, cx);
        }
        allowed
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        allowed: &mut bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if *allowed {
            self.child.paint(window, cx);
        } else {
            // A visible host-owned refusal replaces the expensive native paint.
            window.paint_quad(fill(bounds, rgb(0x7f1d1d)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn compressed_svg_data_is_refused_before_native_parsing() {
        assert!(!svg_data_allowed(&[0x1f, 0x8b, 0x08, 0x00]));
        assert!(svg_data_allowed(
            b"<svg xmlns='http://www.w3.org/2000/svg'/>"
        ));
        let node = wire::Node::Svg {
            id: None,
            source: wire::SvgSource::None,
            transformation: wire::SvgTransformation {
                scale: [1.0, 1.0],
                translate: [0.0, 0.0],
                rotate: 0.0,
            },
            label: None,
            style: Default::default(),
            interactivity: Default::default(),
        };
        let mut tree = ViewTree::new(node);
        tree.remember_vector(7, &[0x1f, 0x8b, 0x08, 0x00]);
        assert!(!tree.vectors.contains_key(&7));
    }

    #[test]
    fn raster_budget_counts_device_scale_smoothing_rgba_and_alpha() {
        assert!(svg_raster_fits(100.0, 100.0, 2.0));
        assert!(!svg_raster_fits(8192.0, 8192.0, 1.0));
        assert!(
            !svg_raster_fits(1000.0, 1.0, 1.0),
            "a narrow source aspect ratio can inflate the raster height"
        );
        assert!(!svg_raster_fits(f32::INFINITY, 1.0, 1.0));
    }

    struct GuardedPaint {
        width: f32,
        paints: Rc<Cell<usize>>,
    }

    impl Render for GuardedPaint {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let paints = self.paints.clone();
            guarded_svg_paint(
                canvas(|_, _, _| (), move |_, _, _, _| paints.set(paints.get() + 1))
                    .w(px(self.width))
                    .h(px(1.0)),
            )
        }
    }

    #[gpui_kit::test]
    fn oversized_guard_skips_child_paint_while_small_guard_paints(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let small_paints = Rc::new(Cell::new(0));
        let small = small_paints.clone();
        let window = cx.open_window(size(px(100.), px(1.)), move |_, _| GuardedPaint {
            width: 100.0,
            paints: small,
        });
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(small_paints.get(), 1);

        let large_paints = Rc::new(Cell::new(0));
        let large = large_paints.clone();
        let window = cx.open_window(size(px(3_700.), px(1.)), move |_, _| GuardedPaint {
            width: 3_700.0,
            paints: large,
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            assert!(!window.painted_quads().is_empty(), "refusal is visible");
        })
        .unwrap();
        assert_eq!(large_paints.get(), 0);
    }
}
