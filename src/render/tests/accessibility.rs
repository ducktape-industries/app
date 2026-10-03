use super::*;

#[test]
fn text_is_a_label_its_content_names() {
    let text = |content: &str| text("t", content);
    assert_eq!(
        accessible(&text("Members")),
        Accessible {
            role: Some(gpui_kit::Role::Label),
            name: Some("Members".into()),
            value: Some("Members".into()),
            ..Default::default()
        }
    );
    for blank in ["", " ", "\n\t"] {
        assert_eq!(accessible(&text(blank)), Accessible::default(), "{blank:?}");
    }
}

#[test]
fn an_input_is_named_by_its_label_and_a_secure_one_never_reports_its_value() {
    assert_eq!(
        accessible(&input("Room name", false, false)),
        Accessible {
            role: Some(gpui_kit::Role::TextInput),
            name: Some("Room name".into()),
            description: Some("Shown to members".into()),
            placeholder: Some("Type here".into()),
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
        binding: None,
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
            placeholder: Some("Write something".into()),
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
            binding: None,
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
        let (_, mut native) = with_editors(root, Some(words), cx);
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
    draw(cx, root)
        .iter()
        .map(|node| serde_json::to_value(node).unwrap())
        .collect()
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

/// AX-014 through the door: a blank text is no Label (forge's file-row glyph,
/// explorer's empty stat note), and a Heading whose only child is a RichText
/// (forge's README heading) is named by its words.
#[gpui_kit::test]
fn a_blank_text_is_no_node_and_a_heading_is_named_by_its_rich_text(
    cx: &mut gpui_kit::TestAppContext,
) {
    let rich = wire::Node::RichText {
        id: Some(named_id("words")),
        style: Default::default(),
        text: "sandbox".into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    };
    let mut heading = axis_container("heading", Axis::Column, [rich]);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut heading {
        interactivity.role = Some(gpui_kit::Role::Heading);
        interactivity.aria.level = Some(1);
    }
    let root = axis_container(
        "root",
        Axis::Column,
        [
            heading,
            text("empty", ""),
            // as the SDK sends every text: no id, so the host numbers it
            wire::Node::Text(view_wire::TextNode {
                id: None,
                style: Default::default(),
                content: " ".into(),
            }),
            text("kept", "kept"),
        ],
    );
    let nodes = door(cx, root);
    // neither blank text is a node, whether the view named it or the host
    // numbered it
    let ids: Vec<_> = nodes.iter().map(|node| node["id"].clone()).collect();
    assert_eq!(ids, ["t:heading", "t:rich-text", "t:kept"]);
    let of = |role: &str| -> Vec<_> {
        nodes
            .iter()
            .filter(|node| node["role"] == role)
            .map(|node| node["name"].clone())
            .collect()
    };
    assert_eq!(of("Heading"), ["sandbox"]);
    assert_eq!(of("Label"), ["sandbox", "kept"]);
}

/// A view Text carries its words as its value, as gpui's own `Text` does:
/// what a live region announces and what the door reads back.
#[gpui_kit::test]
fn a_view_text_carries_its_words_as_its_value_through_the_door(cx: &mut gpui_kit::TestAppContext) {
    let nodes = door(cx, text("plain", "Members"));
    let label = nodes
        .iter()
        .find(|node| node["role"] == "Label")
        .expect("the text is in the tree");
    assert_eq!(label["value"], "Members");
}

/// A field's placeholder is its placeholder, not its name: the door
/// reads it back under its own key (AX-111).
#[gpui_kit::test]
fn a_fields_placeholder_reaches_the_door(cx: &mut gpui_kit::TestAppContext) {
    let root = axis_container("root", Axis::Column, [input("Room name", false, false)]);
    let nodes = door(cx, root);
    let field = nodes
        .iter()
        .find(|node| node["role"] == "TextInput")
        .expect("the field is in the tree");
    assert_eq!(field["name"], "Room name");
    assert_eq!(field["placeholder"], "Type here");
}

/// A field the view marks invalid, required and read-only says so on its
/// node, and the door reads each back (AX-108, AX-109). Read-only, it
/// offers no edit it would refuse; an editable one offers both.
#[gpui_kit::test]
fn a_field_says_it_is_invalid_required_and_read_only(cx: &mut gpui_kit::TestAppContext) {
    let mut field = input("Room name", false, false);
    let wire::Node::Input { options, .. } = &mut field else {
        unreachable!()
    };
    options.invalid = Some(gpui_kit::accesskit::Invalid::True);
    options.required = true;
    options.read_only = true;
    let mut editable = input("Topic", false, false);
    if let wire::Node::Input { id, .. } = &mut editable {
        *id = wire::ElementIdWire::Name("topic".into());
    }
    let nodes = door(cx, axis_container("root", Axis::Column, [field, editable]));
    let named = |name: &str| {
        nodes
            .iter()
            .find(|node| node["role"] == "TextInput" && node["name"] == name)
            .expect("the field is in the tree")
    };
    let field = named("Room name");
    assert_eq!(field["invalid"], "true");
    assert_eq!(field["required"], true);
    assert_eq!(field["read_only"], true);
    assert_eq!(field["description"], "Shown to members");
    assert_eq!(field["actions"], serde_json::json!(["focus"]));
    assert_eq!(
        named("Topic")["actions"],
        serde_json::json!(["focus", "set_value", "type"])
    );
}

/// A view whose rich text has clickable ranges is clean on the door, with
/// the keys elsewhere and with them on the text: the links are reached
/// from the box around the text, whose picked link the door reports
/// focused, a Link under the text's box.
#[gpui_kit::test]
fn a_rich_texts_links_are_clean_on_the_door(cx: &mut gpui_kit::TestAppContext) {
    use crate::ax::audit::{Reading, audit};
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(rich()));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let away = native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        crate::ax::snapshot("t", window, false)
    });
    let held = native.update(|window, cx| {
        window.focus_next(cx);
        window.render_frame(cx);
        crate::ax::snapshot("t", window, false)
    });
    let seen: Vec<serde_json::Value> = held
        .iter()
        .map(|node| serde_json::to_value(node).unwrap())
        .collect();
    let focused = seen
        .iter()
        .find(|node| node["state"] == serde_json::json!(["focused"]))
        .expect("the keys are on a node");
    assert_eq!(focused["role"], "Link");
    assert_eq!(focused["name"], "the docs");
    let text = seen
        .iter()
        .find(|node| node["id"] == focused["parent"])
        .expect("the picked link sits under the text");
    assert_eq!(text["role"], "Group");
    assert_eq!(text["actions"], serde_json::json!(["focus"]));
    let report = audit(
        &Reading {
            snapshots: vec![away, held],
            ..Default::default()
        },
        false,
    );
    assert_eq!(report.violations, []);
}

/// A rich text carries no aria of its own (the wire gives it no
/// interactivity), so its links and a view's phase-2 aria never share a
/// node: a roled box around it keeps both, its patch on its own node and
/// the links under the text's.
#[gpui_kit::test]
fn a_roled_box_keeps_its_aria_and_the_links_of_the_text_it_holds(
    cx: &mut gpui_kit::TestAppContext,
) {
    let note = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("note")),
        style: Default::default(),
        interactivity: Box::new(wire::Interactivity {
            role: Some(gpui_kit::Role::Status),
            aria: wire::Aria {
                label: Some("Note".into()),
                live: Some(gpui_kit::accesskit::Live::Polite),
                busy: true,
                ..Default::default()
            },
            ..Default::default()
        }),
        children: vec![rich()],
    });
    let nodes = door(cx, note);
    let status = nodes
        .iter()
        .find(|node| node["role"] == "Status")
        .expect("the box is in the tree");
    assert_eq!(status["live"], "polite");
    assert_eq!(status["state"], serde_json::json!(["busy"]));
    let links: Vec<_> = nodes
        .iter()
        .filter(|node| node["role"] == "Link")
        .map(|node| node["name"].clone())
        .collect();
    assert_eq!(links, ["the docs", "the code"]);
}

/// A RichText's clickable ranges are Links, each named by its words, and a
/// press on one from assistive technology is that range's click (AX-117).
#[gpui_kit::test]
fn a_rich_texts_ranges_are_links_a_press_reaches(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(rich()));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    let links: Vec<serde_json::Value> = native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        crate::ax::snapshot("t", window, false)
            .iter()
            .map(|node| serde_json::to_value(node).unwrap())
            .filter(|node| node["role"] == "Link")
            .collect()
    });
    let said: Vec<_> = links
        .iter()
        .map(|link| {
            (
                link["name"].clone(),
                link["actions"].clone(),
                link["id"].clone(),
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            (
                "the docs".into(),
                serde_json::json!(["press"]),
                "t:Link".into()
            ),
            (
                "the code".into(),
                serde_json::json!(["press"]),
                "t:Link2".into()
            ),
        ]
    );
    native.update(|window, cx| {
        let target = window
            .a11y_tree()
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("the code"))
            .map(|(id, _)| *id)
            .unwrap();
        window.dispatch_a11y_action(
            ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: target,
                data: None,
            },
            cx,
        );
    });
    let events = events.borrow();
    let _event = events
        .iter()
        .find(|event| {
            matches!(
                event,
                wire::Event::Select {
                    handler: 72,
                    index: 1
                }
            )
        })
        .expect("the link's press is its range's click");
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_some(),
            "the reader's press is a gesture, as a pointer's is"
        );
    });
}
