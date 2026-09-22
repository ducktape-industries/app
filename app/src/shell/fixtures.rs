use super::launch::initialize_rendering;
use super::theme::fallback_chain;
use gpui_kit::{Context, Entity, IntoElement, Render, Window};

/// Development-only host renderer: never creates a node, shell model, or network runtime.
pub(crate) fn render_tree_fixture() {
    use gpui_kit::component::{Root, Theme, ThemeMode};
    use gpui_kit::*;
    let args: Vec<_> = std::env::args().collect();
    let fixture: Fixture = serde_json::from_slice(
        &std::fs::read(
            args.get(2)
                .expect("--render-tree <json> --size WxH --theme light|dark"),
        )
        .expect("read fixture"),
    )
    .expect("decode wire tree or pane fixture");
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
                    let (panes, focused, popped_out, bare) = match fixture {
                        Fixture::Tree(tree) => (
                            vec![FixturePane {
                                label: String::new(),
                                context: String::new(),
                                tree: *tree,
                            }],
                            0,
                            false,
                            true,
                        ),
                        Fixture::Workspace {
                            panes,
                            focused,
                            popped_out,
                        } => {
                            assert!(
                                (1..=super::layout::MAX_PANES).contains(&panes.len()),
                                "one to three panes"
                            );
                            assert!(focused < panes.len(), "focused pane exists");
                            assert!(!popped_out || panes.len() == 1, "one popped view");
                            (panes, focused, popped_out, false)
                        }
                    };
                    let panes = panes
                        .into_iter()
                        .map(|pane| {
                            let store = fixture_editor_store(&pane.tree, &path);
                            let tree = cx.new(|cx| {
                                let mut tree = crate::view_tree::ViewTree::new(pane.tree);
                                tree.set_editor_store(store, cx);
                                tree
                            });
                            (pane.label, pane.context, tree)
                        })
                        .collect();
                    let frame = cx.new(|_| TreeFixtureFrame {
                        panes,
                        focused,
                        popped_out,
                        bare,
                        dark,
                    });
                    cx.new(|cx| Root::new(frame, window, cx))
                },
            )
            .expect("open fixture window");
            cx.activate(true);
        });
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Fixture {
    Workspace {
        panes: Vec<FixturePane>,
        #[serde(default)]
        focused: usize,
        #[serde(default)]
        popped_out: bool,
    },
    Tree(Box<view_wire::Node>),
}

#[derive(serde::Deserialize)]
struct FixturePane {
    label: String,
    #[serde(default)]
    context: String,
    tree: view_wire::Node,
}

struct TreeFixtureFrame {
    panes: Vec<(String, String, Entity<crate::view_tree::ViewTree>)>,
    focused: usize,
    popped_out: bool,
    bare: bool,
    dark: bool,
}

impl Render for TreeFixtureFrame {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::a11y::Control as _;
        use gpui_kit::component::ActiveTheme as _;
        use gpui_kit::prelude::FluentBuilder as _;
        use gpui_kit::*;
        let palette = design::palette(self.dark);
        let ink = super::hsla_of(palette.sidebar_foreground);
        let muted = super::hsla_of(palette.sidebar_muted);
        let border = super::hsla_of(palette.sidebar_border);
        let accent = super::hsla_of(palette.accent);
        let mut frame = div();
        frame.text_style().font_fallbacks = Some(fallback_chain());
        let frame = frame
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground);
        if self.bare {
            return frame.child(self.panes[0].2.clone());
        }
        let rail = div()
            .id("rail")
            .w(px(super::RAIL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(border)
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .text_size(px(13.))
                    .text_color(ink)
                    .child("Workspace")
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child("Fixture network"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .px_2()
                    .children(self.panes.iter().enumerate().map(|(n, (label, _, _))| {
                        div()
                            .h(px(28.))
                            .px_2()
                            .rounded(px(design::radius::CONTROL as f32))
                            .text_size(px(13.))
                            .text_color(ink)
                            .when(n == self.focused, |row| {
                                row.bg(super::hsla_of(palette.sidebar_raised))
                            })
                            .child(label.clone())
                    })),
            )
            .child(
                div()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .text_size(px(11.))
                    .text_color(muted)
                    .child("Reading without a key"),
            );
        let stage = div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap(px(6.))
            .py(px(8.))
            .pr(px(8.))
            .when(self.popped_out, |stage| stage.pl(px(8.)))
            .children(
                self.panes
                    .iter()
                    .enumerate()
                    .map(|(n, (label, context, tree))| {
                        let control = |action: &str, glyph: &str| {
                            div()
                                .id(SharedString::from(format!("pane/{n}/{action}")))
                                .control(Role::Button, SharedString::from(action.to_owned()))
                                .aria_disabled(
                                    action == "split"
                                        && self.panes.len() == super::layout::MAX_PANES,
                                )
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(22.))
                                .text_color(muted)
                                .child(glyph.to_owned())
                        };
                        div()
                            .id(SharedString::from(format!("pane/{n}")))
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .rounded(px(8.))
                            .bg(cx.theme().background)
                            .border(px(if n == self.focused { 1.5 } else { 1. }))
                            .border_color(if n == self.focused { accent } else { border })
                            .child(
                                div()
                                    .id(SharedString::from(format!("pane/{n}/strip")))
                                    .flex()
                                    .items_center()
                                    .h(px(30.))
                                    .flex_shrink_0()
                                    .px(px(10.))
                                    .gap(px(6.))
                                    .text_size(px(12.))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(ink)
                                            .child(label.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(muted)
                                            .child(context.clone()),
                                    )
                                    // Mirrors panes.rs: a console pane splits and pops
                                    // out; a popped-out window only pops back in.
                                    .when(!self.popped_out, |strip| {
                                        strip
                                            .child(control("split", "⊞"))
                                            .child(control("popout", "↗"))
                                    })
                                    .when(self.popped_out, |strip| {
                                        strip.child(control("popin", "↙"))
                                    })
                                    .child(control("close", "×")),
                            )
                            .child(div().flex_1().min_h_0().size_full().child(tree.clone()))
                    }),
            );
        frame
            .flex()
            .when(!self.popped_out, |frame| frame.child(rail))
            .child(stage)
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
