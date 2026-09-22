use super::picture_resources::{cache_fits, decode_image};
use super::*;

#[derive(Clone, Default)]
pub(super) struct ViewerState {
    pub(super) scale: f32,
    pub(super) offset: Point<Pixels>,
    pub(super) drag: Option<Point<Pixels>>,
}

pub(super) fn qr(code: &wire::Qr) -> AnyElement {
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
    let cell = code.cell.map(rgba).unwrap_or_else(|| rgb(0).into());
    let background = code
        .background
        .map(rgba)
        .unwrap_or_else(|| rgb(0xffffff).into());
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
            id,
            source,
            transformation,
            style,
            interactivity,
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
                    svg()
                        .data(bytes)
                        .with_transformation(native_transform)
                        .size_full(),
                )),
                None => element.child("SVG data unavailable"),
            },
            wire::SvgSource::Asset(path) if safe_asset_path(path) => {
                element.child(guarded_svg_paint(
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
        self.primitive_interactivity(element, id.as_ref(), interactivity, cx)
    }

    pub(super) fn drawing(&mut self, node: &wire::Node, cx: &mut Context<Self>) -> AnyElement {
        let wire::Node::Canvas { commands, style } = node else {
            unreachable!()
        };
        let mut root = div().relative().overflow_hidden();
        *root.style() = style.clone();
        if native_canvas_commands(commands) {
            let commands = commands.clone();
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
                .child(self.measure(&self.authored_path, cx))
                .into_any_element();
        }
        let bounds = self.bounds.get(&self.authored_path).copied().unwrap_or_default();
        let width = f32::from(bounds.size.width).max(1.);
        let height = f32::from(bounds.size.height).max(1.);
        root.child(guarded_svg_paint(
            img(Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                canvas_svg(commands, width, height),
            )))
            .size_full()
            .object_fit(ObjectFit::Fill),
        ))
        .child(self.measure(&self.authored_path, cx))
        .into_any_element()
    }

    fn image_state<'a>(
        loading: bool,
        fallback: bool,
        children: &'a [wire::Node],
        want_fallback: bool,
    ) -> Option<&'a wire::Node> {
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
        // Keep accepted hashes stable: eviction would break hash-only frames.
        // Further resources use the existing refusal/fallback path at capacity.
        if !cache_fits(self.vectors.len(), used, bytes.len()) {
            return;
        }
        self.vectors.insert(hash, Arc::from(bytes));
    }

    pub(super) fn image_viewer(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::ImageViewer {
            key,
            hash,
            data,
            width,
            height,
            options,
            ..
        } = node
        else {
            unreachable!()
        };
        if let Some(data) = data {
            self.remember_image(*hash, data);
        }
        let frame = self.image_frame(*hash, data.as_ref());
        let path = self.authored_path.clone();
        let viewer = self.viewers.entry(path.clone()).or_default();
        if viewer.scale == 0.0 {
            viewer.scale = 1.0;
        }
        let mut element = announce(
            dimensions(div().relative().overflow_hidden(), *width, *height).id(key.clone()),
            accessible(node),
        );
        if let Some(image) = frame {
            let original = image.size(0);
            let viewport = self
                .bounds
                .get(&path)
                .map_or(window.viewport_size(), |bounds| bounds.size);
            let inset = options.padding.unwrap_or_default() * 2.0;
            let ratio = ((f32::from(viewport.width) - inset) / u32::from(original.width) as f32)
                .min((f32::from(viewport.height) - inset) / u32::from(original.height) as f32)
                .max(0.0);
            let width = u32::from(original.width) as f32 * ratio * viewer.scale;
            let height = u32::from(original.height) as f32 * ratio * viewer.scale;
            let x = (f32::from(viewport.width) - width) / 2.0 + f32::from(viewer.offset.x);
            let y = (f32::from(viewport.height) - height) / 2.0 + f32::from(viewer.offset.y);
            if !matches!(data, Some(wire::ImageData::Resource(_))) {
                element = element.child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(width))
                        .h(px(height))
                        .child(img(image.clone())),
                );
            }
        }
        let (minimum, maximum) = options.scale_bounds.unwrap_or((0.25, 10.0));
        let step = options.scale_step.unwrap_or(0.1);
        let wheel_key = path.clone();
        element =
            element.on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                let delta = match event.delta {
                    ScrollDelta::Pixels(delta) => f32::from(delta.y),
                    ScrollDelta::Lines(delta) => delta.y,
                };
                let Some(viewer) = this.viewers.get_mut(&wheel_key) else {
                    return;
                };
                viewer.scale =
                    (viewer.scale * (1.0 + step).powf(delta.signum())).clamp(minimum, maximum);
                cx.stop_propagation();
                cx.notify();
            }));
        let down_key = path.clone();
        element = element.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if let Some(viewer) = this.viewers.get_mut(&down_key) {
                    viewer.drag = Some(event.position);
                }
                cx.stop_propagation();
            }),
        );
        let move_key = path.clone();
        element = element.on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
            let Some(viewer) = this.viewers.get_mut(&move_key) else {
                return;
            };
            let Some(previous) = viewer.drag else {
                return;
            };
            if event.pressed_button != Some(MouseButton::Left) {
                viewer.drag = None;
                return;
            }
            viewer.offset += event.position - previous;
            viewer.drag = Some(event.position);
            cx.notify();
        }));
        let up_key = path.clone();
        element = element.on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, _, _| {
                if let Some(viewer) = this.viewers.get_mut(&up_key) {
                    viewer.drag = None;
                }
            }),
        );
        element.child(self.measure(&path, cx)).into_any_element()
    }

    pub(super) fn picture(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Image {
            id,
            hash,
            data,
            image_style,
            loading,
            fallback,
            state_children,
            style,
            interactivity,
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
                    // State recipes remain ordinary guest nodes and therefore stay inside the slot clip.
                    element = element.child(self.node(child, window, cx));
                }
            }
        }
        self.primitive_interactivity(element, id.as_ref(), interactivity, cx)
    }

    fn primitive_interactivity(
        &mut self,
        element: Div,
        id: Option<&wire::ElementIdWire>,
        interactivity: &wire::Interactivity,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let Some(id) = id else {
            return element.into_any_element();
        };
        let mut element = element.id(id.to_gpui().expect("sanitized portable primitive ID"));
        if let Some(style) = &interactivity.active {
            let style = style.clone();
            element = element.active(move |_| style);
        }
        if let Some(group) = &interactivity.group_active {
            let style = group.style.clone();
            element = element.group_active(group.group.clone(), move |_| style);
        }
        if let Some(role) = interactivity.role {
            element = element.role(role);
        }
        if interactivity.focusable {
            element = element.focusable();
        }
        if let Some(value) = &interactivity.aria.author_id {
            element = element.accessibility_id(value.clone());
        }
        if let Some(value) = &interactivity.aria.label {
            element = element.aria_label(value.clone());
        }
        if let Some(value) = &interactivity.aria.description {
            element = element.aria_description(value.clone());
        }
        if let Some(value) = &interactivity.aria.keyshortcuts {
            element = element.aria_keyshortcuts(value.clone());
        }
        if let Some(value) = &interactivity.aria.value {
            element = element.aria_value(value.clone());
        }
        if let Some(value) = &interactivity.aria.placeholder {
            element = element.aria_placeholder(value.clone());
        }
        if let Some(value) = interactivity.aria.selected {
            element = element.aria_selected(value);
        }
        if let Some(value) = interactivity.aria.expanded {
            element = element.aria_expanded(value);
        }
        if let Some(value) = interactivity.aria.disabled {
            element = element.aria_disabled(value);
        }
        if let Some(value) = interactivity.aria.numeric_value {
            element = element.aria_numeric_value(value);
        }
        if let Some(value) = interactivity.aria.numeric_value_step {
            element = element.aria_numeric_value_step(value);
        }
        if let Some(value) = interactivity.aria.min_numeric_value {
            element = element.aria_min_numeric_value(value);
        }
        if let Some(value) = interactivity.aria.max_numeric_value {
            element = element.aria_max_numeric_value(value);
        }
        if let Some(value) = interactivity.aria.level {
            element = element.aria_level(value);
        }
        if let Some(value) = interactivity.aria.position_in_set {
            element = element.aria_position_in_set(value);
        }
        if let Some(value) = interactivity.aria.size_of_set {
            element = element.aria_size_of_set(value);
        }
        if let Some(value) = interactivity.aria.row_index {
            element = element.aria_row_index(value);
        }
        if let Some(value) = interactivity.aria.column_index {
            element = element.aria_column_index(value);
        }
        if let Some(value) = interactivity.aria.row_count {
            element = element.aria_row_count(value);
        }
        if let Some(value) = interactivity.aria.column_count {
            element = element.aria_column_count(value);
        }
        if let Some(value) = interactivity.aria.toggled {
            element = element.aria_toggled(value);
        }
        if let Some(value) = interactivity.aria.orientation {
            element = element.aria_orientation(value);
        }
        if let Some(handler) = interactivity.on_click {
            element = element.on_click(cx.listener(
                move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    this.user_activation.set(Some(handler));
                    cx.emit(wire::Event::Click {
                        handler,
                        event: event.into(),
                    });
                },
            ));
        }
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
