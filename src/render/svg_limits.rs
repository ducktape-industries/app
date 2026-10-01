//! A per-window budget for rasterised guest SVGs. `guarded_svg_paint` wraps
//! an svg element; at prepaint its raster, under the key gpui paints it with
//! (`Window::svg_params`), is admitted to the window's ledger and charged
//! the raster gpui makes: the box width, and the height the SVG's own
//! aspect ratio gives it, scaled down to its 8192px cap (svg_renderer.rs
//! `rasterize_tree`). A raster that does not fit evicts the rasters drawn
//! least recently that no frame gpui may still show holds, and each leaves
//! the window's atlas with its charge (`Window::drop_svg`); one that does
//! not fit beside those is not painted (a red quad shows in its place). The
//! ledger goes when the window closes.
//! `svg_data_allowed` refuses gzip-compressed SVG before it reaches the
//! parser.
use super::*;
use gpui_kit::{DevicePixels, Global, RenderSvgParams, Svg, WindowId};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

const MAX_SVG_RASTER_BYTES: u64 = 64 << 20;
const MAX_WINDOW_SVG_RASTER_BYTES: u64 = 256 << 20;
const MAX_WINDOW_SVG_RASTERS: usize = 4_096;
const SVG_BYTES_PER_PIXEL: u64 = 5; // tiny-skia RGBA plus GPUI's alpha mask.

/// What a guard draws.
pub(super) enum SvgPaintSource {
    /// `svg().data(bytes)`.
    Data(Arc<[u8]>),
    /// `svg().path(path)`, read from the asset source.
    Asset(SharedString),
    /// `img(image)` of SVG bytes: gpui's image assets decode it, and its
    /// tile is the decoded image's.
    Image(Arc<Image>),
}

impl SvgPaintSource {
    /// The path gpui paints the SVG under (`Svg::data_path` for bytes).
    fn path(&self) -> SharedString {
        match self {
            Self::Data(bytes) => Svg::data_path(bytes),
            Self::Asset(path) => path.clone(),
            Self::Image(image) => Svg::data_path(image.bytes()),
        }
    }
}

/// A raster as gpui holds it in a window, and so how it leaves.
#[derive(Clone, PartialEq, Eq, Hash)]
enum SvgRaster {
    /// `paint_svg`'s alpha mask, under its own key.
    Svg(RenderSvgParams),
    /// The image gpui decodes, and its tile.
    Image(Arc<Image>),
}

struct HeldRaster {
    charge: u64,
    /// The ledger's tick when a guard last drew it.
    drawn: u64,
    /// The guards whose element state holds it (`SvgPin`).
    pins: usize,
}

#[derive(Default)]
struct WindowSvgLedger {
    bytes: u64,
    tick: u64,
    rasters: HashMap<SvgRaster, HeldRaster>,
}

impl WindowSvgLedger {
    /// Marks `raster` drawn now, pinned once more when `pin`; `false` when
    /// the ledger does not hold it.
    fn drew(&mut self, raster: &SvgRaster, pin: bool) -> bool {
        self.tick += 1;
        let Some(held) = self.rasters.get_mut(raster) else {
            return false;
        };
        held.drawn = self.tick;
        held.pins += usize::from(pin);
        true
    }

    fn fits(&self, charge: u64) -> bool {
        self.rasters.len() < MAX_WINDOW_SVG_RASTERS
            && self
                .bytes
                .checked_add(charge)
                .is_some_and(|total| total <= MAX_WINDOW_SVG_RASTER_BYTES)
    }

    /// Takes out the raster drawn least recently that no guard pins.
    fn evict(&mut self) -> Option<SvgRaster> {
        // ponytail: a scan per eviction, over at most `MAX_WINDOW_SVG_RASTERS`
        let raster = (self.rasters.iter())
            .filter(|(_, held)| held.pins == 0)
            .min_by_key(|(_, held)| held.drawn)
            .map(|(raster, _)| raster.clone())?;
        let held = self.rasters.remove(&raster)?;
        self.bytes -= held.charge;
        Some(raster)
    }
}

/// A guard's hold on the raster it drew. It lives in the guard's element
/// state, which gpui keeps while the frame being drawn or the last frame
/// drawn has it: a cached view's frame replays its sprites without drawing
/// the guard again, so a pinned raster's tile must stay in the atlas.
struct SvgPin {
    ledger: Rc<RefCell<WindowSvgLedger>>,
    raster: SvgRaster,
}

impl Drop for SvgPin {
    fn drop(&mut self) {
        if let Some(held) = self.ledger.borrow_mut().rasters.get_mut(&self.raster) {
            held.pins -= 1;
        }
    }
}

struct SvgAdmissionLedgers {
    windows: HashMap<WindowId, Rc<RefCell<WindowSvgLedger>>>,
    _cleanup: Subscription,
}

impl Global for SvgAdmissionLedgers {}

/// Whether the bytes may reach the SVG parser: gzip (svgz, by its magic) is
/// refused, so a guest cannot hand the parser a small file that inflates.
pub(super) fn svg_data_allowed(bytes: &[u8]) -> bool {
    !bytes.starts_with(&[0x1f, 0x8b])
}

/// What a raster of `size` costs. `intrinsic` is the SVG's own size
/// (usvg's, as gpui parses it); `None`, an SVG gpui cannot parse,
/// rasterises nothing and costs nothing.
fn svg_raster_charge(size: Size<DevicePixels>, intrinsic: Option<(f32, f32)>) -> Option<u64> {
    let (raster_width, raster_height) = (size.width.0, size.height.0);
    if raster_width <= 0 || raster_width > 8192 || raster_height <= 0 {
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
    (charge <= MAX_SVG_RASTER_BYTES).then_some(charge)
}

/// An SVG's own size as gpui's parser reads it, or `None` when it cannot
/// parse the SVG. An asset is read from the app's asset source, as gpui
/// reads it to draw.
fn svg_intrinsic_size(source: &SvgPaintSource, cx: &App) -> Option<(f32, f32)> {
    let asset;
    let bytes = match source {
        SvgPaintSource::Data(bytes) => bytes,
        SvgPaintSource::Image(image) => image.bytes(),
        SvgPaintSource::Asset(path) => {
            asset = cx.asset_source().load(path).ok()??;
            &*asset
        }
    };
    let size = usvg::Tree::from_data(bytes, &usvg::Options::default())
        .ok()?
        .size();
    Some((size.width(), size.height()))
}

/// The window's ledger, made on first use.
fn window_ledger(window_id: WindowId, cx: &mut App) -> Rc<RefCell<WindowSvgLedger>> {
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
    cx.global_mut::<SvgAdmissionLedgers>()
        .windows
        .entry(window_id)
        .or_default()
        .clone()
}

/// Admits `raster`, a raster of `size` drawn from `source`, to `ledger`,
/// evicting what it must; the pin holds it, `None` when it does not fit.
fn admit_svg_raster(
    ledger: &Rc<RefCell<WindowSvgLedger>>,
    raster: SvgRaster,
    size: Size<DevicePixels>,
    source: &SvgPaintSource,
    window: &mut Window,
    cx: &mut App,
) -> Option<SvgPin> {
    // a raster already held costs nothing more
    if !ledger.borrow_mut().drew(&raster, true) {
        // ponytail: a refused key is parsed again each frame it shows; the
        // refusal is the window's last resort, so its cost is not cached
        let charge = svg_raster_charge(size, svg_intrinsic_size(source, cx))?;
        loop {
            let evicted = {
                let mut ledger = ledger.borrow_mut();
                if ledger.fits(charge) {
                    ledger.bytes += charge;
                    let drawn = ledger.tick;
                    ledger.rasters.insert(
                        raster.clone(),
                        HeldRaster {
                            charge,
                            drawn,
                            pins: 1,
                        },
                    );
                    break;
                }
                ledger.evict()?
            };
            drop_raster(evicted, window, cx);
        }
    }
    Some(SvgPin {
        ledger: ledger.clone(),
        raster,
    })
}

/// Takes an evicted raster out of the window's atlas. A decoded image
/// leaves gpui's image assets too, unless another window's ledger holds it.
fn drop_raster(raster: SvgRaster, window: &mut Window, cx: &mut App) {
    match raster {
        SvgRaster::Svg(params) => window.drop_svg(params),
        SvgRaster::Image(image) => {
            if let Some(decoded) = image.clone().get_render_image(window, cx) {
                window.drop_image(decoded).ok();
            }
            let raster = SvgRaster::Image(image.clone());
            let held = cx
                .global::<SvgAdmissionLedgers>()
                .windows
                .values()
                .any(|ledger| ledger.borrow().rasters.contains_key(&raster));
            if !held {
                image.remove_asset(cx);
            }
        }
    }
}

#[cfg(test)]
pub(super) fn svg_admitted_bytes(window_id: WindowId, cx: &App) -> u64 {
    cx.try_global::<SvgAdmissionLedgers>()
        .and_then(|ledgers| ledgers.windows.get(&window_id))
        .map_or(0, |ledger| ledger.borrow().bytes)
}

/// `child` drawn only once its raster is admitted.
pub(super) fn guarded_svg_paint(source: SvgPaintSource, child: impl IntoElement) -> SvgPaintGuard {
    SvgPaintGuard {
        path: source.path(),
        source,
        child: child.into_any_element(),
    }
}

pub(super) struct SvgPaintGuard {
    source: SvgPaintSource,
    /// `source.path()`, which also names the guard, so its element state
    /// (`SvgPin`) is its own.
    path: SharedString,
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
        Some(host_id(self.path.clone()))
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
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        // the child is laid out on the guard's own layout, so these are the
        // bounds it paints with
        let params = window.svg_params(bounds, self.path.clone());
        let size = params.size;
        let raster = match &self.source {
            SvgPaintSource::Image(image) => SvgRaster::Image(image.clone()),
            SvgPaintSource::Data(_) | SvgPaintSource::Asset(_) => SvgRaster::Svg(params),
        };
        let ledger = window_ledger(window.window_handle().window_id(), cx);
        let source = &self.source;
        let allowed = window.with_element_state(
            global_id.expect("the guard has an id"),
            |pin: Option<Option<SvgPin>>, window| {
                let pin = match pin.flatten() {
                    Some(pin) if pin.raster == raster => {
                        ledger.borrow_mut().drew(&raster, false);
                        Some(pin)
                    }
                    // the guard's last raster is no longer drawn: its pin
                    // goes before the new one is admitted
                    other => {
                        drop(other);
                        admit_svg_raster(&ledger, raster, size, source, window, cx)
                    }
                };
                (pin.is_some(), pin)
            },
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
    use gpui_kit::{AnyView, StyleRefinement};
    use std::{cell::Cell, collections::HashSet};

    /// An SVG one unit wide and 100 tall: in a box 100 wide its raster
    /// reaches gpui's 8192px cap, the most a box that wide can cost.
    const TALL: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="100"><path d="M0 0h1v100H0z"/></svg>"#;

    fn svg_raster_fits(width: i32, height: i32, svg: (f32, f32)) -> bool {
        svg_raster_charge(size(DevicePixels(width), DevicePixels(height)), Some(svg)).is_some()
    }

    #[test]
    fn compressed_svg_data_is_refused_before_native_parsing() {
        assert!(!svg_data_allowed(&[0x1f, 0x8b, 0x08, 0x00]));
        assert!(svg_data_allowed(
            b"<svg xmlns='http://www.w3.org/2000/svg'/>"
        ));
    }

    /// Sizes are gpui's raster key (`Window::svg_params`): the box at twice
    /// the device scale.
    #[test]
    fn raster_budget_counts_device_scale_smoothing_rgba_and_alpha() {
        assert!(svg_raster_fits(400, 400, (1.0, 1.0)));
        assert!(!svg_raster_fits(0, 200, (1.0, 1.0)));
        assert!(!svg_raster_fits(200, 0, (1.0, 1.0)));
        assert!(!svg_raster_fits(16384, 16384, (1.0, 1.0)));
        assert!(
            !svg_raster_fits(2000, 2, (1.0, 4.0)),
            "a narrow source aspect ratio can inflate the raster height"
        );
        assert!(
            svg_raster_fits(2000, 2, (1.0, 1.0)),
            "a square source in the same short box is charged its square"
        );
    }

    /// The charge is the raster gpui makes (`rasterize_tree`): the key's
    /// width, the SVG's own aspect, the 8192px cap.
    #[test]
    fn raster_charge_is_the_raster_gpui_makes() {
        let charge =
            |width, svg| svg_raster_charge(size(DevicePixels(width), DevicePixels(2)), svg);
        assert_eq!(charge(96, Some((24.0, 24.0))), Some(96 * 96 * 5));
        assert_eq!(charge(48, Some((10.0, 5.0))), Some(48 * 24 * 5));
        assert_eq!(
            charge(200, Some((1.0, 100.0))),
            Some(81 * 8192 * 5),
            "a tall SVG scales down until its height is 8192px"
        );
        assert_eq!(
            charge(48, None),
            Some(0),
            "an SVG gpui cannot parse rasterises nothing"
        );
    }

    /// Admits `source` under the key `name` at `width` x `height` raster
    /// pixels, as a guard does.
    fn admit_named(
        source: &SvgPaintSource,
        name: &str,
        width: i32,
        height: i32,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<SvgPin> {
        let size = size(DevicePixels(width), DevicePixels(height));
        let raster = SvgRaster::Svg(RenderSvgParams {
            path: SharedString::from(name.to_owned()),
            size,
        });
        let ledger = window_ledger(window.window_handle().window_id(), cx);
        admit_svg_raster(&ledger, raster, size, source, window, cx)
    }

    /// Rasters still pinned are never evicted: past the caps a new one is
    /// refused.
    #[gpui_kit::test]
    fn window_aggregate_reuses_keys_and_refuses_new_rasters_at_budget(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(10.), px(10.)), |_, _| GuardedPaint {
            width: 1.0,
            paints: Rc::new(Cell::new(0)),
        });
        let tall = SvgPaintSource::Data(Arc::from(TALL));
        let charge =
            svg_raster_charge(size(DevicePixels(200), DevicePixels(2)), Some((1.0, 100.0)))
                .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            let window_id = window.window_handle().window_id();
            let mut pins = vec![
                admit_named(&tall, "1", 200, 2, window, cx).unwrap(),
                admit_named(&tall, "1", 200, 2, window, cx).unwrap(),
            ];
            assert_eq!(svg_admitted_bytes(window_id, cx), charge);

            // GPUI includes both requested dimensions in its native atlas key.
            pins.push(admit_named(&tall, "1", 200, 4, window, cx).unwrap());
            assert_eq!(svg_admitted_bytes(window_id, cx), charge * 2);

            let mut source = 2;
            while let Some(pin) = admit_named(&tall, &source.to_string(), 200, 2, window, cx) {
                pins.push(pin);
                source += 1;
            }
            assert!(source > 2, "the budget admitted some distinct sources");
            assert!(svg_admitted_bytes(window_id, cx) <= MAX_WINDOW_SVG_RASTER_BYTES);
            assert!(admit_named(&tall, "1", 200, 2, window, cx).is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn window_aggregate_caps_keys_and_cleans_up_closed_windows(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(10.), px(10.)), |_, _| GuardedPaint {
            width: 1.0,
            paints: Rc::new(Cell::new(0)),
        });
        let window_id = window.window_id();
        cx.update_window(window.into(), |_, window, cx| {
            cx.global_mut::<SvgAdmissionLedgers>()
                .windows
                .remove(&window_id);
            let blank = SvgPaintSource::Data(Arc::from(&b"blank"[..]));
            let pins: Vec<_> = (0..MAX_WINDOW_SVG_RASTERS)
                .map(|source| admit_named(&blank, &source.to_string(), 1, 2, window, cx).unwrap())
                .collect();
            assert!(
                admit_named(
                    &blank,
                    &MAX_WINDOW_SVG_RASTERS.to_string(),
                    1,
                    2,
                    window,
                    cx
                )
                .is_none()
            );
            drop(pins);
        })
        .unwrap();
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
                SvgPaintSource::Data(Arc::from(&b"guard-test"[..])),
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
                        SvgPaintSource::Data(Arc::from(svg.as_bytes())),
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
            let icon_box = Bounds::new(point(px(0.), px(0.)), size(px(24.), px(24.)));
            let icon = svg_raster_charge(
                window.svg_params(icon_box, SharedString::default()).size,
                Some((24.0, 24.0)),
            )
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
    #[cfg(target_os = "linux")]
    struct RealIcons {
        count: usize,
    }

    #[cfg(target_os = "linux")]
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
                        SvgPaintSource::Data(Arc::from(icon.as_bytes())),
                        svg()
                            .data(icon.as_bytes())
                            .size(px(24.))
                            .text_color(gpui_kit::black()),
                    )
                }))
        }
    }

    /// The window ledger's cost, measured: one window drawing 4,096
    /// distinct 24px icons through gpui's rasteriser, the ledger's key cap,
    /// into wgpu's atlas (`rendering_app`). With no GPU, Mesa's software
    /// Vulkan keeps the atlas in this process, so resident memory counts
    /// it. The window is drawn and read back empty first, so the readback
    /// target is not counted. Run alone: `cargo test
    /// render::svg_limits::tests::a_window_of_icons_at_the_key_cap --
    /// --ignored --exact --nocapture`.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "measurement: prints resident memory and the ledger, asserts nothing of them"]
    fn a_window_of_icons_at_the_key_cap() {
        let mut cx = crate::render::tests::picture_pixels::rendering_app();
        let window = cx
            .open_window(size(px(1536.), px(1536.)), |_, cx| {
                cx.new(|_| RealIcons { count: 0 })
            })
            .unwrap();
        let drawn = |cx: &mut gpui_kit::HeadlessAppContext| {
            let (scale, ledger, refusals) = cx
                .update_window(window.into(), |_, window, cx| {
                    window.draw(cx).clear(cx);
                    (
                        window.scale_factor(),
                        svg_admitted_bytes(window.window_handle().window_id(), cx),
                        window.painted_quads().len(),
                    )
                })
                .unwrap();
            // the renderer uploads the frame's tiles when it draws it
            cx.capture_screenshot(window.into()).unwrap();
            (scale, ledger, refusals)
        };
        drawn(&mut cx);
        let before = crate::render::tests::resident_mib();
        cx.update(|cx| {
            window.update(cx, |icons, _, cx| {
                icons.count = MAX_WINDOW_SVG_RASTERS;
                cx.notify();
            })
        })
        .unwrap();
        let (scale, ledger, refusals) = drawn(&mut cx);
        let after = crate::render::tests::resident_mib();
        println!(
            "{MAX_WINDOW_SVG_RASTERS} icons at scale {scale}: ledger {ledger} B, {refusals} refusal quads, resident +{:.1} MiB",
            after - before,
        );
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

    /// The SVG rasters `window_id`'s ledger holds.
    fn svg_admitted(window_id: WindowId, cx: &App) -> HashSet<RenderSvgParams> {
        let ledger = cx.global::<SvgAdmissionLedgers>().windows[&window_id].borrow();
        (ledger.rasters.keys())
            .filter_map(|raster| match raster {
                SvgRaster::Svg(params) => Some(params.clone()),
                SvgRaster::Image(_) => None,
            })
            .collect()
    }

    fn distinct_icon(index: usize) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" id="icon-{index}"><circle cx="12" cy="12" r="10"/></svg>"#
        )
    }

    /// Icons `first..first + count`, each its own SVG, `side` px square,
    /// painted by gpui's svg element.
    struct Churn {
        first: usize,
        count: usize,
        side: f32,
        renders: Rc<Cell<usize>>,
    }

    impl Churn {
        fn new(first: usize, count: usize) -> Self {
            Self {
                first,
                count,
                side: 24.,
                renders: Rc::new(Cell::new(0)),
            }
        }
    }

    impl Render for Churn {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            let side = self.side;
            div().flex().flex_wrap().size_full().children(
                (self.first..self.first + self.count).map(move |index| {
                    let icon = distinct_icon(index);
                    guarded_svg_paint(
                        SvgPaintSource::Data(Arc::from(icon.as_bytes())),
                        svg()
                            .data(icon.as_bytes())
                            .text_color(rgb(0xffffff))
                            .size(px(side)),
                    )
                }),
            )
        }
    }

    /// Asserts the frame drew no refusal, the ledger is within its caps,
    /// every raster it holds is in the atlas, and every raster it once held
    /// and let go has left it. Returns how many it let go.
    fn assert_ledger_and_atlas_agree(
        seen: &mut HashSet<RenderSvgParams>,
        frame: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> usize {
        let window_id = window.window_handle().window_id();
        assert!(window.painted_quads().is_empty(), "{frame} drew a refusal");
        let held = svg_admitted(window_id, cx);
        assert!(held.len() <= MAX_WINDOW_SVG_RASTERS);
        assert!(svg_admitted_bytes(window_id, cx) <= MAX_WINDOW_SVG_RASTER_BYTES);
        for params in &held {
            assert!(
                window.has_svg_atlas_entry(params),
                "{frame}: a held raster is in the atlas"
            );
        }
        seen.extend(held.iter().cloned());
        let evicted: Vec<_> = seen.difference(&held).collect();
        for params in &evicted {
            assert!(
                !window.has_svg_atlas_entry(params),
                "{frame}: an evicted raster's tile left the atlas"
            );
        }
        evicted.len()
    }

    /// A window that draws more distinct icons over time than the ledger's
    /// key cap, a few hundred at a time and then through a resize sweep,
    /// never refuses one: the rasters no frame shows any more are evicted,
    /// and their tiles leave the atlas with them.
    #[gpui_kit::test]
    fn icons_drawn_over_time_past_the_key_cap_never_refuse(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1200.), px(1200.)), |_, _| Churn::new(0, 250));
        let mut seen = HashSet::new();
        for batch in 0..20 {
            window
                .update(cx, |churn, _, cx| {
                    churn.first = batch * 250;
                    cx.notify();
                })
                .unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                assert_ledger_and_atlas_agree(&mut seen, &format!("batch {batch}"), window, cx);
            })
            .unwrap();
        }
        assert_eq!(seen.len(), 5_000, "5,000 distinct icons were drawn");
        for step in 1..=20 {
            window
                .update(cx, |churn, _, cx| {
                    churn.side = 24. + step as f32;
                    cx.notify();
                })
                .unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                assert_ledger_and_atlas_agree(&mut seen, &format!("resize {step}"), window, cx);
            })
            .unwrap();
        }
        assert_eq!(seen.len(), 10_000, "and 5,000 more sizes of the last 250");
        cx.update_window(window.into(), |_, window, cx| {
            let evicted = assert_ledger_and_atlas_agree(&mut seen, "the end", window, cx);
            assert!(evicted >= 10_000 - MAX_WINDOW_SVG_RASTERS);
        })
        .unwrap();
    }

    /// Two views, as a desk's panes: the first cached and left alone, so
    /// gpui replays its frame (and its sprites' tiles) without drawing it.
    struct TwoPanes {
        still: Entity<Churn>,
        busy: Entity<Churn>,
    }

    impl Render for TwoPanes {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(
                    div().h(px(100.)).child(
                        AnyView::from(self.still.clone())
                            .cached(StyleRefinement::default().size_full()),
                    ),
                )
                .child(div().flex_1().child(self.busy.clone()))
        }
    }

    /// While one view churns past the key cap, a cached view that gpui
    /// replays keeps its rasters: they are its frame's, though no guard of
    /// it has drawn since.
    #[gpui_kit::test]
    fn a_cached_views_icons_stay_while_another_view_churns(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let still_renders = Rc::new(Cell::new(0));
        let (mut still_entity, mut busy_entity) = (None, None);
        let window = cx.open_window(size(px(1200.), px(1200.)), |_, cx| {
            let still = cx.new(|_| Churn {
                renders: still_renders.clone(),
                ..Churn::new(100_000, 40)
            });
            let busy = cx.new(|_| Churn::new(0, 250));
            still_entity = Some(still.clone());
            busy_entity = Some(busy.clone());
            TwoPanes { still, busy }
        });
        let (_still, busy) = (still_entity.unwrap(), busy_entity.unwrap());
        let mut still_keys = HashSet::new();
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let still_paths: HashSet<_> = (100_000..100_040)
                .map(|index| Svg::data_path(distinct_icon(index).as_bytes()))
                .collect();
            still_keys = svg_admitted(window.window_handle().window_id(), cx);
            still_keys.retain(|params| still_paths.contains(&params.path));
        })
        .unwrap();
        assert_eq!(still_keys.len(), 40);
        let renders_before = still_renders.get();
        let mut seen = HashSet::new();
        for batch in 0..20 {
            cx.update(|cx| {
                busy.update(cx, |busy, cx| {
                    busy.first = batch * 250;
                    cx.notify();
                })
            });
            cx.update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                assert_ledger_and_atlas_agree(&mut seen, &format!("batch {batch}"), window, cx);
                for params in &still_keys {
                    assert!(
                        window.has_svg_atlas_entry(params),
                        "batch {batch}: the cached view's raster stayed in the atlas"
                    );
                }
            })
            .unwrap();
        }
        assert_eq!(
            still_renders.get(),
            renders_before,
            "gpui replayed the cached view without drawing it"
        );
        assert!(
            seen.len() > MAX_WINDOW_SVG_RASTERS,
            "the busy view went past the cap"
        );
    }

    /// Icons whose guards are gone stay pinned through the next frame (gpui
    /// keeps their element state until it finishes): a view that swaps
    /// more than half the key cap of icons for new ones refuses the excess
    /// on that frame, and draws them all on the next.
    #[gpui_kit::test]
    fn icons_swapped_wholesale_wait_one_frame_for_the_last_ones_to_go(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let count = MAX_WINDOW_SVG_RASTERS / 2 + 50;
        let window = cx.open_window(size(px(1200.), px(1200.)), move |_, _| Churn {
            side: 1.,
            ..Churn::new(0, count)
        });
        window
            .update(cx, |churn, _, cx| {
                churn.first = count;
                cx.notify();
            })
            .unwrap();
        // the update drew the frame that swapped them
        cx.update_window(window.into(), |_, window, cx| {
            assert_eq!(
                window.painted_quads().len(),
                2 * count - MAX_WINDOW_SVG_RASTERS,
                "the frame that swapped them refused what did not fit beside the last frame's"
            );
            window.draw(cx).clear(cx);
            assert!(
                window.painted_quads().is_empty(),
                "the next frame drew them all"
            );
        })
        .unwrap();
    }

    /// A frame whose own icons pass the key cap evicts none of them: the
    /// one past the cap draws its refusal.
    #[gpui_kit::test]
    fn a_frame_alone_over_the_key_cap_still_refuses(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1200.), px(1200.)), |_, _| Churn {
            side: 1.,
            ..Churn::new(0, MAX_WINDOW_SVG_RASTERS + 1)
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let held = svg_admitted(window.window_handle().window_id(), cx);
            assert_eq!(held.len(), MAX_WINDOW_SVG_RASTERS);
            assert_eq!(
                window.painted_quads().len(),
                1,
                "the icon past the cap drew its refusal"
            );
            for params in &held {
                assert!(
                    window.has_svg_atlas_entry(params),
                    "the frame's own rasters stayed"
                );
            }
        })
        .unwrap();
    }

    /// An SVG canvas's picture as `svg_canvas` draws it: gpui's image
    /// assets decode it.
    struct CanvasPicture {
        image: Option<Arc<Image>>,
    }

    impl Render for CanvasPicture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children(self.image.clone().map(|image| {
                guarded_svg_paint(
                    SvgPaintSource::Image(image.clone()),
                    img(image).size(px(24.)),
                )
            }))
        }
    }

    /// A canvas picture no frame draws any more is evicted like an icon:
    /// its tile leaves the atlas and its decode leaves gpui's image assets.
    #[gpui_kit::test]
    fn an_evicted_canvas_picture_leaves_the_atlas_and_the_image_assets(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let image = Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            distinct_icon(0).into_bytes(),
        ));
        let drawn = image.clone();
        let window = cx.open_window(size(px(100.), px(100.)), move |_, _| CanvasPicture {
            image: Some(drawn),
        });
        cx.run_until_parked();
        let decoded = cx
            .update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                let decoded = image.clone().get_render_image(window, cx).unwrap();
                assert!(window.has_image_atlas_entry(&decoded));
                decoded
            })
            .unwrap();
        window
            .update(cx, |picture, _, cx| {
                picture.image = None;
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let blank = SvgPaintSource::Data(Arc::from(&b"blank"[..]));
            let pins: Vec<_> = (0..MAX_WINDOW_SVG_RASTERS)
                .map(|source| admit_named(&blank, &source.to_string(), 1, 2, window, cx).unwrap())
                .collect();
            assert!(
                !window.has_image_atlas_entry(&decoded),
                "the evicted picture's tile left the atlas"
            );
            assert!(
                !image.is_asset_cached(cx),
                "the evicted picture left gpui's image assets"
            );
            drop(pins);
        })
        .unwrap();
    }
}
