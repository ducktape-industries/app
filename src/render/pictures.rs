//! Image, Svg and Canvas nodes, and the per-view caches behind them:
//! `images` (decoded rasters) and `vectors` (SVG bytes), both keyed by the
//! guest's content hash so a later frame can name a picture without
//! resending it. `qr` is the shell's (sign-in); no wire node draws one.
use super::picture_resources::{cache_fits, decode_image};
use super::*;
use crate::render::native_id;

mod svg_canvas;
use svg_canvas::SvgCanvas;

pub(crate) fn qr(code: &wire::Qr) -> AnyElement {
    let Some(payload) = &code.payload else {
        return div().into_any_element();
    };
    let correction = match code.correction {
        Some(wire::QrCorrection::Low) => qrcode::EcLevel::L,
        Some(wire::QrCorrection::Quartile) => qrcode::EcLevel::Q,
        Some(wire::QrCorrection::High) => qrcode::EcLevel::H,
        Some(wire::QrCorrection::Medium) | None => qrcode::EcLevel::M,
    };
    let result = match code.version {
        Some(wire::QrVersion::Normal(value)) => qrcode::QrCode::with_version(
            payload,
            qrcode::Version::Normal(i16::from(value)),
            correction,
        ),
        Some(wire::QrVersion::Micro(value)) => qrcode::QrCode::with_version(
            payload,
            qrcode::Version::Micro(i16::from(value)),
            correction,
        ),
        None => qrcode::QrCode::with_error_correction_level(payload, correction),
    };
    let Ok(matrix) = result else {
        return div()
            .child("QR payload exceeds the selected code capacity")
            .into_any_element();
    };
    let modules = matrix.width() + 8;
    let total = match code.size {
        Some(wire::QrSize::Total(size)) => size,
        Some(wire::QrSize::Cell(size)) => size * modules as f32,
        None => 4.0 * modules as f32,
    };
    let cell = code.cell.unwrap_or_else(|| rgb(0).into());
    let background = code.background.unwrap_or_else(|| rgb(0xffffff).into());
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            window.paint_quad(fill(bounds, background));
            let unit = f32::from(bounds.size.width.min(bounds.size.height)) / modules as f32;
            for y in 0..matrix.width() {
                for x in 0..matrix.width() {
                    if matrix[(x, y)] == qrcode::Color::Dark {
                        let origin = bounds.origin
                            + point(px((x + 4) as f32 * unit), px((y + 4) as f32 * unit));
                        window
                            .paint_quad(fill(Bounds::new(origin, size(px(unit), px(unit))), cell));
                    }
                }
            }
        },
    )
    .w(px(total))
    .h(px(total))
    .into_any_element()
}

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
        let refused_data = matches!(
            source,
            wire::SvgSource::Data {
                bytes: Some(bytes),
                ..
            } if !svg_data_allowed(bytes)
        );
        if let wire::SvgSource::Data {
            hash,
            bytes: Some(bytes),
        } = source
            && !refused_data
        {
            self.remember_vector(*hash, bytes);
        }
        let mut element = div();
        *element.style() = style.clone();
        let native_transform =
            Transformation::scale(size(transformation.scale[0], transformation.scale[1]))
                .with_translation(point(
                    px(transformation.translate[0]),
                    px(transformation.translate[1]),
                ))
                .with_rotation(radians(transformation.rotate));
        element = match source {
            wire::SvgSource::Data { .. } if refused_data => {
                element.child("Compressed SVG data refused")
            }
            wire::SvgSource::Data { hash, .. } => match self.vectors.get(hash) {
                Some(bytes) => element.child(guarded_svg_paint(
                    SvgPaintSource::data(bytes),
                    svg()
                        .data(bytes)
                        .with_transformation(native_transform)
                        .size_full(),
                )),
                None => element.child("SVG data unavailable"),
            },
            wire::SvgSource::Asset(path) if safe_asset_path(path) => {
                element.child(guarded_svg_paint(
                    SvgPaintSource::asset(path.clone()),
                    svg()
                        .path(path.clone())
                        .with_transformation(native_transform)
                        .size_full(),
                ))
            }
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
        *root.style() = style.clone();
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
    /// in that order, each present only when its flag says so (the guest SDK
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

    pub(super) fn remember_image(&mut self, hash: u64, data: &wire::ImageData) {
        if matches!(
            data,
            wire::ImageData::Resource(_) | wire::ImageData::Refusal(_)
        ) {
            return;
        }
        if self.images.contains_key(&hash) {
            return;
        }
        let used = self
            .images
            .values()
            .map(|image| image.as_bytes(0).map_or(0, <[u8]>::len))
            .sum();
        if !cache_fits(self.images.len(), used, 1) {
            return;
        }
        let Some(image) = decode_image(data) else {
            return;
        };
        if !cache_fits(
            self.images.len(),
            used,
            image.as_bytes(0).map_or(0, <[u8]>::len),
        ) {
            return;
        }
        self.images.insert(hash, Arc::new(image));
    }

    pub(super) fn image_frame(
        &self,
        hash: u64,
        data: Option<&wire::ImageData>,
    ) -> Option<Arc<RenderImage>> {
        match data {
            Some(wire::ImageData::Resource(_) | wire::ImageData::Refusal(_)) => None,
            _ => self.images.get(&hash).cloned(),
        }
    }

    pub(super) fn remember_vector(&mut self, hash: u64, bytes: &[u8]) {
        if !svg_data_allowed(bytes) || self.vectors.contains_key(&hash) {
            return;
        }
        let used = self.vectors.values().map(|bytes| bytes.len()).sum();
        // Never evict: a guest sends an SVG's bytes once and names it by hash
        // afterwards, so an evicted entry could never come back. When full, a
        // new SVG is not cached and draws as "SVG data unavailable" (`vector`).
        if !cache_fits(self.vectors.len(), used, bytes.len()) {
            return;
        }
        self.vectors.insert(hash, Arc::from(bytes));
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
        if let Some(data) = data {
            self.remember_image(*hash, data);
        }
        let mut element = div();
        *element.style() = style.clone();
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
                if let Some(image) = self.image_frame(*hash, data.as_ref()) {
                    element = element.child(
                        img(image.clone())
                            .size_full()
                            .grayscale(image_style.grayscale)
                            .object_fit(primitive_object_fit(image_style.object_fit)),
                    );
                } else if let Some(child) =
                    Self::image_state(*loading, *fallback, state_children, false)
                {
                    // the loading placeholder is an ordinary guest node, laid
                    // out and clipped inside this image's box
                    element = element.child(self.node(child, window, cx));
                }
            }
        }
        self.primitive_interactivity(element, node, cx)
    }

    /// An Image's or Svg's box as the guest styled and wired it; its role
    /// and name come through `accessible` (`guest_aria`), so a labelled
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
        let mut element = element;
        if let Some(group) = &interactivity.group {
            element = element.group(group.clone());
        }
        if let Some(style) = &interactivity.hover {
            let style = style.clone();
            element = element.hover(move |_| style);
        }
        if let Some(group) = &interactivity.group_hover {
            let style = group.style.clone();
            element = element.group_hover(group.group.clone(), move |_| style);
        }
        let native_id = id.as_ref().map(native_id).unwrap_or_else(|| {
            let index = self.render_index;
            self.render_index += 1;
            ElementId::NamedInteger("guest-primitive".into(), index)
        });
        let mut element = element.id(native_id);
        if let Some(style) = &interactivity.active {
            let style = style.clone();
            element = element.active(move |_| style);
        }
        if let Some(group) = &interactivity.group_active {
            let style = group.style.clone();
            element = element.group_active(group.group.clone(), move |_| style);
        }
        element = self.guest_aria(element, node, interactivity, cx);
        element.into_any_element()
    }
}

fn primitive_object_fit(fit: wire::ImageObjectFit) -> ObjectFit {
    match fit {
        wire::ImageObjectFit::Fill => ObjectFit::Fill,
        wire::ImageObjectFit::Contain => ObjectFit::Contain,
        wire::ImageObjectFit::Cover => ObjectFit::Cover,
        wire::ImageObjectFit::ScaleDown => ObjectFit::ScaleDown,
        wire::ImageObjectFit::None => ObjectFit::None,
    }
}

fn safe_asset_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
