//! A per-window budget for view SVG rasters. The host rasterises a view's glyph itself (resvg), in
//! the colour it paints in, and an SVG canvas's picture through gpui's image assets; each raster is
//! charged its frame and atlas tile, and those no frame still shows leave with `drop_image`.
use super::*;
use gpui_kit::{Corners, Global, RenderImage, Rgba, SMOOTH_SVG_SCALE_FACTOR, WindowId, relative};
use resvg::tiny_skia::{Pixmap, Transform};
use std::hash::{BuildHasher, RandomState};
use std::sync::LazyLock;
use std::{cell::RefCell, collections::HashMap, rc::Rc};

const ENTRY_BYTES: u64 = std::mem::size_of::<(Raster, HeldRaster)>() as u64;
const BYTES_PER_PIXEL: u64 = 8;
const MOST_SIDE: f32 = 8192.;

static SOURCE_KEYS: LazyLock<RandomState> = LazyLock::new(RandomState::new);

pub(super) enum GlyphSource {
    Data(Arc<[u8]>),
    Asset(SharedString),
}

impl GlyphSource {
    fn key(&self) -> u64 {
        match self {
            Self::Data(bytes) => SOURCE_KEYS.hash_one((0_u8, &**bytes)),
            Self::Asset(path) => SOURCE_KEYS.hash_one((1_u8, &**path)),
        }
    }

    fn tree(&self, cx: &App) -> Option<usvg::Tree> {
        let asset;
        let bytes = match self {
            Self::Data(bytes) => &**bytes,
            Self::Asset(path) => {
                asset = cx.asset_source().load(path).ok()??;
                &*asset
            }
        };
        super::svg_check::parse(bytes).ok()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    source: u64,
    width: u32,
    scale: u32,
    tint: u32,
    turn: [u32; 5],
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Raster {
    Glyph(GlyphKey),
    Image(Arc<Image>),
}

struct Placed {
    image: Arc<RenderImage>,
    origin: Point<f32>,
    extent: Size<f32>,
}

struct HeldRaster {
    charge: u64,
    drawn: u64,
    pins: usize,
    placed: Option<Placed>,
}

#[derive(Default)]
struct WindowSvgLedger {
    bytes: u64,
    tick: u64,
    rasters: HashMap<Raster, HeldRaster>,
}

impl WindowSvgLedger {
    fn drew(&mut self, raster: &Raster, pin: bool) -> bool {
        self.tick += 1;
        let Some(held) = self.rasters.get_mut(raster) else {
            return false;
        };
        held.drawn = self.tick;
        held.pins += usize::from(pin);
        true
    }

    fn fits(&self, charge: u64, budget: u64) -> bool {
        let entries = (self.rasters.len() as u64 + 1).saturating_mul(ENTRY_BYTES);
        (self.bytes)
            .checked_add(charge)
            .and_then(|total| total.checked_add(entries))
            .is_some_and(|total| total <= budget)
    }

    fn evict(&mut self) -> Option<(Raster, HeldRaster)> {
        let raster = (self.rasters.iter())
            .filter(|(_, held)| held.pins == 0)
            .min_by_key(|(_, held)| held.drawn)
            .map(|(raster, _)| raster.clone())?;
        let held = self.rasters.remove(&raster)?;
        self.bytes -= held.charge;
        Some((raster, held))
    }
}

struct SvgPin {
    ledger: Rc<RefCell<WindowSvgLedger>>,
    raster: Raster,
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
    budget: u64,
    _cleanup: Subscription,
}

impl Global for SvgAdmissionLedgers {}

fn fitted(width: f32, height: f32) -> Option<(f32, (u32, u32))> {
    let fit = (MOST_SIDE / width).min(MOST_SIDE / height).min(1.);
    let pixels = ((width * fit) as u32, (height * fit) as u32);
    (pixels.0 > 0 && pixels.1 > 0).then_some((fit, pixels))
}

fn pixels_charge((width, height): (u32, u32)) -> u64 {
    u64::from(width) * u64::from(height) * BYTES_PER_PIXEL
}

fn image_charge(width: f32, image: &Image) -> u64 {
    let Ok(tree) = super::svg_check::parse(image.bytes()) else {
        return 0;
    };
    let svg = tree.size();
    fitted(width, svg.height() * width / svg.width()).map_or(0, |(_, pixels)| pixels_charge(pixels))
}

struct Plan {
    tree: usvg::Tree,
    pixels: (u32, u32),
    transform: Transform,
    origin: Point<f32>,
    extent: Size<f32>,
}

fn plan(tree: usvg::Tree, width: f32, scale: f32, turn: &wire::SvgTransformation) -> Option<Plan> {
    let svg = tree.size();
    let height = svg.height() * width / svg.width();
    let (sin, cos) = turn.rotate.sin_cos();
    let [sx, sy] = turn.scale;
    let linear = [cos * sx, sin * sx, -sin * sy, cos * sy];
    let shift = turn.translate.map(|by| by * scale);
    let place = |x: f32, y: f32| {
        point(
            linear[0] * x + linear[2] * y + shift[0],
            linear[1] * x + linear[3] * y + shift[1],
        )
    };
    let corners = [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)]
        .map(|(x, y): (f32, f32)| place(x * width / 2., y * height / 2.));
    let low = corners
        .iter()
        .fold(point(f32::INFINITY, f32::INFINITY), |low, at| {
            point(low.x.min(at.x), low.y.min(at.y))
        });
    let high = (corners.iter()).fold(point(f32::NEG_INFINITY, f32::NEG_INFINITY), |high, at| {
        point(high.x.max(at.x), high.y.max(at.y))
    });
    let extent = size(high.x - low.x, high.y - low.y);
    let (fit, pixels) = fitted(extent.width, extent.height)?;
    let corner = place(-width / 2., -height / 2.);
    let (along, down) = (fit * width / svg.width(), fit * height / svg.height());
    let transform = Transform::from_row(
        linear[0] * along,
        linear[1] * along,
        linear[2] * down,
        linear[3] * down,
        fit * (corner.x - low.x),
        fit * (corner.y - low.y),
    );
    Some(Plan {
        tree,
        pixels,
        transform,
        origin: low,
        extent,
    })
}

fn tinted(plan: &Plan, tint: u32) -> Option<Arc<RenderImage>> {
    let (width, height) = plan.pixels;
    let mut pixmap = Pixmap::new(width, height)?;
    resvg::render(&plan.tree, plan.transform, &mut pixmap.as_mut());
    let [red, green, blue, alpha] = tint.to_be_bytes();
    let alpha = u16::from(alpha);
    let pixels = (pixmap.data().chunks_exact(4))
        .flat_map(|pixel| {
            let covered = (alpha * u16::from(pixel[3]) + 127) / 255;
            [blue, green, red, covered as u8]
        })
        .collect();
    let frame = image::RgbaImage::from_raw(width, height, pixels)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(frame)])))
}

fn admission_ledgers(cx: &mut App) -> &mut SvgAdmissionLedgers {
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
            budget: crate::backend::memory_bounds().svg_raster_bytes,
            _cleanup: cleanup,
        });
    }
    cx.global_mut::<SvgAdmissionLedgers>()
}

fn window_ledger(window_id: WindowId, cx: &mut App) -> Rc<RefCell<WindowSvgLedger>> {
    admission_ledgers(cx)
        .windows
        .entry(window_id)
        .or_default()
        .clone()
}

fn pinned(ledger: &Rc<RefCell<WindowSvgLedger>>, raster: &Raster) -> Option<SvgPin> {
    ledger.borrow_mut().drew(raster, true).then(|| SvgPin {
        ledger: ledger.clone(),
        raster: raster.clone(),
    })
}

fn admit(
    ledger: &Rc<RefCell<WindowSvgLedger>>,
    raster: Raster,
    charge: u64,
    window: &mut Window,
    cx: &mut App,
) -> Option<SvgPin> {
    let budget = admission_ledgers(cx).budget;
    loop {
        let evicted = {
            let mut ledger = ledger.borrow_mut();
            if ledger.fits(charge, budget) {
                ledger.bytes += charge;
                let drawn = ledger.tick;
                ledger.rasters.insert(
                    raster.clone(),
                    HeldRaster {
                        charge,
                        drawn,
                        pins: 1,
                        placed: None,
                    },
                );
                break;
            }
            ledger.evict()?
        };
        drop_raster(evicted, window, cx);
    }
    Some(SvgPin {
        ledger: ledger.clone(),
        raster,
    })
}

fn drop_raster((raster, held): (Raster, HeldRaster), window: &mut Window, cx: &mut App) {
    if let Some(placed) = held.placed {
        window.drop_image(placed.image).ok();
    }
    if let Raster::Image(image) = raster {
        if let Some(decoded) = image.clone().get_render_image(window, cx) {
            window.drop_image(decoded).ok();
        }
        let raster = Raster::Image(image.clone());
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

pub(super) fn svg_admitted_bytes(window_id: WindowId, cx: &App) -> u64 {
    cx.try_global::<SvgAdmissionLedgers>()
        .and_then(|ledgers| ledgers.windows.get(&window_id))
        .map_or(0, |ledger| ledger.borrow().bytes)
}

fn full_size(window: &mut Window, cx: &mut App) -> LayoutId {
    let style = gpui_kit::Style {
        size: size(relative(1.).into(), relative(1.).into()),
        ..Default::default()
    };
    window.request_layout(style, [], cx)
}

pub(super) fn svg_glyph(source: GlyphSource, turn: wire::SvgTransformation) -> SvgGlyph {
    SvgGlyph {
        key: source.key(),
        source,
        turn,
    }
}

pub(super) struct SvgGlyph {
    source: GlyphSource,
    key: u64,
    turn: wire::SvgTransformation,
}

enum Drawn {
    Held,
    Blank,
    Refused,
}

impl SvgGlyph {
    fn admit(
        &self,
        key: GlyphKey,
        ledger: &Rc<RefCell<WindowSvgLedger>>,
        window: &mut Window,
        cx: &mut App,
    ) -> (Drawn, Option<SvgPin>) {
        let raster = Raster::Glyph(key);
        if let Some(pin) = pinned(ledger, &raster) {
            return (Drawn::Held, Some(pin));
        }
        let (width, scale) = (f32::from_bits(key.width), f32::from_bits(key.scale));
        let Some(plan) =
            (self.source.tree(cx)).and_then(|tree| plan(tree, width, scale, &self.turn))
        else {
            return (Drawn::Blank, None);
        };
        let Some(pin) = admit(ledger, raster, pixels_charge(plan.pixels), window, cx) else {
            return (Drawn::Refused, None);
        };
        let placed = tinted(&plan, key.tint).map(|image| Placed {
            image,
            origin: plan.origin,
            extent: plan.extent,
        });
        if let Some(held) = ledger.borrow_mut().rasters.get_mut(&pin.raster) {
            held.placed = placed;
        }
        (Drawn::Held, Some(pin))
    }
}

impl IntoElement for SvgGlyph {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SvgGlyph {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(host_id(format!("svg/{:016x}", self.key)))
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
        (full_size(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let scale = window.scale_factor() * SMOOTH_SVG_SCALE_FACTOR;
        let turn = self.turn;
        let key = GlyphKey {
            source: self.key,
            width: (f32::from(bounds.size.width) * scale).ceil().to_bits(),
            scale: scale.to_bits(),
            tint: u32::from(Rgba::from(window.text_style().color)),
            turn: [
                turn.scale[0],
                turn.scale[1],
                turn.translate[0],
                turn.translate[1],
                turn.rotate,
            ]
            .map(f32::to_bits),
        };
        let raster = Raster::Glyph(key);
        let ledger = window_ledger(window.window_handle().window_id(), cx);
        let drawn = window.with_element_state(
            global_id.expect("the glyph has an id"),
            |pin: Option<Option<SvgPin>>, window| match pin.flatten() {
                Some(pin) if pin.raster == raster => {
                    ledger.borrow_mut().drew(&raster, false);
                    (Drawn::Held, Some(pin))
                }
                other => {
                    drop(other);
                    self.admit(key, &ledger, window, cx)
                }
            },
        );
        match drawn {
            Drawn::Held => {
                let placed = (ledger.borrow().rasters.get(&raster))
                    .and_then(|held| held.placed.as_ref())
                    .map(|placed| (placed.image.clone(), placed.origin, placed.extent));
                let Some((image, origin, extent)) = placed else {
                    return;
                };
                let at = Bounds::new(
                    bounds.center() + point(px(origin.x / scale), px(origin.y / scale)),
                    size(px(extent.width / scale), px(extent.height / scale)),
                );
                if let Err(error) = window.paint_image(at, at, Corners::default(), image, 0, false)
                {
                    tracing::warn!(
                        target: "ducktape::app",
                        reason = "view_svg_paint_failed",
                        %error,
                        "a view SVG's raster did not paint"
                    );
                }
            }
            Drawn::Refused => window.paint_quad(fill(bounds, rgb(0x7f1d1d))),
            Drawn::Blank => {}
        }
    }
}

pub(super) fn guarded_image(image: Arc<Image>, child: impl IntoElement) -> SvgPaintGuard {
    SvgPaintGuard {
        image,
        child: child.into_any_element(),
    }
}

pub(super) struct SvgPaintGuard {
    image: Arc<Image>,
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
        Some(host_id(format!("svg-image/{:016x}", self.image.id())))
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
        let scale = window.scale_factor() * SMOOTH_SVG_SCALE_FACTOR;
        let width = (f32::from(bounds.size.width) * scale).ceil();
        let raster = Raster::Image(self.image.clone());
        let ledger = window_ledger(window.window_handle().window_id(), cx);
        let image = &self.image;
        let allowed = window.with_element_state(
            global_id.expect("the guard has an id"),
            |pin: Option<Option<SvgPin>>, window| {
                let pin = match pin.flatten() {
                    Some(pin) if pin.raster == raster => {
                        ledger.borrow_mut().drew(&raster, false);
                        Some(pin)
                    }
                    other => {
                        drop(other);
                        pinned(&ledger, &raster).or_else(|| {
                            let charge = image_charge(width, image);
                            admit(&ledger, raster.clone(), charge, window, cx)
                        })
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
            window.paint_quad(fill(bounds, rgb(0x7f1d1d)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AnyView, ImageId, StyleRefinement};
    use std::{cell::Cell, collections::HashSet};

    const HELD: usize = 4_096;
    const SQUARE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><path d="M1 1h2v2H1z"/></svg>"#;

    fn unturned() -> wire::SvgTransformation {
        wire::SvgTransformation {
            scale: [1., 1.],
            translate: [0., 0.],
            rotate: 0.,
        }
    }

    fn parsed(svg: &[u8]) -> usvg::Tree {
        crate::render::svg_check::parse(svg).unwrap()
    }

    fn sized(svg: (f32, f32), width: f32, turn: wire::SvgTransformation) -> Option<Plan> {
        let markup = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}"/>"#,
            svg.0, svg.1
        );
        plan(parsed(markup.as_bytes()), width, 2., &turn)
    }

    #[test]
    fn a_glyph_is_charged_the_raster_it_draws() {
        let pixels = |svg, width| sized(svg, width, unturned()).map(|plan| plan.pixels);
        assert_eq!(pixels((24., 24.), 96.), Some((96, 96)));
        assert_eq!(pixels((10., 5.), 48.), Some((48, 24)));
        assert_eq!(
            pixels((1., 100.), 200.),
            Some((81, 8192)),
            "a tall SVG scales down until its height is 8192px"
        );
        assert_eq!(
            pixels((24., 24.), 0.),
            None,
            "a box with no width draws nothing"
        );
        assert_eq!(pixels_charge((96, 96)), 96 * 96 * 8);
    }

    #[test]
    fn a_turned_glyph_turns_round_the_middle_of_its_box() {
        let quarter = wire::SvgTransformation {
            rotate: std::f32::consts::FRAC_PI_2,
            ..unturned()
        };
        assert_eq!(sized((10., 5.), 48., quarter).unwrap().pixels, (24, 48));
        let moved = wire::SvgTransformation {
            translate: [3., -1.],
            ..unturned()
        };
        let moved = sized((10., 5.), 48., moved).unwrap();
        assert_eq!((moved.pixels, moved.origin), ((48, 24), point(-18., -14.)));
        let halved = wire::SvgTransformation {
            scale: [0.5, 0.5],
            ..unturned()
        };
        let halved = sized((10., 5.), 48., halved).unwrap();
        assert_eq!((halved.pixels, halved.origin), ((24, 12), point(-12., -6.)));
    }

    #[test]
    fn a_glyph_takes_the_colour_it_paints_in_through_its_alpha() {
        let plan = plan(parsed(SQUARE), 4., 1., &unturned()).unwrap();
        let image = tinted(&plan, 0x1122_3380).unwrap();
        let pixels = image.as_bytes(0).unwrap();
        assert_eq!(
            pixels[5 * 4..][..4],
            [0x33, 0x22, 0x11, 0x80],
            "inside the square"
        );
        assert_eq!(pixels[..4], [0x33, 0x22, 0x11, 0], "outside it");
    }

    fn hold_only(rasters: usize, charge: u64, cx: &mut App) {
        admission_ledgers(cx).budget = rasters as u64 * (charge + ENTRY_BYTES);
    }

    fn key(source: u64, width: u32) -> Raster {
        Raster::Glyph(GlyphKey {
            source,
            width,
            scale: 0,
            tint: 0,
            turn: [0; 5],
        })
    }

    fn admit_key(raster: Raster, charge: u64, window: &mut Window, cx: &mut App) -> Option<SvgPin> {
        let ledger = window_ledger(window.window_handle().window_id(), cx);
        pinned(&ledger, &raster).or_else(|| admit(&ledger, raster, charge, window, cx))
    }

    struct Blank;

    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui_kit::test]
    fn window_aggregate_reuses_keys_and_refuses_new_rasters_at_budget(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(10.), px(10.)), |_, _| Blank);
        let charge = 1 << 20;
        cx.update_window(window.into(), |_, window, cx| {
            let window_id = window.window_handle().window_id();
            let mut pins = vec![
                admit_key(key(1, 2), charge, window, cx).unwrap(),
                admit_key(key(1, 2), charge, window, cx).unwrap(),
            ];
            assert_eq!(svg_admitted_bytes(window_id, cx), charge);
            pins.push(admit_key(key(1, 4), charge, window, cx).unwrap());
            assert_eq!(svg_admitted_bytes(window_id, cx), charge * 2);
            let mut source = 2;
            while let Some(pin) = admit_key(key(source, 2), charge, window, cx) {
                pins.push(pin);
                source += 1;
            }
            assert!(source > 2, "the budget admitted some distinct sources");
            assert!(svg_admitted_bytes(window_id, cx) <= admission_ledgers(cx).budget);
            assert!(admit_key(key(1, 2), charge, window, cx).is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn window_aggregate_charges_each_key_and_cleans_up_closed_windows(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(10.), px(10.)), |_, _| Blank);
        let window_id = window.window_id();
        cx.update_window(window.into(), |_, window, cx| {
            admission_ledgers(cx).windows.remove(&window_id);
            hold_only(HELD, 0, cx);
            let pins: Vec<_> = (0..HELD)
                .map(|source| admit_key(key(source as u64, 2), 0, window, cx).unwrap())
                .collect();
            assert!(admit_key(key(HELD as u64, 2), 0, window, cx).is_none());
            drop(pins);
        })
        .unwrap();
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(svg_admitted_bytes(window_id, cx), 0));
    }

    fn distinct_icon(index: usize) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" id="icon-{index}"><circle cx="12" cy="12" r="10"/></svg>"#
        )
    }

    fn icon_source(index: usize) -> GlyphSource {
        GlyphSource::Data(Arc::from(distinct_icon(index).as_bytes()))
    }

    fn icon_charge(side: f32, window: &Window) -> u64 {
        let scale = window.scale_factor() * SMOOTH_SVG_SCALE_FACTOR;
        let tree = parsed(distinct_icon(0).as_bytes());
        pixels_charge(
            plan(tree, (side * scale).ceil(), scale, &unturned())
                .unwrap()
                .pixels,
        )
    }

    fn glyphs(window_id: WindowId, cx: &App) -> HashMap<GlyphKey, ImageId> {
        let ledger = cx.global::<SvgAdmissionLedgers>().windows[&window_id].borrow();
        (ledger.rasters.iter())
            .filter_map(|(raster, held)| match raster {
                Raster::Glyph(key) => Some((*key, held.placed.as_ref()?.image.id)),
                Raster::Image(_) => None,
            })
            .collect()
    }

    fn in_atlas(id: ImageId, window: &Window) -> bool {
        let mut probe = RenderImage::new(vec![image::Frame::new(image::RgbaImage::new(1, 1))]);
        probe.id = id;
        window.has_image_atlas_entry(&probe)
    }

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
            div()
                .flex()
                .flex_wrap()
                .size_full()
                .text_color(rgb(0xffffff))
                .children((self.first..self.first + self.count).map(move |index| {
                    div()
                        .size(px(side))
                        .child(svg_glyph(icon_source(index), unturned()))
                }))
        }
    }

    #[gpui_kit::test]
    fn two_hundred_distinct_icons_all_paint_in_one_window(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(480.), px(240.)), |_, _| Churn::new(0, 200));
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            assert!(
                window.painted_quads().is_empty(),
                "an icon drew its refusal"
            );
            let window_id = window.window_handle().window_id();
            let held = glyphs(window_id, cx);
            assert_eq!(held.len(), 200, "every icon has its own raster");
            assert_eq!(
                svg_admitted_bytes(window_id, cx),
                200 * icon_charge(24., window),
                "each icon is charged its own raster"
            );
            assert!(
                held.values().all(|id| in_atlas(*id, window)),
                "every icon painted"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn a_glyph_past_the_budget_draws_its_refusal_and_one_that_fits_paints(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let small = cx.open_window(size(px(24.), px(24.)), |_, _| Churn::new(0, 1));
        cx.update_window(small.into(), |_, window, cx| {
            hold_only(1, icon_charge(24., window), cx);
            window.draw(cx).clear(cx);
            assert!(window.painted_quads().is_empty(), "the small icon painted");
            assert_eq!(glyphs(window.window_handle().window_id(), cx).len(), 1);
        })
        .unwrap();
        let large = cx.open_window(size(px(48.), px(48.)), |_, _| Churn {
            side: 48.,
            ..Churn::new(1, 1)
        });
        cx.update_window(large.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            assert_eq!(
                window.painted_quads().len(),
                1,
                "the large icon drew its refusal"
            );
            assert!(glyphs(window.window_handle().window_id(), cx).is_empty());
        })
        .unwrap();
    }

    fn assert_ledger_and_atlas_agree(
        seen: &mut HashMap<GlyphKey, ImageId>,
        frame: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> usize {
        let window_id = window.window_handle().window_id();
        assert!(window.painted_quads().is_empty(), "{frame} drew a refusal");
        let held = glyphs(window_id, cx);
        assert!(svg_admitted_bytes(window_id, cx) <= admission_ledgers(cx).budget);
        for id in held.values() {
            assert!(
                in_atlas(*id, window),
                "{frame}: a held raster is in the atlas"
            );
        }
        seen.extend(held.iter().map(|(key, id)| (*key, *id)));
        let evicted: Vec<ImageId> = (seen.iter())
            .filter(|(key, _)| !held.contains_key(key))
            .map(|(_, id)| *id)
            .collect();
        for id in &evicted {
            assert!(
                !in_atlas(*id, window),
                "{frame}: an evicted raster's tile left the atlas"
            );
        }
        evicted.len()
    }

    #[gpui_kit::test]
    fn icons_drawn_over_time_past_the_budget_never_refuse(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1200.), px(1200.)), |_, _| Churn::new(0, 250));
        cx.update_window(window.into(), |_, window, cx| {
            hold_only(HELD, icon_charge(24., window), cx)
        })
        .unwrap();
        let mut seen = HashMap::new();
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
            assert!(evicted >= 10_000 - HELD);
        })
        .unwrap();
    }

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
        let still_sources: HashSet<u64> = (100_000..100_040)
            .map(|index| icon_source(index).key())
            .collect();
        let mut still_keys = HashMap::new();
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            still_keys = glyphs(window.window_handle().window_id(), cx);
            still_keys.retain(|key, _| still_sources.contains(&key.source));
        })
        .unwrap();
        assert_eq!(still_keys.len(), 40);
        cx.update_window(window.into(), |_, window, cx| {
            hold_only(HELD, icon_charge(24., window), cx)
        })
        .unwrap();
        let renders_before = still_renders.get();
        let mut seen = HashMap::new();
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
                for id in still_keys.values() {
                    assert!(
                        in_atlas(*id, window),
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
        assert!(seen.len() > HELD, "the busy view went past the budget");
    }

    #[gpui_kit::test]
    fn icons_swapped_wholesale_wait_one_frame_for_the_last_ones_to_go(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let count = HELD / 2 + 50;
        let window = cx.open_window(size(px(1200.), px(1200.)), move |_, _| Churn {
            side: 1.,
            ..Churn::new(0, count)
        });
        cx.update_window(window.into(), |_, window, cx| {
            hold_only(HELD, icon_charge(1., window), cx)
        })
        .unwrap();
        window
            .update(cx, |churn, _, cx| {
                churn.first = count;
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            assert_eq!(
                window.painted_quads().len(),
                2 * count - HELD,
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

    #[gpui_kit::test]
    fn a_frame_alone_over_the_budget_still_refuses(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(1200.), px(1200.)), |_, _| Churn {
            side: 1.,
            ..Churn::new(0, 0)
        });
        cx.update_window(window.into(), |_, window, cx| {
            hold_only(HELD, icon_charge(1., window), cx)
        })
        .unwrap();
        window
            .update(cx, |churn, _, cx| {
                churn.count = HELD + 1;
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let held = glyphs(window.window_handle().window_id(), cx);
            assert_eq!(held.len(), HELD);
            assert_eq!(
                window.painted_quads().len(),
                1,
                "the icon past the budget drew its refusal"
            );
            for id in held.values() {
                assert!(in_atlas(*id, window), "the frame's own rasters stayed");
            }
        })
        .unwrap();
    }

    struct CanvasPicture {
        image: Option<Arc<Image>>,
    }

    impl Render for CanvasPicture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children(
                (self.image.clone())
                    .map(|image| guarded_image(image.clone(), img(image).size(px(24.)))),
            )
        }
    }

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
            hold_only(HELD, 0, cx);
            let pins: Vec<_> = (0..HELD)
                .map(|source| admit_key(key(source as u64, 2), 0, window, cx).unwrap())
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
