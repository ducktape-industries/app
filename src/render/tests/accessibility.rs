use super::*;

#[test]
fn text_is_a_label_its_content_names() {
    let text = |content: &str| text("t", content);
    assert_eq!(
        accessible(&text("Members")),
        Accessible {
            role: Some(gpui_kit::Role::Label),
            name: Some("Members".into()),
            ..Default::default()
        }
    );
    assert_eq!(accessible(&text("")).name, None);
}

#[test]
fn an_input_is_named_by_its_label_and_a_secure_one_never_reports_its_value() {
    assert_eq!(
        accessible(&input("Room name", false, false)),
        Accessible {
            role: Some(gpui_kit::Role::TextInput),
            name: Some("Room name".into()),
            description: Some("Shown to members".into()),
            value: Some("hunter2".into()),
            ..Default::default()
        }
    );
    let secure = accessible(&input("Password", true, true));
    assert_eq!(secure.role, Some(gpui_kit::Role::PasswordInput));
    assert_eq!(secure.value, None);
    assert!(secure.disabled);
    // the placeholder is not a name
    assert_eq!(accessible(&input("", false, false)).name, None);
}

#[test]
fn an_editor_is_named_by_its_label_and_described_by_its_placeholder_without_one() {
    let mut editor = wire::Node::Editor {
        options: Box::default(),
        id: named_id("e"),
        style: gpui_kit::StyleRefinement::default(),
        label: None,
        placeholder: "Write something".into(),
        document: wire::editor_document::EditorDocumentRef {
            document: "e".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: Default::default(),
            byte_len: 0,
        },
        on_document: 0,
        editable: true,
    };
    assert_eq!(
        accessible(&editor),
        Accessible {
            role: Some(gpui_kit::Role::MultilineTextInput),
            description: Some("Write something".into()),
            ..Default::default()
        }
    );
    let wire::Node::Editor { label, .. } = &mut editor else {
        unreachable!()
    };
    *label = Some("Message".into());
    let named = accessible(&editor);
    assert_eq!(named.name.as_deref(), Some("Message"));
    // the placeholder is not a name, nor a second one
    assert_eq!(named.description, None);
}

/// #139: the field reads the text it holds, and one that takes no typing
/// says so and is offered none — the door reads the tree a screen reader
/// does.
#[gpui_kit::test]
fn a_read_only_editor_reads_its_text_and_is_offered_no_typing(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let words = "no channel is open";
    for editable in [true, false] {
        let root = wire::Node::Editor {
            id: named_id("composer"),
            style: gpui_kit::StyleRefinement::default(),
            label: Some("Message".into()),
            options: Box::default(),
            placeholder: String::new(),
            document: wire::editor_document::EditorDocumentRef {
                document: "composer".into(),
                reset: 1,
                text_revision: 0,
                revision: 0,
                cursor: Default::default(),
                byte_len: words.len() as u32,
            },
            on_document: 0,
            editable,
        };
        let store = crate::editor::wire::EditorStore::new(91);
        store.replace(&root).unwrap();
        seed_editor_text(&store, words);
        let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
            let mut tree = ViewTree::new(root);
            tree.set_editor_store(store, cx);
            tree
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let nodes = native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            window.render_frame(cx);
            crate::ax::snapshot("t", window, false)
        });
        let field = nodes
            .iter()
            .find(|node| node.role == "MultilineTextInput")
            .expect("the editor's field is in the tree");
        assert_eq!(field.value.as_deref(), Some(words), "editable: {editable}");
        match editable {
            true => {
                assert!(!field.state.contains(&"disabled"));
                assert!(field.actions.contains(&"type"));
                assert!(field.actions.contains(&"set_value"));
            }
            false => {
                assert!(field.state.contains(&"disabled"));
                assert!(field.actions.is_empty(), "{:?}", field.actions);
            }
        }
    }
}

#[test]
fn a_labelled_picture_is_an_image_and_an_unlabelled_one_is_decoration() {
    for node in picture(Some("Ada's avatar")) {
        assert_eq!(
            accessible(&node),
            Accessible {
                role: Some(gpui_kit::Role::Image),
                name: Some("Ada's avatar".into()),
                ..Default::default()
            }
        );
    }
    for node in picture(None).into_iter().chain(picture(Some(""))) {
        assert_eq!(accessible(&node), Accessible::default());
    }
}

#[test]
fn layout_is_not_in_the_accessibility_tree() {
    let space = wire::Node::Space {
        style: gpui_kit::StyleRefinement::default(),
    };
    assert_eq!(accessible(&space), Accessible::default());
}

#[test]
fn a_text_heading_has_its_level_and_a_live_text_its_politeness() {
    let text = |heading, live| {
        wire::Node::Text(view_wire::TextNode {
            id: Some(named_id("t")),
            style: gpui_kit::StyleRefinement::default(),
            content: "Members".into(),
            heading,
            live,
        })
    };
    for level in 1..=6u8 {
        assert_eq!(
            accessible(&text(Some(level), None)),
            Accessible {
                role: Some(gpui_kit::Role::Heading),
                name: Some("Members".into()),
                level: Some(level.into()),
                ..Default::default()
            }
        );
    }
    for live in [wire::Live::Polite, wire::Live::Assertive] {
        assert_eq!(
            accessible(&text(None, Some(live))),
            Accessible {
                role: Some(gpui_kit::Role::Label),
                name: Some("Members".into()),
                live: Some(live),
                ..Default::default()
            }
        );
    }
}

#[test]
fn a_named_overlay_is_a_dialog_and_an_unnamed_one_is_layout() {
    let overlay = |label: Option<&str>, open| wire::Node::Overlay {
        id: named_id("o"),
        label: label.map(str::to_owned),
        on_dismiss: None,
        children: if open {
            vec![wire::Node::empty(), wire::Node::empty()]
        } else {
            Vec::new()
        },
        style: gpui_kit::StyleRefinement::default(),
    };
    assert_eq!(
        accessible(&overlay(Some("Rename channel"), true)),
        Accessible {
            role: Some(gpui_kit::Role::Dialog),
            name: Some("Rename channel".into()),
            ..Default::default()
        }
    );
    assert_eq!(
        accessible(&overlay(Some("Rename channel"), false)),
        Accessible::default()
    );
    assert_eq!(accessible(&overlay(None, true)), Accessible::default());
}

/// What the door reads of `root`, drawn in a window with the tree on.
fn door(cx: &mut gpui_kit::TestAppContext, root: wire::Node) -> Vec<serde_json::Value> {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        crate::ax::snapshot("t", window, false)
            .iter()
            .map(|node| serde_json::to_value(node).unwrap())
            .collect()
    })
}

/// A view's words are what its text is called, through the door as through
/// the OS adapters, which name a Label by its value: a Text, and a RichText
/// (gpui's `InteractiveText`, which carries its words only as the value).
#[gpui_kit::test]
fn a_view_text_reads_its_words_as_its_name_through_the_door(cx: &mut gpui_kit::TestAppContext) {
    let rich = wire::Node::RichText {
        id: Some(named_id("rich")),
        style: Default::default(),
        text: "Three online".into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    };
    let root = axis_container("root", Axis::Column, [text("plain", "Members"), rich]);
    let labels: Vec<_> = door(cx, root)
        .into_iter()
        .filter(|node| node["role"] == "Label")
        .map(|node| node["name"].clone())
        .collect();
    assert_eq!(labels, ["Members", "Three online"]);
}
