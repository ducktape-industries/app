//! Not a check: `dev/screens/png.sh` runs it `--ignored` with `SNAP_FRAME`, `SNAP_OUT`, `SNAP_SIZE`
//! (WxH) and `SNAP_THEME` to draw one whole view_wire frame to a PNG through the headless wgpu
//! renderer, as `ducktape-app --render-tree` draws it in a window, with no display.
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
    let dark = var("SNAP_THEME") == "dark";
    let text = Arc::new(crate::fonts::font_fallback::text_system());
    let mut cx = HeadlessAppContext::with_platform(text, Arc::new(()), || {
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
    let image = cx.capture_screenshot(window.into()).unwrap();
    image.save(var("SNAP_OUT")).expect("SNAP_OUT names a png");
}
