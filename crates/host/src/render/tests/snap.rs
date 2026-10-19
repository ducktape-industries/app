//! `dev/screens/png.sh` runs `snap` `--ignored` with `SNAP_FRAME`, `SNAP_OUT`, `SNAP_SIZE` (WxH)
//! and `SNAP_THEME` to draw one whole view_wire frame to a PNG through the headless wgpu renderer,
//! as `ducktape-app --render-tree` draws it, the bundled assets in, with no display.
use super::*;
use gpui_kit::component::{ActiveTheme as _, Root, Theme, ThemeMode};
use gpui_kit::{HeadlessAppContext, PlatformHeadlessRenderer};

struct Framed(Entity<ViewTree>);

impl Render for Framed {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut frame = div();
        frame.text_style().font_fallbacks = Some(crate::fonts::fallback_chain());
        frame.text_style().font_family = Some(crate::fonts::FAMILY_UI.into());
        frame
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.0.clone())
    }
}

fn drawn(tree: Tree, width: f32, height: f32, dark: bool) -> image::RgbaImage {
    let text = Arc::new(crate::fonts::font_fallback::text_system());
    let assets = Arc::new(gpui_kit::assets::AllAssets);
    let mut cx = HeadlessAppContext::with_platform(text, assets, || {
        let renderer: Box<dyn PlatformHeadlessRenderer> =
            Box::new(gpui_wgpu::WgpuHeadlessRenderer::new()?);
        Ok(Some(renderer))
    });
    cx.update(|cx| {
        gpui_kit::init(cx);
        let mode = if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        Theme::change(mode, None, cx);
    });
    let window = cx
        .open_window(size(px(width), px(height)), |window, cx| {
            let tree = cx.new(|_| ViewTree::new(tree));
            let framed = cx.new(|_| Framed(tree));
            cx.new(|cx| Root::new(framed, window, cx))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    cx.capture_screenshot(window.into()).unwrap()
}

#[test]
#[ignore]
fn snap() {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let bytes = std::fs::read(var("SNAP_FRAME")).expect("SNAP_FRAME names a frame");
    let mut frame: view_wire::Frame = serde_json::from_slice(&bytes).expect("a whole frame");
    let mut styles = view_wire::Styles::default();
    view_wire::sanitize(&mut frame, &mut styles).expect("a frame the host takes");
    let tree = Tree {
        root: frame.root.expect("a root"),
        styles,
    };
    let sized = var("SNAP_SIZE");
    let (width, height) = sized.split_once('x').expect("SNAP_SIZE is WxH");
    let (width, height): (f32, f32) = (width.parse().unwrap(), height.parse().unwrap());
    let image = drawn(tree, width, height, var("SNAP_THEME") == "dark");
    image.save(var("SNAP_OUT")).expect("SNAP_OUT names a png");
}

#[test]
fn a_bundled_icon_draws() {
    let icon = wire::Node::Svg {
        id: Some(named_id("icon")),
        source: wire::SvgSource::Asset("icons/plus.svg".into()),
        transformation: wire::SvgTransformation {
            scale: [1.0, 1.0],
            translate: [0.0, 0.0],
            rotate: 0.0,
        },
        label: None,
        style: crate::render::test_style(
            div()
                .size(px(30.))
                .text_color(rgb(0xff0000))
                .style()
                .clone(),
        ),
        interactivity: Default::default(),
    };
    let page = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("page")),
        style: crate::render::test_style(div().size_full().bg(rgb(0xffffff)).style().clone()),
        interactivity: Default::default(),
        children: vec![icon],
    });
    let tree = Tree {
        root: page,
        styles: crate::render::test_styles(),
    };
    let image = drawn(tree, 60., 60., false);
    let scale = image.width() / 60;
    let inked = (0..30 * scale)
        .flat_map(|x| (0..30 * scale).map(move |y| (x, y)))
        .filter(|&(x, y)| {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            r > 200 && g < 80 && b < 80
        })
        .count();
    assert!(inked > 0, "the plus draws in red inside its box");
}
