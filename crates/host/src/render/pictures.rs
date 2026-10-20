//! Image, Svg and Canvas nodes. A picture's bytes cross the wire once;
//! later frames name it by the view's content hash, and the tree draws
//! that hash from the seat's [`PictureBytes`]. Rasters decoded from them
//! are cached in [`Rasters`].
use super::picture_resources::{cache_fits, decode_image};
use super::*;
use crate::render::native_id;
use std::{cell::RefCell, rc::Rc};

pub use crate::runtime::pictures::PictureBytes;

/// The rasters decoded from a seat's picture bytes, by content hash: one
/// cache for the seat's tree and the tooltip trees it opens, carried
/// across a hot swap, and released with their atlas tiles when the seat
/// drops (`ViewTree::release`). Past the picture memory setting
/// (`picture_resources`) it evicts the rasters drawn least recently before
/// the current frame, whose bytes the seat still holds to decode again; a raster that does not fit
/// beside the current frame's is refused, and its Image draws the view's
/// fallback.
#[derive(Default)]
pub struct Rasters {
    /// Each raster, and the frame that last drew it.
    held: HashMap<u64, (Arc<RenderImage>, u64)>,
    bytes: usize,
    /// The frame the seat's tree adopted last (`ViewTree::replace`).
    frame: u64,
    /// The first refusal is logged; later ones are the same story.
    refused: bool,
}

/// [`Rasters`] as the seat's trees share it.
pub type SharedRasters = Rc<RefCell<Rasters>>;

fn raster_len(image: &RenderImage) -> usize {
    image.as_bytes(0).map_or(0, <[u8]>::len)
}

impl Rasters {
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }

    /// A new frame was adopted: what the last one drew may now be evicted.
    pub(super) fn next_frame(&mut self) {
        self.frame += 1;
    }

    /// Decodes `data` under `hash` unless it is held, and marks it drawn by
    /// the current frame. `false` when it did not fit beside what the
    /// current frame draws.
    fn remember(
        &mut self,
        hash: u64,
        data: &wire::ImageData,
        mut window: Option<&mut Window>,
        cx: &mut App,
    ) -> bool {
        if matches!(
            data,
            wire::ImageData::Resource(_) | wire::ImageData::Refusal(_)
        ) {
            return true;
        }
        if let Some((_, drawn)) = self.held.get_mut(&hash) {
            *drawn = self.frame;
            return true;
        }
        if !self.make_room(0, window.as_deref_mut(), cx) {
            return false;
        }
        let Some(image) = decode_image(data) else {
            return true;
        };
        let len = raster_len(&image);
        if !self.make_room(len, window, cx) {
            return false;
        }
        self.bytes += len;
        self.held.insert(hash, (Arc::new(image), self.frame));
        true
    }

    /// Evicts the rasters drawn least recently, never one the current frame
    /// drew, until one more raster of `additional` bytes fits. `false` when
    /// it still does not.
    fn make_room(
        &mut self,
        additional: usize,
        mut window: Option<&mut Window>,
        cx: &mut App,
    ) -> bool {
        let budget = crate::backend::memory_bounds().picture_bytes;
        while !cache_fits(self.held.len(), self.bytes, additional, budget) {
            let Some(hash) = (self.held.iter())
                .filter(|(_, (_, drawn))| *drawn < self.frame)
                .min_by_key(|(_, (_, drawn))| *drawn)
                .map(|(hash, _)| *hash)
            else {
                return false;
            };
            if let Some((image, _)) = self.held.remove(&hash) {
                self.bytes -= raster_len(&image);
                cx.drop_image(image, window.as_deref_mut());
            }
        }
        true
    }

    fn log_refusal(&mut self) {
        if !std::mem::replace(&mut self.refused, true) {
            tracing::warn!(
                target: "ducktape::app",
                reason = "program_view_image_cache_full",
                held = self.bytes,
                rasters = self.held.len(),
                "program view images past the seat's raster cache; drawing their fallback"
            );
        }
    }

    fn get(&self, hash: u64) -> Option<Arc<RenderImage>> {
        self.held.get(&hash).map(|(image, _)| image.clone())
    }

    /// Drops every raster and its atlas tiles in every window. Called
    /// outside any window's update, so `drop_image` reaches them all.
    fn release(&mut self, cx: &mut App) {
        self.bytes = 0;
        for (_, (image, _)) in self.held.drain() {
            cx.drop_image(image, None);
        }
    }
}

mod svg_canvas;
use svg_canvas::SvgCanvas;

impl ViewTree {
    pub(super) fn vector(
        &mut self,
        node: &wire::Node,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Svg {
            source,
            transformation,
            style,
            ..
        } = node
        else {
            unreachable!()
        };
        // the bytes the node brings, else the seat's under its hash
        let data: Option<(Arc<[u8]>, crate::runtime::pictures::Checked)> = match source {
            wire::SvgSource::Data {
                bytes: Some(bytes), ..
            } => {
                let bytes: Arc<[u8]> = Arc::from(bytes.as_slice());
                let checked = super::svg_check::parse(&bytes).map(drop);
                Some((bytes, checked))
            }
            wire::SvgSource::Data { hash, bytes: None } => {
                self.pictures.vector.get(hash).map(|held| {
                    let checked = held
                        .checked
                        .get_or_init(|| super::svg_check::parse(&held.bytes).map(drop));
                    (held.bytes.clone(), *checked)
                })
            }
            _ => None,
        };
        let mut element = div();
        *element.style() = Arc::unwrap_or_clone(self.style(*style));
        element = match source {
            wire::SvgSource::Data { .. } => match data {
                Some((_, Err(refused))) => element.child(refused),
                Some((bytes, Ok(()))) => {
                    element.child(svg_glyph(GlyphSource::Data(bytes), *transformation))
                }
                None => element.child("SVG data unavailable"),
            },
            wire::SvgSource::Asset(path) if wire::safe_asset_path(path) => element.child(
                svg_glyph(GlyphSource::Asset(path.clone().into()), *transformation),
            ),
            wire::SvgSource::Asset(_) => element.child("SVG asset identifier refused"),
            wire::SvgSource::External(_) => element.child("External SVG path refused"),
            wire::SvgSource::None => element.child("SVG source unavailable"),
        };
        self.primitive_interactivity(element, node, cx)
    }

    pub(super) fn drawing(&mut self, node: &wire::Node, _cx: &mut Context<Self>) -> AnyElement {
        let wire::Node::Canvas { commands, style } = node else {
            unreachable!()
        };
        let mut root = div().relative().overflow_hidden();
        *root.style() = Arc::unwrap_or_clone(self.style(*style));
        let commands = commands.clone();
        if native_canvas_commands(&commands) {
            return root
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            paint_canvas_commands(&commands, bounds.origin, window);
                        },
                    )
                    .size_full(),
                )
                .into_any_element();
        }
        root.child(SvgCanvas { commands }).into_any_element()
    }

    /// The loading or the fallback child of an Image. `children` holds them
    /// in that order, each present only when its flag says so (the view SDK
    /// pushes loading first, then fallback), so which index is which depends
    /// on both flags.
    fn image_state(
        loading: bool,
        fallback: bool,
        children: &[wire::Node],
        want_fallback: bool,
    ) -> Option<&wire::Node> {
        match (want_fallback, loading, fallback) {
            (false, true, _) => children.first(),
            (true, true, true) => children.get(1),
            (true, false, true) => children.first(),
            _ => None,
        }
    }

    /// The raster under `hash`, decoded from `data` unless held; `false`
    /// when the cache had no room for it (`Rasters::remember`). `window` is
    /// the one being drawn, if any, so an eviction drops its tiles too.
    pub(super) fn remember_image(
        &mut self,
        hash: u64,
        data: &wire::ImageData,
        window: Option<&mut Window>,
        cx: &mut App,
    ) -> bool {
        self.images.borrow_mut().remember(hash, data, window, cx)
    }

    pub(super) fn image_frame(
        &self,
        hash: u64,
        data: Option<&wire::ImageData>,
    ) -> Option<Arc<RenderImage>> {
        match data {
            Some(wire::ImageData::Resource(_) | wire::ImageData::Refusal(_)) => None,
            _ => self.images.borrow().get(hash),
        }
    }

    /// The raster held under `hash`, for a test to find its atlas tiles.
    #[cfg(any(test, feature = "test-support"))]
    pub fn image_for_test(&self, hash: u64) -> Option<Arc<RenderImage>> {
        self.images.borrow().get(hash)
    }

    /// The seat is dropping this tree: its rasters leave every window's
    /// atlas (never on a hot swap, whose next tree carries them), and its
    /// cameras and microphones close now, not when gpui gets round to it.
    pub fn release(&mut self, cx: &mut App) {
        self.images.borrow_mut().release(cx);
        self.release_media();
    }

    pub(super) fn picture(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Image {
            hash,
            data,
            image_style,
            loading,
            fallback,
            state_children,
            style,
            ..
        } = node
        else {
            unreachable!()
        };
        // the bytes the node brings, else the seat's under its hash
        let pictures = self.pictures.clone();
        let data = data
            .as_ref()
            .or_else(|| pictures.raster.get(hash).map(|data| &**data));
        let full = data.is_some_and(|data| !self.remember_image(*hash, data, Some(window), cx));
        let mut element = div();
        *element.style() = Arc::unwrap_or_clone(self.style(*style));
        match data {
            Some(wire::ImageData::Refusal(reason)) => {
                element = match Self::image_state(*loading, *fallback, state_children, true) {
                    Some(child) => element.child(self.node(child, window, cx)),
                    None => element.child(reason.clone()),
                };
            }
            Some(wire::ImageData::Resource(_)) => {
                element = element.child("Host image resource unavailable")
            }
            _ => {
                if full {
                    // a picture the cache has no room for draws the
                    // view's fallback, as a refused one does
                    self.images.borrow_mut().log_refusal();
                    if let Some(child) =
                        Self::image_state(*loading, *fallback, state_children, true)
                    {
                        element = element.child(self.node(child, window, cx));
                    }
                } else if let Some(image) = self.image_frame(*hash, data) {
                    let mut picture = img(image)
                        .size_full()
                        .grayscale(image_style.grayscale)
                        .object_fit(primitive_object_fit(image_style.object_fit));
                    picture.style().corner_radii = element.style().corner_radii.clone();
                    element = element.child(picture);
                } else if let Some(child) =
                    Self::image_state(*loading, *fallback, state_children, false)
                {
                    // the loading placeholder is an ordinary view node, laid
                    // out and clipped inside this image's box
                    element = element.child(self.node(child, window, cx));
                }
            }
        }
        self.primitive_interactivity(element, node, cx)
    }

    /// An Image's or Svg's box as the view styled and wired it; its role
    /// and name come through `accessible` (`view_aria`), so a labelled
    /// picture with no role is an Image.
    fn primitive_interactivity(
        &mut self,
        element: Div,
        node: &wire::Node,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (wire::Node::Image {
            id, interactivity, ..
        }
        | wire::Node::Svg {
            id, interactivity, ..
        }) = node
        else {
            unreachable!()
        };
        let interactivity = interactivity
            .as_deref()
            .unwrap_or(wire::Interactivity::none());
        let native_id = id.as_ref().map(native_id).unwrap_or_else(|| {
            let index = self.render_index;
            self.render_index += 1;
            host_id(format!("primitive-{index}"))
        });
        let element = element.id(native_id);
        self.view_aria(element, node, interactivity, cx)
            .into_any_element()
    }
}

/// gpui's fit for a picture's: an image's, a camera's (`render/av.rs`).
pub(super) fn primitive_object_fit(fit: wire::ImageObjectFit) -> ObjectFit {
    match fit {
        wire::ImageObjectFit::Fill => ObjectFit::Fill,
        wire::ImageObjectFit::Contain => ObjectFit::Contain,
        wire::ImageObjectFit::Cover => ObjectFit::Cover,
        wire::ImageObjectFit::ScaleDown => ObjectFit::ScaleDown,
        wire::ImageObjectFit::None => ObjectFit::None,
    }
}
