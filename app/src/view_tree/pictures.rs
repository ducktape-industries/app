use super::*;

#[derive(Clone, Default)]
pub(super) struct ViewerState {
    pub(super) scale: f32,
    pub(super) offset: Point<Pixels>,
    pub(super) drag: Option<Point<Pixels>>,
}

pub(super) fn decode_image(data: &wire::ImageData) -> Option<RenderImage> {
    let mut pixels = match data {
        wire::ImageData::Resource(_) => return None,
        wire::ImageData::Rgba {
            width,
            height,
            pixels,
        } => image::RgbaImage::from_raw(*width, *height, pixels.clone())?,
        wire::ImageData::Encoded(bytes) => {
            let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .ok()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(64 << 20);
            reader.limits(limits);
            reader.decode().ok()?.into_rgba8()
        }
    };
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(RenderImage::new(vec![image::Frame::new(pixels)]))
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
    pub(super) fn vector(&mut self, node: &wire::Node, window: &mut Window) -> AnyElement {
        let wire::Node::Svg {
            key,
            hash,
            bytes,
            color,
            inherit_button_ink,
            fit,
            width,
            height,
            opacity,
            ..
        } = node
        else {
            unreachable!()
        };
        if let Some(bytes) = bytes {
            self.remember_vector(*hash, bytes);
        }
        let mut element = dimensions(div(), *width, *height).opacity(opacity.unwrap_or(1.0));
        if let Some(bytes) = self.vectors.get(hash) {
            let monochrome = color.is_some() || *inherit_button_ink;
            if monochrome {
                let ink = color.map(rgba).unwrap_or_else(|| window.text_style().color);
                element = element.child(svg().data(bytes).size_full().text_color(ink));
            } else {
                // GPUI's SVG icon renderer is an alpha mask. Untinted
                // artwork instead uses its native full-color image decoder.
                element = element.child(
                    img(Arc::new(Image::from_bytes(
                        ImageFormat::Svg,
                        bytes.to_vec(),
                    )))
                    .size_full()
                    .object_fit(object_fit(*fit)),
                );
            }
        }
        announce(element.id(key.clone()), accessible(node)).into_any_element()
    }

    pub(super) fn drawing(&mut self, node: &wire::Node, cx: &mut Context<Self>) -> AnyElement {
        let wire::Node::Canvas {
            key,
            width,
            height,
            commands,
            ..
        } = node
        else {
            unreachable!()
        };
        // Primitive canvases paint in the current native frame. Recreating an
        // asynchronous SVG image on every pointer move leaves blank drag frames.
        if native_canvas_commands(commands) {
            let commands = commands.clone();
            return dimensions(div().relative().overflow_hidden(), *width, *height)
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            paint_canvas_commands(&commands, bounds.origin, window);
                        },
                    )
                    .size_full(),
                )
                .child(self.measure(key, cx))
                .into_any_element();
        }
        let bounds = self.bounds.get(key).copied().unwrap_or_default();
        let known = |length: Option<wire::Length>, measured: Pixels| match length {
            Some(wire::Length::Fixed(value)) => value,
            _ => f32::from(measured).max(1.0),
        };
        dimensions(div().relative(), *width, *height)
            .child(
                img(Arc::new(Image::from_bytes(
                    ImageFormat::Svg,
                    canvas_svg(
                        commands,
                        known(*width, bounds.size.width),
                        known(*height, bounds.size.height),
                    ),
                )))
                .size_full()
                .object_fit(ObjectFit::Fill),
            )
            .child(self.measure(key, cx))
            .into_any_element()
    }

    pub(super) fn remember_image(&mut self, hash: u64, data: &wire::ImageData) {
        if matches!(data, wire::ImageData::Resource(_)) {
            return;
        }
        if self.images.contains_key(&hash) {
            return;
        }
        let Some(image) = decode_image(data) else {
            return;
        };
        self.images.insert(hash, Arc::new(image));
    }

    pub(super) fn image_frame(
        &self,
        hash: u64,
        data: Option<&wire::ImageData>,
    ) -> Option<Arc<RenderImage>> {
        match data {
            Some(wire::ImageData::Resource(_)) => None,
            _ => self.images.get(&hash).cloned(),
        }
    }

    pub(super) fn remember_vector(&mut self, hash: u64, bytes: &[u8]) {
        if self.vectors.contains_key(&hash) {
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
        let viewer = self.viewers.entry(key.clone()).or_default();
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
                .get(key)
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
        let wheel_key = key.clone();
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
        let down_key = key.clone();
        element = element.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if let Some(viewer) = this.viewers.get_mut(&down_key) {
                    viewer.drag = Some(event.position);
                }
                cx.stop_propagation();
            }),
        );
        let move_key = key.clone();
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
        let up_key = key.clone();
        element = element.on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, _, _| {
                if let Some(viewer) = this.viewers.get_mut(&up_key) {
                    viewer.drag = None;
                }
            }),
        );
        element.child(self.measure(key, cx)).into_any_element()
    }

    pub(super) fn picture(&mut self, node: &wire::Node, cx: &mut Context<Self>) -> AnyElement {
        let wire::Node::Image {
            key,
            hash,
            data,
            width,
            height,
            fit,
            opacity,
            ..
        } = node
        else {
            unreachable!()
        };
        if let Some(data) = data {
            self.remember_image(*hash, data);
        }
        let mut element = dimensions(div(), *width, *height).opacity(opacity.unwrap_or(1.0));
        match data {
            Some(wire::ImageData::Resource(_)) => {
                element = element.child(self.measure(key, cx));
            }
            _ => {
                if let Some(image) = self.image_frame(*hash, data.as_ref()) {
                    element =
                        element.child(img(image.clone()).size_full().object_fit(object_fit(*fit)));
                }
            }
        }
        announce(element.id(key.clone()), accessible(node)).into_any_element()
    }
}
