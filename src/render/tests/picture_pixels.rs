//! Pictures as a real renderer draws them: wgpu's headless renderer (on a
//! box with no GPU, Mesa's software Vulkan) draws the scene the window
//! painted, and the test reads its pixels back.
use super::*;
use gpui_kit::{HeadlessAppContext, PlatformHeadlessRenderer};

/// A square filling its whole box.
const SQUARE: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><path d="M0 0h24v24H0z"/></svg>"#;
const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// An app whose windows draw through wgpu's headless renderer, so its
/// atlas is a GPU's and `capture_screenshot` reads back what it drew.
pub(in crate::render) fn rendering_app() -> HeadlessAppContext {
    let text = Arc::new(gpui_wgpu::CosmicTextSystem::new_without_system_fonts(
        "fallback",
    ));
    let mut cx = HeadlessAppContext::with_platform(text, Arc::new(()), || {
        let renderer: Box<dyn PlatformHeadlessRenderer> =
            Box::new(gpui_wgpu::WgpuHeadlessRenderer::new()?);
        Ok(Some(renderer))
    });
    cx.update(gpui_kit::init);
    cx
}

/// A 30px guest Svg of [`SQUARE`] with `style` on top.
fn glyph(style: gpui_kit::Div) -> wire::Node {
    wire::Node::Svg {
        id: Some(named_id("glyph")),
        source: wire::SvgSource::Data {
            hash: 1,
            bytes: Some(SQUARE.to_vec()),
        },
        transformation: wire::SvgTransformation {
            scale: [1.0, 1.0],
            translate: [0.0, 0.0],
            rotate: 0.0,
        },
        label: None,
        style: crate::render::test_style(style.size(px(30.)).style().clone()),
        interactivity: Default::default(),
    }
}

/// `child` at the top left of a white 60px page, read back as the
/// renderer drew it: the pixel at the middle of the glyph's box, and one
/// outside it.
fn painted(page: gpui_kit::Div, child: wire::Node) -> ([u8; 4], [u8; 4]) {
    let mut cx = rendering_app();
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("page")),
        style: crate::render::test_style(page.size_full().bg(rgb(0xffffff)).style().clone()),
        interactivity: Default::default(),
        children: vec![child],
    });
    let window = cx
        .open_window(size(px(60.), px(60.)), |_, cx| {
            cx.new(|cx| Seat(cx.new(|_| ViewTree::new(root))))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    let image = cx.capture_screenshot(window.into()).unwrap();
    let scale = image.width() / 60;
    let at = |x: u32, y: u32| image.get_pixel(x * scale, y * scale).0;
    (at(15, 15), at(45, 45))
}

fn assert_near(actual: [u8; 4], expected: [u8; 4], what: &str) {
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 3),
        "{what}: {actual:?}, expected {expected:?}"
    );
}

/// A guest Svg with its own colour draws in it.
#[test]
fn a_guest_svg_paints_in_its_own_colour() {
    let (glyph, outside) = painted(div(), glyph(div().text_color(rgb(0xff0000))));
    assert_near(outside, WHITE, "the page around the glyph");
    assert_near(glyph, RED, "the glyph's middle");
}

/// A guest Svg with no colour of its own draws in the colour it inherits
/// (CSS `currentColor`), as text does, not nothing.
#[test]
fn a_guest_svg_with_no_colour_paints_in_the_one_it_inherits() {
    let (glyph, outside) = painted(div().text_color(rgb(0x0000ff)), glyph(div()));
    assert_near(outside, WHITE, "the page around the glyph");
    assert_near(glyph, BLUE, "the glyph's middle");
}

/// A guest's gradient background crosses the host's sanitizer and paints
/// as gpui draws one: red at its left, blue at its right, where the
/// sanitizer once dropped it and left the page white.
#[test]
fn a_guest_gradient_background_paints() {
    let gradient = gpui_kit::linear_gradient(
        90.,
        gpui_kit::linear_color_stop(rgb(0xff0000), 0.),
        gpui_kit::linear_color_stop(rgb(0x0000ff), 1.),
    );
    let mut frame = wire::Frame {
        root: Some(wire::Node::Container(view_wire::ContainerNode {
            id: Some(named_id("gradient")),
            style: crate::render::test_style(div().size_full().bg(gradient).style().clone()),
            interactivity: Default::default(),
            children: vec![],
        })),
        ..wire::Frame::default()
    };
    crate::render::sanitize_whole(&mut frame).unwrap();
    let (left, right) = painted(div(), frame.root.unwrap());
    assert!(
        left[0] > left[2] && right[2] > right[0],
        "red to blue, left to right: {left:?} then {right:?}"
    );
}

/// The canvas painter as it was before a run of quads took one draw order:
/// every command painted by itself, each with a draw order of its own.
fn painted_one_by_one(
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
                                gpui_kit::fill(
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
            gpui_kit::fill(expanded, color)
                .corner_radii(px(radius + border / 2.))
                .border_widths(px(border))
                .border_color(border_color),
        );
    }
}

type Painter = fn(&[wire::CanvasCommand], Point<Pixels>, &mut Window);

/// `commands` on a white page, painted by `painter`.
struct Painted(Rc<[wire::CanvasCommand]>, Painter);

impl Render for Painted {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (commands, painter) = (self.0.clone(), self.1);
        div().size_full().bg(rgb(0xffffff)).child(
            gpui_kit::canvas(
                |_, _, _| (),
                move |bounds, _, window, _| painter(&commands, bounds.origin, window),
            )
            .size_full(),
        )
    }
}

/// `commands` as the renderer draws what `painter` paints, 240 by 140.
fn picture(commands: &[wire::CanvasCommand], painter: Painter) -> image::RgbaImage {
    let mut cx = rendering_app();
    let commands: Rc<[wire::CanvasCommand]> = commands.into();
    let window = cx
        .open_window(size(px(240.), px(140.)), |_, cx| {
            cx.new(|_| Painted(commands, painter))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    cx.capture_screenshot(window.into()).unwrap()
}

fn filled(shape: wire::CanvasShape, color: gpui_kit::Rgba) -> wire::CanvasCommand {
    wire::CanvasCommand::Draw {
        shape,
        fill: Some(color.into()),
        stroke: None,
        even_odd: false,
    }
}

fn stroked(
    shape: wire::CanvasShape,
    fill: Option<gpui_kit::Rgba>,
    color: gpui_kit::Rgba,
    width: f32,
    cap: wire::CanvasLineCap,
) -> wire::CanvasCommand {
    wire::CanvasCommand::Draw {
        shape,
        fill: fill.map(Into::into),
        stroke: Some(wire::CanvasStroke {
            color: color.into(),
            width,
            cap,
            join: wire::CanvasLineJoin::Miter,
            dash: Vec::new(),
            dash_offset: 0,
        }),
        even_odd: false,
    }
}

fn rectangle(x: f32, y: f32, width: f32, height: f32) -> wire::CanvasShape {
    wire::CanvasShape::Rectangle {
        position: [x, y],
        size: [width, height],
        radius: [0.; 4],
    }
}

/// The quads of a chart as a view draws one: rules, then for each candle a
/// wick, a body wider than the step (neighbours overlap) and a half-clear
/// volume bar, all at fractions of a pixel; ringed half-clear discs and a
/// rounded box over them; and rules laid over everything.
fn chart(candles: usize) -> Vec<wire::CanvasCommand> {
    let grey = gpui_kit::rgba(0x80808080);
    let mut commands = Vec::new();
    for rule in 0..4 {
        commands.push(filled(
            rectangle(0., 20. + 30. * rule as f32, 240., 1.),
            grey,
        ));
    }
    // a small deterministic scatter
    let mut seed = 7_u32;
    let mut next = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1 << 24) as f32
    };
    let step = 220. / candles as f32;
    for slot in 0..candles {
        let color = match slot % 3 {
            0 => gpui_kit::rgba(0xd03030ff),
            _ => gpui_kit::rgba(0x20a060ff),
        };
        let middle = 8.3 + step * slot as f32;
        let (high, low) = (10. + 60. * next(), 70. + 40. * next());
        let (top, bottom) = (high + 12. * next(), low - 12. * next());
        commands.push(filled(rectangle(middle, high, 1., low - high), color));
        commands.push(filled(
            rectangle(
                middle - 2.6,
                top,
                step.max(1.) + 1.7,
                (bottom - top).max(1.),
            ),
            color,
        ));
        let mut clear = color;
        clear.a = 0.4;
        let volume = 25. * next();
        commands.push(filled(
            rectangle(middle - 2.6, 135. - volume, step + 1.7, volume),
            clear,
        ));
    }
    for disc in 0..5 {
        commands.push(stroked(
            wire::CanvasShape::Circle {
                center: [40.5 + 17. * disc as f32, 50.25 + 3. * disc as f32],
                radius: 12.,
            },
            Some(gpui_kit::rgba(0xffcc0080)),
            gpui_kit::rgba(0x000000c0),
            2.5,
            wire::CanvasLineCap::Butt,
        ));
    }
    commands.push(filled(
        wire::CanvasShape::Rectangle {
            position: [150.5, 30.5],
            size: [60., 40.],
            radius: [9.; 4],
        },
        gpui_kit::rgba(0x3060d080),
    ));
    for rule in 0..3 {
        commands.push(filled(
            rectangle(0., 44.5 + 21. * rule as f32, 240., 1.),
            gpui_kit::rgba(0x2040ff80),
        ));
    }
    commands
}

/// The pixels of two pictures that differ: where, and both colours.
fn difference(
    one: &image::RgbaImage,
    other: &image::RgbaImage,
) -> Vec<(u32, u32, [u8; 4], [u8; 4])> {
    assert_eq!(one.dimensions(), other.dimensions());
    one.enumerate_pixels()
        .zip(other.pixels())
        .filter(|((_, _, a), b)| a != b)
        .map(|((x, y, a), b)| (x, y, a.0, b.0))
        .collect()
}

/// A line painted before a run of quads lies under every quad of the run,
/// the far ones too: the run's layer is the box around all of it, not
/// around the quad that begins it.
#[test]
fn a_line_before_a_run_lies_under_every_quad_of_it() {
    let yellow = gpui_kit::rgba(0xffcc00ff);
    let commands = [
        stroked(
            wire::CanvasShape::Line {
                from: [10., 70.],
                to: [230., 70.],
            },
            None,
            gpui_kit::rgba(0x000000ff),
            4.,
            wire::CanvasLineCap::Butt,
        ),
        filled(rectangle(10., 10., 20., 20.), yellow),
        filled(rectangle(100., 50., 40., 40.), yellow),
    ];
    let before = picture(&commands, painted_one_by_one);
    let after = picture(&commands, crate::render::canvas::paint_canvas_commands);
    let scale = after.width() / 240;
    let at = |x: u32, y: u32| after.get_pixel(x * scale, y * scale).0;
    assert_near(
        at(120, 70),
        [0xff, 0xcc, 0x00, 255],
        "the box over the line",
    );
    assert_near(at(60, 70), [0, 0, 0, 255], "the line beside the box");
    assert_eq!(
        difference(&before, &after),
        Vec::new(),
        "the pixels that differ"
    );
}

/// A run of rectangles and circles under one draw order is the picture the
/// same commands were with a draw order each: overlapping candles, discs
/// with rings, half-clear fills, at two densities.
#[test]
fn quads_under_one_draw_order_are_the_picture_they_were_one_by_one() {
    for candles in [40, 160] {
        let commands = chart(candles);
        let before = picture(&commands, painted_one_by_one);
        let after = picture(&commands, crate::render::canvas::paint_canvas_commands);
        let ink = before.pixels().filter(|pixel| pixel.0 != WHITE).count();
        assert!(ink > 5_000, "the chart is drawn: {ink} pixels");
        assert_eq!(
            difference(&before, &after),
            Vec::new(),
            "{candles} candles: the pixels that differ"
        );
    }
}

/// Lines between two runs of quads lie over the run before them and under
/// the run after, as they did when every quad had a draw order of its own.
/// Every edge here is on a whole pixel and the lines are solid, so nothing
/// is anti-aliased and the picture is only which shape lies over which.
/// The steps go on from each other, so they are one path: its corners are
/// as open as they were, no join drawn between two steps.
#[test]
fn lines_between_runs_of_quads_lie_over_the_run_before_and_under_the_run_after() {
    let ink = gpui_kit::rgba(0x202020ff);
    let mut commands = vec![
        filled(rectangle(20., 20., 120., 80.), gpui_kit::rgba(0xd03030ff)),
        filled(rectangle(100., 10., 100., 60.), gpui_kit::rgba(0x20a06080)),
    ];
    // a staircase, each step from the last one's end
    let corners = [
        [10., 30.],
        [60., 30.],
        [60., 70.],
        [130., 70.],
        [130., 110.],
        [220., 110.],
    ];
    for ends in corners.windows(2) {
        commands.push(stroked(
            wire::CanvasShape::Line {
                from: ends[0],
                to: ends[1],
            },
            None,
            ink,
            4.,
            wire::CanvasLineCap::Butt,
        ));
    }
    commands.push(filled(
        rectangle(40., 0., 30., 140.),
        gpui_kit::rgba(0x3060d080),
    ));
    commands.push(filled(
        rectangle(0., 100., 240., 20.),
        gpui_kit::rgba(0xffcc00ff),
    ));
    let before = picture(&commands, painted_one_by_one);
    let after = picture(&commands, crate::render::canvas::paint_canvas_commands);
    let scale = before.width() / 240;
    let at = |x: u32, y: u32| before.get_pixel(x * scale, y * scale).0;
    assert_near(
        at(100, 70),
        [0x20, 0x20, 0x20, 255],
        "a step over the red box",
    );
    assert_near(at(180, 110), [0xff, 0xcc, 0x00, 255], "the bar over a step");
    assert_eq!(
        difference(&before, &after),
        Vec::new(),
        "the pixels that differ"
    );
}

/// A line as a chart draws an average: a command a segment, each from the
/// last one's end, at fractions of a pixel.
fn average(
    segments: usize,
    color: gpui_kit::Rgba,
    width: f32,
    cap: wire::CanvasLineCap,
) -> Vec<wire::CanvasCommand> {
    let step = 220. / segments as f32;
    let ends: Vec<[f32; 2]> = (0..=segments)
        .map(|end| {
            [
                8.8 + step * end as f32,
                60.3 + ((end * 7) % 11) as f32 * 2.3,
            ]
        })
        .collect();
    ends.windows(2)
        .map(|ends| {
            stroked(
                wire::CanvasShape::Line {
                    from: ends[0],
                    to: ends[1],
                },
                None,
                color,
                width,
                cap,
            )
        })
        .collect()
}

/// One path with a subpath a line has the triangles its lines have alone,
/// in their order: two a line, no join between two lines. So what a line
/// in one path changes in the picture is the renderer's doing, not the
/// geometry's.
#[test]
fn one_path_has_the_triangles_its_lines_have_alone() {
    let points = [[10.3, 20.7], [14.1, 18.2], [18.9, 19.4], [23.2, 25.1]];
    let triangles = |points: &[[f32; 2]]| {
        let mut path = gpui_kit::PathBuilder::stroke(px(1.25));
        for ends in points.windows(2) {
            path.move_to(point(px(ends[0][0]), px(ends[0][1])));
            path.line_to(point(px(ends[1][0]), px(ends[1][1])));
        }
        let vertices = path.build().unwrap().vertices;
        Vec::from_iter(vertices.into_iter().map(|vertex| vertex.xy_position))
    };
    let alone = Vec::from_iter(points.windows(2).flat_map(triangles));
    assert_eq!(alone.len(), 3 * 2 * 3, "two triangles a line");
    assert_eq!(triangles(&points), alone);
}

/// A butt-capped line alone, as one path, loses no ink it had as a path a
/// segment: no pixel is lighter. (With quads ordered between its segments
/// the old batches were cut elsewhere, and a pixel at a joint can be
/// lighter.) It is not the same picture: the renderer drops a
/// path's pixel whose middle is outside that path's own box, which trims
/// the partly covered pixels at a short segment's edge and not those of
/// the whole line, so the one path adds ink there. How many pixels that is
/// depends on the renderer, so no count is held here.
#[test]
fn a_line_in_one_path_loses_no_ink_its_segments_had() {
    for segments in [40, 160] {
        let commands = average(
            segments,
            gpui_kit::rgba(0x404040ff),
            1.25,
            wire::CanvasLineCap::Butt,
        );
        let before = picture(&commands, painted_one_by_one);
        let after = picture(&commands, crate::render::canvas::paint_canvas_commands);
        let ink = before.pixels().filter(|pixel| pixel.0 != WHITE).count();
        assert!(ink > 500, "the line is drawn: {ink} pixels");
        let mut lighter = difference(&before, &after);
        lighter.retain(|(_, _, before, after)| before.iter().zip(after).any(|(b, a)| a > b));
        assert_eq!(
            lighter,
            Vec::new(),
            "{segments} segments: the pixels that lost ink"
        );
    }
}

/// A line that goes on from the last one's end with another stroke is not
/// of its run: it keeps its own colour and its own width.
#[test]
fn a_line_of_another_stroke_stays_out_of_the_run() {
    let (black, red) = (gpui_kit::rgba(0x000000ff), gpui_kit::rgba(0xff0000ff));
    let line = |from: [f32; 2], to: [f32; 2], color, width| {
        stroked(
            wire::CanvasShape::Line { from, to },
            None,
            color,
            width,
            wire::CanvasLineCap::Butt,
        )
    };
    let commands = [
        line([20., 40.], [120., 40.], black, 4.),
        line([120., 40.], [220., 40.], red, 4.),
        line([20., 80.], [120., 80.], black, 4.),
        line([120., 80.], [220., 80.], black, 8.),
    ];
    let before = picture(&commands, painted_one_by_one);
    let after = picture(&commands, crate::render::canvas::paint_canvas_commands);
    let scale = after.width() / 240;
    let at = |x: u32, y: u32| after.get_pixel(x * scale, y * scale).0;
    assert_near(at(170, 40), RED, "the second line keeps its colour");
    assert_near(
        at(170, 77),
        [0, 0, 0, 255],
        "the wider line keeps its width",
    );
    assert_near(at(70, 77), WHITE, "the narrower line keeps its width");
    assert_eq!(
        difference(&before, &after),
        Vec::new(),
        "the pixels that differ"
    );
}

/// Round-capped lines stay a path each, their caps laid over its ends
/// before the next one's path, so they are the picture they were.
#[test]
fn round_capped_lines_are_the_picture_they_were() {
    for segments in [40, 160] {
        let commands = average(
            segments,
            gpui_kit::rgba(0xe0a000b0),
            2.,
            wire::CanvasLineCap::Round,
        );
        let before = picture(&commands, painted_one_by_one);
        let after = picture(&commands, crate::render::canvas::paint_canvas_commands);
        let ink = before.pixels().filter(|pixel| pixel.0 != WHITE).count();
        assert!(ink > 500, "the line is drawn: {ink} pixels");
        assert_eq!(
            difference(&before, &after),
            Vec::new(),
            "{segments} segments: the pixels that differ"
        );
    }
}
