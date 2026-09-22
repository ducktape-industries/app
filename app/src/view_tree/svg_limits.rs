use super::*;
use gpui_kit::{Global, WindowId};
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
};

const MAX_SVG_RASTER_BYTES: u64 = 64 << 20;
const MAX_WINDOW_SVG_RASTER_BYTES: u64 = 256 << 20;
const MAX_WINDOW_SVG_RASTERS: usize = 4_096;
const SVG_BYTES_PER_PIXEL: u64 = 5; // tiny-skia RGBA plus GPUI's alpha mask.
const SMOOTH_SVG_SCALE: f64 = 2.0;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum SvgPaintSource {
    Data(u64),
    Asset(SharedString),
}

impl SvgPaintSource {
    pub(super) fn data(bytes: &[u8]) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        Self::Data(hasher.finish())
    }

    pub(super) fn asset(path: impl Into<SharedString>) -> Self {
        Self::Asset(path.into())
    }
}

#[derive(PartialEq, Eq, Hash)]
struct SvgRasterKey {
    source: SvgPaintSource,
    width: i32,
    height: i32,
}

#[derive(Default)]
struct WindowSvgLedger {
    bytes: u64,
    rasters: HashSet<SvgRasterKey>,
}

struct SvgAdmissionLedgers {
    windows: HashMap<WindowId, WindowSvgLedger>,
    _cleanup: Subscription,
}

impl Global for SvgAdmissionLedgers {}

pub(super) fn svg_data_allowed(bytes: &[u8]) -> bool {
    !bytes.starts_with(&[0x1f, 0x8b])
}

pub(super) fn svg_raster_fits(width: f32, height: f32, device_scale: f32) -> bool {
    svg_raster_size_and_charge(width, height, device_scale).is_some()
}

fn svg_raster_size_and_charge(
    width: f32,
    height: f32,
    device_scale: f32,
) -> Option<(i32, i32, u64)> {
    if !width.is_finite()
        || !height.is_finite()
        || !device_scale.is_finite()
        || width < 0.0
        || height < 0.0
        || device_scale <= 0.0
    {
        return None;
    }
    // Native Size scales from width while a guest controls the source aspect
    // ratio. The other side may reach GPUI's 8192px cap even in a short box.
    let raster_width = (f64::from(width) * f64::from(device_scale) * SMOOTH_SVG_SCALE).ceil();
    let charge = raster_width * 8192.0 * SVG_BYTES_PER_PIXEL as f64;
    if raster_width <= 0.0 || raster_width > 8192.0 || charge > MAX_SVG_RASTER_BYTES as f64 {
        return None;
    }
    let raster_height = (f64::from(height) * f64::from(device_scale) * SMOOTH_SVG_SCALE).ceil();
    if raster_height <= 0.0 {
        return None;
    }
    Some((raster_width as i32, raster_height as i32, charge as u64))
}

fn admit_svg_raster(
    window_id: WindowId,
    source: SvgPaintSource,
    width: f32,
    height: f32,
    device_scale: f32,
    cx: &mut App,
) -> bool {
    let Some((width, height, charge)) = svg_raster_size_and_charge(width, height, device_scale)
    else {
        return false;
    };
    if !cx.has_global::<SvgAdmissionLedgers>() {
        let cleanup = cx.on_window_closed(|cx, closed| {
            if cx.has_global::<SvgAdmissionLedgers>() {
                cx.global_mut::<SvgAdmissionLedgers>()
                    .windows
                    .remove(&closed);
            }
        });
        cx.set_global(SvgAdmissionLedgers {
            windows: HashMap::new(),
            _cleanup: cleanup,
        });
    }
    let ledger = cx
        .global_mut::<SvgAdmissionLedgers>()
        .windows
        .entry(window_id)
        .or_default();
    let key = SvgRasterKey {
        source,
        width,
        height,
    };
    if ledger.rasters.contains(&key) {
        return true;
    }
    if ledger.rasters.len() >= MAX_WINDOW_SVG_RASTERS {
        return false;
    }
    let Some(total) = ledger.bytes.checked_add(charge) else {
        return false;
    };
    if total > MAX_WINDOW_SVG_RASTER_BYTES {
        return false;
    }
    ledger.bytes = total;
    ledger.rasters.insert(key);
    true
}

#[cfg(test)]
pub(super) fn svg_admitted_bytes(window_id: WindowId, cx: &App) -> u64 {
    cx.try_global::<SvgAdmissionLedgers>()
        .and_then(|ledgers| ledgers.windows.get(&window_id))
        .map_or(0, |ledger| ledger.bytes)
}

pub(super) fn guarded_svg_paint(source: SvgPaintSource, child: impl IntoElement) -> SvgPaintGuard {
    SvgPaintGuard {
        source,
        child: child.into_any_element(),
    }
}

pub(super) struct SvgPaintGuard {
    source: SvgPaintSource,
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
        let allowed = admit_svg_raster(
            window.window_handle().window_id(),
            self.source.clone(),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            window.scale_factor(),
            cx,
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
        assert!(!svg_raster_fits(0.0, 100.0, 1.0));
        assert!(!svg_raster_fits(100.0, 0.0, 1.0));
        assert!(!svg_raster_fits(8192.0, 8192.0, 1.0));
        assert!(
            !svg_raster_fits(1000.0, 1.0, 1.0),
            "a narrow source aspect ratio can inflate the raster height"
        );
        assert!(!svg_raster_fits(f32::INFINITY, 1.0, 1.0));
    }

    #[gpui_kit::test]
    fn window_aggregate_reuses_keys_and_refuses_new_rasters_at_budget(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let window_id = WindowId::from(1);
        let (_, _, charge) = svg_raster_size_and_charge(100.0, 1.0, 1.0).unwrap();
        cx.update(|cx| {
            assert!(admit_svg_raster(
                window_id,
                SvgPaintSource::Data(1),
                100.0,
                1.0,
                1.0,
                cx,
            ));
            assert!(admit_svg_raster(
                window_id,
                SvgPaintSource::Data(1),
                100.0,
                1.0,
                1.0,
                cx,
            ));
            assert_eq!(svg_admitted_bytes(window_id, cx), charge);

            // GPUI includes both requested dimensions in its native atlas key.
            assert!(admit_svg_raster(
                window_id,
                SvgPaintSource::Data(1),
                100.0,
                2.0,
                1.0,
                cx,
            ));
            assert_eq!(svg_admitted_bytes(window_id, cx), charge * 2);

            let mut source = 2;
            while admit_svg_raster(window_id, SvgPaintSource::Data(source), 100.0, 1.0, 1.0, cx) {
                source += 1;
            }
            assert!(source > 2, "the budget admitted some distinct sources");
            assert!(svg_admitted_bytes(window_id, cx) <= MAX_WINDOW_SVG_RASTER_BYTES);
            assert!(admit_svg_raster(
                window_id,
                SvgPaintSource::Data(1),
                100.0,
                1.0,
                1.0,
                cx,
            ));
        });
    }

    #[gpui_kit::test]
    fn window_aggregate_caps_keys_and_cleans_up_closed_windows(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(10.), px(10.)), |_, _| GuardedPaint {
            width: 1.0,
            paints: Rc::new(Cell::new(0)),
        });
        let window_id = window.window_id();
        cx.update(|cx| {
            if cx.has_global::<SvgAdmissionLedgers>() {
                cx.global_mut::<SvgAdmissionLedgers>()
                    .windows
                    .remove(&window_id);
            }
            for source in 0..MAX_WINDOW_SVG_RASTERS as u64 {
                assert!(admit_svg_raster(
                    window_id,
                    SvgPaintSource::Data(source),
                    0.01,
                    1.0,
                    1.0,
                    cx,
                ));
            }
            assert!(!admit_svg_raster(
                window_id,
                SvgPaintSource::Data(MAX_WINDOW_SVG_RASTERS as u64),
                0.01,
                1.0,
                1.0,
                cx,
            ));
        });
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(svg_admitted_bytes(window_id, cx), 0));
    }

    struct GuardedPaint {
        width: f32,
        paints: Rc<Cell<usize>>,
    }

    impl Render for GuardedPaint {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let paints = self.paints.clone();
            guarded_svg_paint(
                SvgPaintSource::data(b"guard-test"),
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
        small_paints.set(0);
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
