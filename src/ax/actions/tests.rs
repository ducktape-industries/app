//! Regression tests for the AX door's editor path: `perform_by_id` on a
//! nested editor must reach the guest's own document; an action a view
//! advertises reaches the view; and a press on a view's control is that
//! control's click, wherever it is drawn.

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
        binding: Some(Box::new(wire::EditorBinding {
            claims: Vec::new(),
            on_request: 1,
            on_event: 2,
        })),
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

/// A view's stepper advertising increment, decrement and one custom
/// action: the door offers the two it has words for, `/act` performs them,
/// and each request, the custom one too, reaches the view on the route it
/// named, with its data (AX-116).
#[gpui_kit::test]
fn an_action_a_view_advertises_reaches_the_view(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::accesskit::Action as A;
    cx.update(gpui_kit::init);
    let mut stepper = container("stepper", Vec::new());
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut stepper {
        interactivity.role = Some(Role::SpinButton);
        interactivity.aria.label = Some("Count".into());
        interactivity.aria.numeric_value = Some(3.);
        interactivity.aria.actions =
            vec![(A::Increment, 7), (A::Decrement, 8), (A::CustomAction, 9)];
        interactivity.aria.custom_actions = vec![(4, "Reset".into())];
    }
    let root = container("chat-viewport", vec![stepper]);
    let window = cx.open_window(size(px(200.), px(120.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            if let wire::Event::A11yAction { handler, data } = event {
                seen.borrow_mut().push((*handler, data.clone()));
            }
        })
    });
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        let node = snapshot("t", window, false)
            .into_iter()
            .find(|node| node.role == "SpinButton")
            .expect("the stepper is in the tree");
        assert_eq!(node.actions, ["increment", "decrement"]);
        for word in ["increment", "decrement"] {
            assert!(perform_by_id("t", window, cx, &node.id, word, ""));
            window.render_frame(cx);
        }
        window.dispatch_a11y_action(
            ActionRequest {
                action: A::CustomAction,
                target_tree: TreeId::ROOT,
                target_node: node.node,
                data: Some(ActionData::CustomAction(4)),
            },
            cx,
        );
    });
    assert_eq!(
        *events.borrow(),
        [(7, None), (8, None), (9, Some(ActionData::CustomAction(4)))]
    );
}

/// A control a view draws out of a box: `role`, named `name`, sized by
/// `style`, its click on route `click`.
fn pressable(
    key: &str,
    role: Role,
    name: String,
    style: gpui_kit::StyleRefinement,
    click: u32,
) -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id(key)),
        style,
        interactivity: wire::Interactivity {
            role: Some(role),
            aria: wire::Aria {
                label: Some(name.into()),
                ..Default::default()
            },
            on_click: Some(click),
            ..Default::default()
        },
        children: Vec::new(),
    })
}

/// What the census does to a view: `/act press` on a row scrolled out of
/// its list is that row's click. Ten 40 px rows in a 100 px scroller put
/// row 5 at 200..240, outside the list and inside the window, so the door
/// shows it; a button of the view's lies right there. gpui's own Click was
/// a pointer press at the row's middle with no hit test: it clicked the
/// button. The click is the row's own, a keyboard click on its bounds as
/// Enter's is, and grants user activation, as a pointer's press does.
#[gpui_kit::test]
fn a_press_on_a_view_row_scrolled_out_of_its_list_is_that_rows_click(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::Styled as _;
    cx.update(gpui_kit::init);
    let rows = (0..10).map(|n| {
        pressable(
            &format!("row-{n}"),
            Role::ListBoxOption,
            format!("Row {n}"),
            gpui_kit::div().h(px(40.)).flex_shrink_0().style().clone(),
            10 + n,
        )
    });
    let mut scrolls = gpui_kit::div()
        .flex()
        .flex_col()
        .w(px(200.))
        .h(px(100.))
        .style()
        .clone();
    scrolls.overflow.y = Some(gpui_kit::Overflow::Scroll);
    let list = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("list")),
        style: scrolls,
        interactivity: Default::default(),
        children: rows.collect(),
    });
    let over = pressable(
        "over",
        Role::Button,
        "Over".into(),
        gpui_kit::div()
            .absolute()
            .top(px(200.))
            .left(px(0.))
            .w(px(200.))
            .h(px(40.))
            .style()
            .clone(),
        99,
    );
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("root")),
        style: gpui_kit::div().relative().size(px(300.)).style().clone(),
        interactivity: Default::default(),
        children: vec![list, over],
    });
    let window = cx.open_window(size(px(300.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            seen.borrow_mut().push(event.clone());
        })
    });
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        let nodes = snapshot("t", window, true);
        let row = nodes
            .iter()
            .find(|node| node.name == "Row 5")
            .expect("the door shows the row scrolled out of its list");
        let over = nodes
            .iter()
            .find(|node| node.name == "Over")
            .expect("the button is in the tree");
        assert!(row.actions.contains(&"press"));
        assert_eq!(row.bounds, Some([0, 200, 200, 240]));
        assert_eq!(over.bounds, row.bounds);
        assert!(perform_by_id("t", window, cx, &row.id, "press", ""));
    });
    let events = events.borrow();
    let [
        wire::Event::Click {
            handler: 15,
            event: wire::click::Click::Keyboard { button, bounds },
        },
    ] = &events[..]
    else {
        panic!("row 5's click alone, not {events:?}");
    };
    assert_eq!(*button, wire::click::KeyboardButton::Enter);
    assert_eq!(
        *bounds,
        gpui_kit::Bounds::new(gpui_kit::point(px(0.), px(200.)), size(px(200.), px(40.)))
    );
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_some(),
            "the reader's press is a gesture, as a pointer's is"
        );
    });
}
