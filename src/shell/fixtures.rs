use super::launch::initialize_rendering;
use super::theme::fallback_chain;
use gpui_kit::{Context, Entity, IntoElement, Render, Window};

/// Development-only host renderer: never creates a node, shell model, or network runtime.
pub(crate) fn render_tree_fixture() {
    use gpui_kit::component::{Root, Theme, ThemeMode};
    use gpui_kit::*;
    let args: Vec<_> = std::env::args().collect();
    let tree: view_wire::Node = serde_json::from_slice(
        &std::fs::read(
            args.get(2)
                .expect("--render-tree <json> --size WxH --theme light|dark"),
        )
        .expect("read fixture"),
    )
    .expect("decode wire tree");
    let path = std::path::PathBuf::from(&args[2]);
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
            initialize_rendering(cx);
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
                    let store = fixture_editor_store(&tree, &path);
                    let tree = cx.new(|cx| {
                        let mut view = crate::render::ViewTree::new(tree);
                        view.set_editor_store(store, cx);
                        view
                    });
                    let frame = cx.new(|_| TreeFixtureFrame { tree });
                    cx.new(|cx| Root::new(frame, window, cx))
                },
            )
            .expect("open fixture window");
            cx.activate(true);
        });
}

struct TreeFixtureFrame {
    tree: Entity<crate::render::ViewTree>,
}

impl Render for TreeFixtureFrame {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::ActiveTheme as _;
        use gpui_kit::*;
        let mut frame = div();
        frame.text_style().font_fallbacks = Some(fallback_chain());
        frame.text_style().font_family = Some(super::theme::FAMILY_UI.into());
        frame
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.tree.clone())
    }
}

fn fixture_editor_store(
    root: &view_wire::Node,
    fixture: &std::path::Path,
) -> crate::editor::wire::EditorStore {
    use view_wire::editor_document::{
        EditorDocumentMessage as Message, EditorTransfer, MAX_EDITOR_CHUNK_BYTES,
    };
    #[derive(serde::Deserialize)]
    struct Document {
        document: String,
        text: String,
    }
    let path = fixture.with_extension("editors.json");
    let documents: Vec<Document> = if path.exists() {
        serde_json::from_slice(&std::fs::read(path).expect("read editor fixture"))
            .expect("decode editor fixture")
    } else {
        Vec::new()
    };
    let store = crate::editor::wire::EditorStore::new(1);
    store.replace(root).expect("mount editor references");
    while !store.ready().expect("editor store valid") {
        let requests = store.drain();
        assert!(!requests.is_empty(), "editor projection stalled");
        for event in requests {
            if let view_wire::Event::EditorDocument {
                message: Message::Request { id, target },
                ..
            } = event
            {
                let text = documents
                    .iter()
                    .find(|d| d.document == target.document)
                    .map(|d| d.text.as_str())
                    .unwrap_or("");
                assert_eq!(
                    text.len(),
                    target.byte_len as usize,
                    "missing or stale text for {}",
                    target.document
                );
                let mut messages = vec![Message::Transfer(EditorTransfer::Begin {
                    id: id.clone(),
                    target,
                })];
                for (index, bytes) in text.as_bytes().chunks(MAX_EDITOR_CHUNK_BYTES).enumerate() {
                    messages.push(Message::Transfer(EditorTransfer::Chunk {
                        id: id.clone(),
                        index: index.try_into().expect("chunk index"),
                        bytes: bytes.to_vec(),
                    }));
                }
                messages.push(Message::Transfer(EditorTransfer::Complete { id }));
                store
                    .frame(&view_wire::Frame {
                        editor_documents: messages,
                        ..Default::default()
                    })
                    .expect("seed real editor projection");
            }
        }
    }
    store
}
