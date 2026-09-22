use super::*;

/// Development-only host renderer: never creates a node, shell model, or network runtime.
#[cfg(debug_assertions)]
pub(crate) fn render_tree_fixture() {
    use gpui_kit::component::{Root, Theme, ThemeMode};
    use gpui_kit::*;
    let args: Vec<_> = std::env::args().collect();
    let root: view_wire::Node = serde_json::from_slice(
        &std::fs::read(
            args.get(2)
                .expect("--render-tree <json> --size WxH --theme light|dark"),
        )
        .expect("read fixture"),
    )
    .expect("decode wire tree");
    let option = |name: &str| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1].clone())
    };
    let dimensions = option("--size").unwrap_or_else(|| "1100x760".into());
    let (width, height) = dimensions.split_once('x').expect("size WxH");
    let dimensions = size(
        px(width.parse().expect("width")),
        px(height.parse().expect("height")),
    );
    let dark = option("--theme").as_deref() == Some("dark");
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(
                if dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                },
                None,
                cx,
            );
            super::launch::initialize_rendering(cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        dimensions,
                    ))),
                    titlebar: None,
                    app_id: Some("dev.ducktape.tree-fixture".into()),
                    ..Default::default()
                },
                |window, cx| {
                    let tree = cx.new(|_| crate::view_tree::ViewTree::new(root));
                    let frame = cx.new(|_| TreeFixtureFrame(tree));
                    cx.new(|cx| Root::new(frame, window, cx))
                },
            )
            .expect("open fixture window");
            cx.activate(true);
        });
}

#[cfg(debug_assertions)]
struct TreeFixtureFrame(Entity<crate::view_tree::ViewTree>);

#[cfg(debug_assertions)]
impl Render for TreeFixtureFrame {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::ActiveTheme as _;
        let mut frame = gpui_kit::div();
        frame.text_style().font_fallbacks = Some(fallback_chain());
        frame
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.0.clone())
    }
}
