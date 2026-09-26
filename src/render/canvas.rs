use super::*;

pub(super) fn native_canvas_commands(commands: &[wire::CanvasCommand]) -> bool {
    commands.iter().all(|command| {
        let wire::CanvasCommand::Draw { shape, stroke, .. } = command else {
            return false;
        };
        let solid = stroke.as_ref().is_none_or(|s| s.dash.is_empty());
        let primitive = match shape {
            wire::CanvasShape::Rectangle { radius, .. } => radius.iter().all(|r| *r == radius[0]),
            wire::CanvasShape::Circle { .. } => true,
            wire::CanvasShape::Line { .. } => stroke
                .as_ref()
                .is_none_or(|s| s.cap != wire::CanvasLineCap::Square),
            wire::CanvasShape::Path(_) => false,
        };
        solid && primitive
    })
}

pub(super) fn paint_canvas_commands(
    commands: &[wire::CanvasCommand],
    origin: Point<Pixels>,
    window: &mut Window,
) {
    for command in commands {
        let wire::CanvasCommand::Draw {
            shape,
            fill: background,
            stroke,
            ..
        } = command
        else {
            continue;
        };
        let position = |p: [f32; 2]| origin + point(px(p[0]), px(p[1]));
        let (bounds, radius) = match shape {
            wire::CanvasShape::Rectangle {
                position: p,
                size: s,
                radius,
            } => (
                Bounds::new(position(*p), size(px(s[0]), px(s[1]))),
                radius[0],
            ),
            wire::CanvasShape::Circle { center, radius } => (
                Bounds::new(
                    position([center[0] - radius, center[1] - radius]),
                    size(px(2. * radius), px(2. * radius)),
                ),
                *radius,
            ),
            wire::CanvasShape::Line { from, to } => {
                if let Some(stroke) = stroke {
                    let mut path = gpui_kit::PathBuilder::stroke(px(stroke.width));
                    path.move_to(position(*from));
                    path.line_to(position(*to));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, stroke.color);
                    }
                    if stroke.cap == wire::CanvasLineCap::Round {
                        let r = stroke.width / 2.;
                        for p in [from, to] {
                            window.paint_quad(
                                fill(
                                    Bounds::new(
                                        position([p[0] - r, p[1] - r]),
                                        size(px(2. * r), px(2. * r)),
                                    ),
                                    stroke.color,
                                )
                                .corner_radii(px(r)),
                            );
                        }
                    }
                }
                continue;
            }
            wire::CanvasShape::Path(_) => continue,
        };
        // SVG strokes straddle the geometry; native quad borders are inset.
        let border = stroke.as_ref().map_or(0., |s| s.width);
        let expanded = Bounds::new(
            bounds.origin - point(px(border / 2.), px(border / 2.)),
            bounds.size + size(px(border), px(border)),
        );
        let color = background.unwrap_or_default();
        let border_color = stroke.as_ref().map(|s| s.color).unwrap_or_default();
        window.paint_quad(
            fill(expanded, color)
                .corner_radii(px(radius + border / 2.))
                .border_widths(px(border))
                .border_color(border_color),
        );
    }
}

pub(super) fn svg_color(color: Hsla) -> String {
    let color = color.to_rgb();
    let (r, g, b, a) = (color.r, color.g, color.b, color.a);
    format!(
        "rgba({},{},{},{a})",
        (r * 255.0) as u8,
        (g * 255.0) as u8,
        (b * 255.0) as u8
    )
}

pub(super) fn canvas_svg(commands: &[wire::CanvasCommand], width: f32, height: f32) -> Vec<u8> {
    use std::fmt::Write;
    let mut svg =
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">");
    let mut depth = 0;
    for (index, command) in commands.iter().enumerate() {
        match command {
            wire::CanvasCommand::Push {
                translate,
                rotate,
                scale,
                clip,
            } => {
                let _ = write!(
                    svg,
                    "<g transform=\"translate({} {}) rotate({}) scale({} {})\">",
                    translate[0],
                    translate[1],
                    rotate.to_degrees(),
                    scale[0],
                    scale[1]
                );
                if let Some([x, y, w, h]) = clip {
                    let _ = write!(
                        svg,
                        "<defs><clipPath id=\"c{index}\"><rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\"/></clipPath></defs><g clip-path=\"url(#c{index})\">"
                    );
                } else {
                    svg.push_str("<g>");
                }
                depth += 1;
            }
            wire::CanvasCommand::Pop => {
                if depth > 0 {
                    svg.push_str("</g></g>");
                    depth -= 1;
                }
            }
            wire::CanvasCommand::Draw {
                shape,
                fill,
                even_odd,
                stroke,
            } => {
                let path = canvas_path(shape);
                let color = fill.map(svg_color).unwrap_or_else(|| "none".into());
                let rule = match even_odd {
                    true => "evenodd",
                    false => "nonzero",
                };
                let _ = write!(
                    svg,
                    "<path d=\"{path}\" fill=\"{color}\" fill-rule=\"{rule}\""
                );
                if let Some(stroke) = stroke {
                    let cap = match stroke.cap {
                        wire::CanvasLineCap::Butt => "butt",
                        wire::CanvasLineCap::Square => "square",
                        wire::CanvasLineCap::Round => "round",
                    };
                    let join = match stroke.join {
                        wire::CanvasLineJoin::Miter => "miter",
                        wire::CanvasLineJoin::Round => "round",
                        wire::CanvasLineJoin::Bevel => "bevel",
                    };
                    let dash = stroke
                        .dash
                        .iter()
                        .map(f32::to_string)
                        .collect::<Vec<_>>()
                        .join(" ");
                    let _ = write!(
                        svg,
                        " stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"{cap}\" stroke-linejoin=\"{join}\" stroke-dasharray=\"{dash}\" stroke-dashoffset=\"{}\"",
                        svg_color(stroke.color),
                        stroke.width,
                        stroke.dash_offset
                    );
                }
                svg.push_str("/>");
            }
        }
    }
    for _ in 0..depth {
        svg.push_str("</g></g>");
    }
    svg.push_str("</svg>");
    svg.into_bytes()
}

pub(super) fn canvas_path(shape: &wire::CanvasShape) -> String {
    let segments = match shape {
        wire::CanvasShape::Rectangle {
            position,
            size,
            radius,
        } => vec![wire::CanvasSegment::Rectangle {
            position: *position,
            size: *size,
            radius: *radius,
        }],
        wire::CanvasShape::Circle { center, radius } => vec![wire::CanvasSegment::Circle {
            center: *center,
            radius: *radius,
        }],
        wire::CanvasShape::Line { from, to } => vec![
            wire::CanvasSegment::Move(*from),
            wire::CanvasSegment::Line(*to),
        ],
        wire::CanvasShape::Path(segments) => segments.clone(),
    };
    let mut path = String::new();
    let mut cursor = [0.0, 0.0];
    let mut origin = cursor;
    use std::fmt::Write;
    for segment in segments {
        match segment {
            wire::CanvasSegment::Move([x, y]) => {
                let _ = write!(path, "M{x} {y} ");
                cursor = [x, y];
                origin = cursor;
            }
            wire::CanvasSegment::Line([x, y]) => {
                let _ = write!(path, "L{x} {y} ");
                cursor = [x, y];
            }
            wire::CanvasSegment::Close => {
                path.push_str("Z ");
                cursor = origin;
            }
            wire::CanvasSegment::Rectangle {
                position: [x, y],
                size: [w, h],
                radius: [tl, tr, br, bl],
            } => {
                let _ = write!(
                    path,
                    "M{} {y} H{} Q{} {y} {} {} V{} Q{} {} {} {} H{} Q{x} {} {x} {} V{} Q{x} {y} {} {y} Z ",
                    x + tl,
                    x + w - tr,
                    x + w,
                    x + w,
                    y + tr,
                    y + h - br,
                    x + w,
                    y + h,
                    x + w - br,
                    y + h,
                    x + bl,
                    y + h,
                    y + h - bl,
                    y + tl,
                    x + tl
                );
            }
            wire::CanvasSegment::Circle {
                center: [x, y],
                radius,
            } => {
                let _ = write!(
                    path,
                    "M{} {y} a{radius} {radius} 0 1 0 {} 0 a{radius} {radius} 0 1 0 {} 0 Z ",
                    x - radius,
                    radius * 2.0,
                    -radius * 2.0
                );
            }
            wire::CanvasSegment::Bezier { a, b, end } => {
                let _ = write!(
                    path,
                    "C{} {} {} {} {} {} ",
                    a[0], a[1], b[0], b[1], end[0], end[1]
                );
                cursor = end;
            }
            wire::CanvasSegment::Quadratic { control, end } => {
                let _ = write!(
                    path,
                    "Q{} {} {} {} ",
                    control[0], control[1], end[0], end[1]
                );
                cursor = end;
            }
            wire::CanvasSegment::Arc {
                center,
                radius,
                start,
                end,
            } => {
                append_arc(&mut path, center, [radius, radius], 0.0, start, end);
                cursor = [
                    center[0] + radius * end.cos(),
                    center[1] + radius * end.sin(),
                ];
            }
            wire::CanvasSegment::Ellipse {
                center,
                radius,
                rotation,
                start,
                end,
            } => {
                append_arc(&mut path, center, radius, rotation, start, end);
                cursor = [
                    center[0] + radius[0] * end.cos() * rotation.cos()
                        - radius[1] * end.sin() * rotation.sin(),
                    center[1]
                        + radius[0] * end.cos() * rotation.sin()
                        + radius[1] * end.sin() * rotation.cos(),
                ];
            }
            wire::CanvasSegment::ArcTo { a, b, radius } => {
                cursor = append_arc_to(&mut path, cursor, a, b, radius);
            }
        }
    }
    path
}

pub(super) fn append_arc(
    path: &mut String,
    center: [f32; 2],
    radius: [f32; 2],
    rotation: f32,
    start: f32,
    end: f32,
) {
    use std::fmt::Write;
    let point = |angle: f32| {
        let x = radius[0] * angle.cos();
        let y = radius[1] * angle.sin();
        [
            center[0] + x * rotation.cos() - y * rotation.sin(),
            center[1] + x * rotation.sin() + y * rotation.cos(),
        ]
    };
    let a = point(start);
    let sweep = i32::from(end >= start);
    let _ = write!(path, "M{} {} ", a[0], a[1]);
    let steps = ((end - start).abs() / std::f32::consts::PI)
        .ceil()
        .clamp(1.0, 128.0) as usize;
    for index in 1..=steps {
        let b = point(start + (end - start) * index as f32 / steps as f32);
        let _ = write!(
            path,
            "A{} {} {} 0 {sweep} {} {} ",
            radius[0],
            radius[1],
            rotation.to_degrees(),
            b[0],
            b[1]
        );
    }
}

pub(super) fn append_arc_to(
    path: &mut String,
    current: [f32; 2],
    a: [f32; 2],
    b: [f32; 2],
    radius: f32,
) -> [f32; 2] {
    use std::fmt::Write;
    let from = [current[0] - a[0], current[1] - a[1]];
    let to = [b[0] - a[0], b[1] - a[1]];
    let first = from[0].hypot(from[1]);
    let second = to[0].hypot(to[1]);
    let cross = from[0] * to[1] - from[1] * to[0];
    let degenerate = radius <= 0.0
        || first < f32::EPSILON
        || second < f32::EPSILON
        || cross.abs() < f32::EPSILON;
    if degenerate {
        let _ = write!(path, "L{} {} ", a[0], a[1]);
        return a;
    }
    let from = [from[0] / first, from[1] / first];
    let to = [to[0] / second, to[1] / second];
    let angle = (from[0] * to[0] + from[1] * to[1]).clamp(-1.0, 1.0).acos();
    let distance = radius / (angle / 2.0).tan();
    let enter = [a[0] + from[0] * distance, a[1] + from[1] * distance];
    let exit = [a[0] + to[0] * distance, a[1] + to[1] * distance];
    let sweep = i32::from(cross < 0.0);
    let _ = write!(
        path,
        "L{} {} A{radius} {radius} 0 0 {sweep} {} {} ",
        enter[0], enter[1], exit[0], exit[1]
    );
    exit
}
