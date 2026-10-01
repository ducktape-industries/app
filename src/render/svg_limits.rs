//! A per-window budget for rasterised guest SVGs. `guarded_svg_paint` wraps
//! an svg element; at prepaint each (source, raster size) key is admitted
//! once and charged to the window's ledger, and a key past the caps is not
//! painted (a red quad shows in its place). The ledger is for the window's
//! lifetime: nothing is released until the window closes, so the budget is
//! how many distinct rasters a window may ever ask for, not how many it
//! shows at once (gpui keeps an SVG's atlas tile for the window's life: it
//! has no way to drop one). The charge is the raster gpui makes: the box
//! width, and the height the SVG's own aspect ratio gives it, scaled down
//! to its 8192px cap (svg_renderer.rs `rasterize_tree`).
//! `svg_data_allowed` refuses gzip-compressed SVG before it reaches the
//! parser.
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
/// gpui's `SMOOTH_SVG_SCALE_FACTOR`: it rasterises SVGs at twice the device
/// scale.
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

/// Whether the bytes may reach the SVG parser: gzip (svgz, by its magic) is
/// refused, so a guest cannot hand the parser a small file that inflates.
pub(super) fn svg_data_allowed(bytes: &[u8]) -> bool {
    !bytes.starts_with(&[0x1f, 0x8b])
}

/// The raster key gpui uses for an SVG drawn in a `width` x `height` box,
/// and what that raster costs. `intrinsic` is the SVG's own size (usvg's,
/// as gpui parses it); `None`, an SVG gpui cannot parse, rasterises nothing
/// and costs nothing.
fn svg_raster_size_and_charge(
    width: f32,
    height: f32,
    device_scale: f32,
    intrinsic: Option<(f32, f32)>,
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
    let raster_width = (f64::from(width) * f64::from(device_scale) * SMOOTH_SVG_SCALE).ceil();
    if raster_width <= 0.0 || raster_width > 8192.0 {
        return None;
    }
    let raster_height = (f64::from(height) * f64::from(device_scale) * SMOOTH_SVG_SCALE).ceil();
    if raster_height <= 0.0 {
        return None;
    }
    // gpui's `SvgSize::Size`: the width is the box's, the height follows the
    // SVG's aspect ratio, and both scale down until neither passes 8192px.
    let charge = intrinsic.map_or(0, |(svg_width, svg_height)| {
        let scale = raster_width as f32 / svg_width;
        let (width, height) = (svg_width * scale, svg_height * scale);
        let fit = (8192.0 / width).min(8192.0 / height).min(1.0);
        u64::from((width * fit) as u32) * u64::from((height * fit) as u32) * SVG_BYTES_PER_PIXEL
    });
    if charge > MAX_SVG_RASTER_BYTES {
        return None;
    }
    Some((raster_width as i32, raster_height as i32, charge))
}

/// An SVG's own size as gpui's parser reads it, or `None` when it cannot
/// parse the SVG. An asset is read from the app's asset source, as gpui
/// reads it to draw.
fn svg_intrinsic_size(
    source: &SvgPaintSource,
    data: Option<&[u8]>,
    cx: &App,
) -> Option<(f32, f32)> {
    let asset;
    let bytes = match (source, data) {
        (_, Some(bytes)) => bytes,
        (SvgPaintSource::Asset(path), None) => {
            asset = cx.asset_source().load(path).ok()??;
            &*asset
        }
        (SvgPaintSource::Data(_), None) => return None,
    };
    let size = usvg::Tree::from_data(bytes, &usvg::Options::default())
        .ok()?
        .size();
    Some((size.width(), size.height()))
}

fn admit_svg_raster(
    window_id: WindowId,
    source: SvgPaintSource,
    data: Option<&[u8]>,
    width: f32,
    height: f32,
    device_scale: f32,
    cx: &mut App,
) -> bool {
    // the key gpui rasterises under, before any parse: a key already
    // admitted costs nothing more
    let Some((key_width, key_height, _)) =
        svg_raster_size_and_charge(width, height, device_scale, None)
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
        width: key_width,
        height: key_height,
    };
    if ledger.rasters.contains(&key) {
        return true;
    }
    if ledger.rasters.len() >= MAX_WINDOW_SVG_RASTERS {
        return false;
    }
    // ponytail: a refused key is parsed again each frame it shows; the
    // refusal is the window's last resort, so its cost is not cached
    let intrinsic = svg_intrinsic_size(&key.source, data, cx);
    let Some((_, _, charge)) = svg_raster_size_and_charge(width, height, device_scale, intrinsic)
    else {
        return false;
    };
    let ledger = cx
        .global_mut::<SvgAdmissionLedgers>()
        .windows
        .entry(window_id)
        .or_default();
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

/// `child` drawn only once its raster is admitted. `data` is the SVG's
/// bytes, `None` for an asset, which is read from the asset source.
pub(super) fn guarded_svg_paint(
    source: SvgPaintSource,
    data: Option<Arc<[u8]>>,
    child: impl IntoElement,
) -> SvgPaintGuard {
    SvgPaintGuard {
        source,
        data,
        child: child.into_any_element(),
    }
}

pub(super) struct SvgPaintGuard {
    source: SvgPaintSource,
    data: Option<Arc<[u8]>>,
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
            self.data.as_deref(),
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

    /// An SVG one unit wide and 100 tall: in a box 100 wide its raster
    /// reaches gpui's 8192px cap, the most a box that wide can cost.
    const TALL: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="100"><path d="M0 0h1v100H0z"/></svg>"#;

    fn svg_raster_fits(width: f32, height: f32, device_scale: f32, svg: (f32, f32)) -> bool {
        svg_raster_size_and_charge(width, height, device_scale, Some(svg)).is_some()
    }

    #[test]
    fn compressed_svg_data_is_refused_before_native_parsing() {
        assert!(!svg_data_allowed(&[0x1f, 0x8b, 0x08, 0x00]));
        assert!(svg_data_allowed(
            b"<svg xmlns='http://www.w3.org/2000/svg'/>"
        ));
    }

    #[test]
    fn raster_budget_counts_device_scale_smoothing_rgba_and_alpha() {
        assert!(svg_raster_fits(100.0, 100.0, 2.0, (1.0, 1.0)));
        assert!(!svg_raster_fits(0.0, 100.0, 1.0, (1.0, 1.0)));
        assert!(!svg_raster_fits(100.0, 0.0, 1.0, (1.0, 1.0)));
        assert!(!svg_raster_fits(8192.0, 8192.0, 1.0, (1.0, 1.0)));
        assert!(
            !svg_raster_fits(1000.0, 1.0, 1.0, (1.0, 4.0)),
            "a narrow source aspect ratio can inflate the raster height"
        );
        assert!(
            svg_raster_fits(1000.0, 1.0, 1.0, (1.0, 1.0)),
            "a square source in the same short box is charged its square"
        );
        assert!(!svg_raster_fits(f32::INFINITY, 1.0, 1.0, (1.0, 1.0)));
    }

    /// The charge is the raster gpui makes (`rasterize_tree`): the box
    /// width at twice the device scale, the SVG's own aspect, the 8192px cap.
    #[test]
    fn raster_charge_is_the_raster_gpui_makes() {
        let charge = |width, scale, svg| svg_raster_size_and_charge(width, 1.0, scale, svg);
        assert_eq!(
            charge(24.0, 2.0, Some((24.0, 24.0))),
            Some((96, 4, 96 * 96 * 5))
        );
        assert_eq!(
            charge(24.0, 1.0, Some((10.0, 5.0))),
            Some((48, 2, 48 * 24 * 5))
        );
        assert_eq!(
            charge(100.0, 1.0, Some((1.0, 100.0))),
            Some((200, 2, 81 * 8192 * 5)),
            "a tall SVG scales down until its height is 8192px"
        );
        assert_eq!(
            charge(24.0, 1.0, None),
            Some((48, 2, 0)),
            "an SVG gpui cannot parse rasterises nothing"
        );
    }

    #[gpui_kit::test]
    fn window_aggregate_reuses_keys_and_refuses_new_rasters_at_budget(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let window_id = WindowId::from(1);
        let (_, _, charge) =
            svg_raster_size_and_charge(100.0, 1.0, 1.0, Some((1.0, 100.0))).unwrap();
        cx.update(|cx| {
            assert!(admit_svg_raster(
                window_id,
                SvgPaintSource::Data(1),
                Some(TALL),
                100.0,
                1.0,
                1.0,
                cx,
            ));
            assert!(admit_svg_raster(
                window_id,
                SvgPaintSource::Data(1),
                Some(TALL),
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
                Some(TALL),
                100.0,
                2.0,
                1.0,
                cx,
            ));
            assert_eq!(svg_admitted_bytes(window_id, cx), charge * 2);

            let mut source = 2;
            while admit_svg_raster(
                window_id,
                SvgPaintSource::Data(source),
                Some(TALL),
                100.0,
                1.0,
                1.0,
                cx,
            ) {
                source += 1;
            }
            assert!(source > 2, "the budget admitted some distinct sources");
            assert!(svg_admitted_bytes(window_id, cx) <= MAX_WINDOW_SVG_RASTER_BYTES);
            assert!(admit_svg_raster(
                window_id,
                SvgPaintSource::Data(1),
                Some(TALL),
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
                    None,
                    0.01,
                    1.0,
                    1.0,
                    cx,
                ));
            }
            assert!(!admit_svg_raster(
                window_id,
                SvgPaintSource::Data(MAX_WINDOW_SVG_RASTERS as u64),
                None,
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
                None,
                canvas(|_, _, _| (), move |_, _, _, _| paints.set(paints.get() + 1))
                    .w(px(self.width))
                    .h(px(1.0)),
            )
        }
    }

    /// A toolbar's worth of icons, each its own 24px SVG.
    struct Icons {
        paints: Rc<Cell<usize>>,
    }

    impl Render for Icons {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_wrap()
                .size_full()
                .children((0..200).map(|index| {
                    let svg = format!(
                        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" id="icon-{index}"><path d="M0 0h24v24H0z"/></svg>"#
                    );
                    let paints = self.paints.clone();
                    guarded_svg_paint(
                        SvgPaintSource::data(svg.as_bytes()),
                        Some(Arc::from(svg.as_bytes())),
                        canvas(|_, _, _| (), move |_, _, _, _| paints.set(paints.get() + 1))
                            .size(px(24.)),
                    )
                }))
        }
    }

    /// The ledger charges each icon its own square raster, not the 8192px
    /// worst case: 200 distinct icons fit one window's budget and all paint.
    #[gpui_kit::test]
    fn two_hundred_distinct_icons_all_paint_in_one_window(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let paints = Rc::new(Cell::new(0));
        let counted = paints.clone();
        let window = cx.open_window(size(px(480.), px(240.)), move |_, _| Icons {
            paints: counted,
        });
        paints.set(0);
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            assert!(
                window.painted_quads().is_empty(),
                "an icon drew its refusal"
            );
            let (_, _, icon) =
                svg_raster_size_and_charge(24.0, 24.0, window.scale_factor(), Some((24.0, 24.0)))
                    .unwrap();
            assert_eq!(
                svg_admitted_bytes(window.window_handle().window_id(), cx),
                200 * icon,
                "each icon is charged its own raster"
            );
        })
        .unwrap();
        assert_eq!(paints.get(), 200, "every icon painted");
    }

    /// `count` distinct 24px icons drawn by gpui's own SVG element.
    struct RealIcons {
        count: usize,
    }

    impl Render for RealIcons {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_wrap()
                .size_full()
                .children((0..self.count).map(|index| {
                    let icon = format!(
                        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" id="icon-{index}"><circle cx="12" cy="12" r="10"/></svg>"#
                    );
                    guarded_svg_paint(
                        SvgPaintSource::data(icon.as_bytes()),
                        Some(Arc::from(icon.as_bytes())),
                        svg().data(icon.as_bytes()).size(px(24.)),
                    )
                }))
        }
    }

    /// The window ledger's cost, measured: one window drawing 4,096
    /// distinct 24px icons through gpui's rasteriser, the ledger's key cap.
    /// gpui's test atlas keeps no pixels: a GPU holds each icon's alpha
    /// mask, a fifth of its charge. Run alone: `cargo test
    /// a_window_of_icons_at_the_key_cap -- --ignored --exact --nocapture`.
    #[gpui_kit::test]
    #[ignore = "measurement: prints resident memory and the ledger, asserts nothing of them"]
    fn a_window_of_icons_at_the_key_cap(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let before = crate::render::tests::resident_mib();
        let window = cx.open_window(size(px(1536.), px(1536.)), |_, _| RealIcons {
            count: MAX_WINDOW_SVG_RASTERS,
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let after = crate::render::tests::resident_mib();
            println!(
                "{MAX_WINDOW_SVG_RASTERS} icons at scale {}: ledger {} B, {} refusal quads, resident +{:.1} MiB",
                window.scale_factor(),
                svg_admitted_bytes(window.window_handle().window_id(), cx),
                window.painted_quads().len(),
                after - before,
            );
        })
        .unwrap();
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
