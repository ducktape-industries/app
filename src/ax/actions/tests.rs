//! Regression tests for the AX door's editor path: `perform_by_id` on a
//! nested editor must reach the guest's own document.

use super::*;
use crate::editor::wire::{EditorStore, seed_editor_text};
use crate::render::ViewTree;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{px, size};
use view_wire as wire;

fn named_id(key: &str) -> wire::ElementIdWire {
    wire::ElementIdWire::Name(key.into())
}

fn container(key: &str, children: Vec<wire::Node>) -> wire::Node {
    use gpui_kit::Styled as _;
    // Every ancestor fills its parent, same as the real chat pane's own
    // wrappers (`native_root`, the room, the composer's own div): a
    // percentage size against an unsized ancestor resolves to zero, and
    // an invisible (zero-bounds) node never reaches the AX door's tree.
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id(key)),
        style: gpui_kit::div().size_full().style().clone(),
        interactivity: Default::default(),
        children,
    })
}

/// The real shape a chat composer's editor mounts under (see PR #238's
/// `chat_shaped_tree` in `runtime/guest/requests.rs`): `chat-viewport >
/// chat-root > chat-panes > chat-room > draft-general >
/// draft-general/editor`. Every one of those named ancestors is owned by
/// a different file, none of which the editor's own key threads through.
fn chat_shaped_editor() -> (wire::Node, crate::render::AuthoredPath) {
    let editor = wire::Node::Editor {
        options: Box::new(wire::EditorOptions {
            binding: Some(Box::new(wire::EditorBinding {
                claims: Vec::new(),
                on_request: 1,
                on_event: 2,
            })),
            ..Default::default()
        }),
        id: named_id("draft-general/editor"),
        style: Default::default(),
        placeholder: String::new(),
        label: Some("Message #general".into()),
        document: wire::editor_document::EditorDocumentRef {
            document: "draft-general".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: Default::default(),
            byte_len: 0,
        },
        on_document: 0,
        editable: true,
    };
    let root = container(
        "chat-viewport",
        vec![container(
            "chat-root",
            vec![container(
                "chat-panes",
                vec![container(
                    "chat-room",
                    vec![container("draft-general", vec![editor])],
                )],
            )],
        )],
    );
    let path = [
        "chat-viewport",
        "chat-root",
        "chat-panes",
        "chat-room",
        "draft-general",
        "draft-general/editor",
    ]
    .into_iter()
    .map(named_id)
    .collect();
    (root, path)
}

/// Drives `action` with the value "hello" through the AX door path a QA
/// runner uses (`perform_by_id`) against a tree shaped like the real
/// composer's nested ancestry, and asserts the store the guest reads
/// ends up holding it.
fn ax_action_reaches_the_guests_document(cx: &mut gpui_kit::TestAppContext, action: &str) {
    cx.update(gpui_kit::init);
    let (root, path) = chat_shaped_editor();
    let store = EditorStore::new(1);
    store.replace(&root).expect("mount editor references");
    seed_editor_text(&store, "");
    let window_store = store.clone();
    let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
        let mut tree = ViewTree::new(root);
        tree.set_editor_store(window_store, cx);
        tree
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
    });
    let performed = native
        .update(|window, cx| perform_by_id("t", window, cx, "t:editor-field", action, "hello"));
    assert!(performed, "the editor's AX field must be in the tree");
    native.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    store
        .ready()
        .unwrap_or_else(|fault| panic!("no fault after AX `{action}`: {fault}"));
    let text = store
        .projection(&path)
        .and_then(|projection| projection.text)
        .unwrap_or_default();
    assert_eq!(
        &*text, "hello",
        "AX `{action}` on the composer's editor must reach the guest's own document"
    );
}

/// The QA runner's `type` AX action typed into the composer's
/// AX-visible native field (focus and value both showed it) but the
/// guest's own document never moved: Send stayed disabled forever.
#[gpui_kit::test]
fn ax_type_on_a_nested_editor_reaches_the_guests_document(cx: &mut gpui_kit::TestAppContext) {
    ax_action_reaches_the_guests_document(cx, "type");
}

/// The same drive, through AX `set_value`. `perform` focuses the node
/// before SetValue, as it does for `type` (#241); without that focus
/// `TextEditor::observed` drops the edit and the guest never sees it.
#[gpui_kit::test]
fn ax_set_value_on_a_nested_editor_reaches_the_guests_document(cx: &mut gpui_kit::TestAppContext) {
    ax_action_reaches_the_guests_document(cx, "set_value");
}
