use super::*;

/// `POST /reveal`, served only with `DUCKTAPE_AX_DOOR_PRIVATE=1`: the text a
/// person reads on the showing node `id` of window `name`, which must be
/// marked [`crate::a11y::AX_PRIVATE`]. A secure input is refused:
/// dots are all a person ever sees of it.
pub(super) fn reveal(name: &str, window: &Window, id: &str) -> Reply {
    let shown = snapshot(name, window, false);
    let Some(found) = shown.iter().find(|node| node.id == id) else {
        return Reply::new(
            404,
            json!({ "error": "no such node", "nearest": nearest(id, &shown) }),
        );
    };
    let node = window
        .a11y_tree()
        .and_then(|update| update.nodes.iter().find(|(node, _)| *node == found.node))
        .map(|(_, node)| node);
    match node {
        Some(node) if node.role() == Role::PasswordInput => Reply::new(
            403,
            json!({ "error": "a secure input is never shown on screen" }),
        ),
        Some(node) if node.class_name() == Some(crate::a11y::AX_PRIVATE) => Reply::ok(json!({
            "id": id,
            "role": found.role,
            "name": node.label().unwrap_or_default(),
            "value": node.value(),
        })),
        _ => Reply::new(400, json!({ "error": "not private: the tree shows it" })),
    }
}

/// Sends `keys` (space-separated keystrokes as GPUI parses them) and then
/// `text`, one key per character, through the window's own key dispatch:
/// its key bindings and the focused element's handlers, as a keyboard's
/// keys arrive — never an OS event. An unparsable keystroke sends nothing.
pub(super) fn press_keys(
    window: &mut Window,
    cx: &mut App,
    keys: &str,
    text: &str,
) -> Result<(), String> {
    let strokes = keys
        .split_whitespace()
        .map(|key| gpui_kit::Keystroke::parse(key).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    for stroke in strokes {
        // down, then up: a focused element takes Enter/Space as a click on
        // the release, as it does from a keyboard
        window.dispatch_keystroke(stroke.clone(), cx);
        window.dispatch_event(
            gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke: stroke }),
            cx,
        );
    }
    type_text(window, cx, text);
    Ok(())
}

/// Sends `drag` to `window` (name `name`): with an id, `from` and `to` are
/// offset by that node's painted origin, as its tree reports it. Answers
/// the window coordinates actually sent; 404 when the node is not showing.
pub(super) fn drag_by_id(name: &str, window: &mut Window, cx: &mut App, drag: &Drag) -> Reply {
    let steps = match drag.checked() {
        Ok(steps) => steps,
        Err(reply) => return reply,
    };
    let origin = match &drag.id {
        Some(id) => {
            let bounds = snapshot(name, window, true)
                .into_iter()
                .find(|node| node.id == *id)
                .and_then(|node| node.bounds);
            let Some([x, y, ..]) = bounds else {
                return Reply::new(404, json!({ "error": "no such node" }));
            };
            [x as f32, y as f32]
        }
        None => [0., 0.],
    };
    let from = [drag.from[0] + origin[0], drag.from[1] + origin[1]];
    let to = [drag.to[0] + origin[0], drag.to[1] + origin[1]];
    drag_pointer(window, cx, from, to, steps);
    Reply::ok(json!({ "from": from, "to": to, "steps": steps }))
}

/// A left-button drag through the window's own event dispatch, as a mouse's
/// arrives: press at `from`, `steps` held moves along the line, the last
/// exactly at `to`, release at `to`. Logical window px.
fn drag_pointer(window: &mut Window, cx: &mut App, from: [f32; 2], to: [f32; 2], steps: u32) {
    use gpui_kit::{MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput};
    let at = |[x, y]: [f32; 2]| gpui_kit::point(gpui_kit::px(x), gpui_kit::px(y));
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            position: at(from),
            button: MouseButton::Left,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    for step in 1..=steps {
        let position = if step == steps {
            to
        } else {
            let t = step as f32 / steps as f32;
            [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            ]
        };
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: at(position),
                pressed_button: Some(MouseButton::Left),
                modifiers: Default::default(),
            }),
            cx,
        );
    }
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            position: at(to),
            button: MouseButton::Left,
            modifiers: Default::default(),
            click_count: 1,
        }),
        cx,
    );
}

#[derive(Debug, Serialize, PartialEq)]
pub(super) struct Shortcut {
    keys: String,
    action: String,
}

/// The named keys a view's chord may end in, besides a letter or a digit.
const CHORD_KEYS: [&str; 14] = [
    "enter",
    "escape",
    "space",
    "tab",
    "backspace",
    "delete",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
];

/// The key bindings a keyboard can reach from where focus is now (the
/// focused element's context, or the window's root when nothing is), and
/// the chords the seated views hold (`runtime::claim_chord`): a view's
/// chord is claimed, not bound, so no binding names it.
pub(super) fn shortcuts(window: &Window, cx: &App) -> Vec<Shortcut> {
    let focus = window.focused(cx);
    let mut out: Vec<Shortcut> = Vec::new();
    for action in window.available_actions(cx) {
        let bindings = match &focus {
            Some(focus) => window.bindings_for_action_in(&*action, focus),
            None => window.bindings_for_action(&*action),
        };
        for binding in bindings {
            let keys = binding
                .keystrokes()
                .iter()
                .map(|key| key.unparse())
                .collect::<Vec<_>>()
                .join(" ");
            let shortcut = Shortcut {
                keys,
                action: action.name().to_owned(),
            };
            if !out.contains(&shortcut) {
                out.push(shortcut);
            }
        }
    }
    let command = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    let keys = ('a'..='z')
        .chain('0'..='9')
        .map(String::from)
        .chain(CHORD_KEYS.map(String::from));
    for key in keys {
        for extra in ["", "-shift", "-alt", "-shift-alt"] {
            let chord = format!("cmd{extra}-{key}");
            if let Some(module) = crate::runtime::chord_holder(&chord) {
                out.push(Shortcut {
                    keys: format!("{command}{extra}-{key}"),
                    action: format!("the {module} view's {chord}"),
                });
            }
        }
    }
    out
}

/// Every served window's [`current`] nodes.
pub(super) fn read(
    windows: &impl Fn(&App) -> Vec<(String, AnyWindowHandle)>,
    filter: &Filter,
    bounds: bool,
    seen: &mut Seen,
    cx: &mut AsyncApp,
) -> Vec<AxNode> {
    let list = cx.update(|cx| windows(cx));
    let mut out = Vec::new();
    for (name, handle) in &list {
        if filter.window.as_deref().is_some_and(|want| want != name) {
            continue;
        }
        let nodes = handle.update(cx, |_, window, cx| current(name, window, cx, bounds, seen));
        out.extend(nodes.unwrap_or_default());
    }
    out.retain(|node| filter.keeps(node));
    out
}

/// `window`'s visible nodes off a frame drawn for this read: a window the OS
/// stops drawing (covered, asleep, locked; #147) would otherwise serve its
/// last tree for as long as it stays hidden. The OS presents the draw with
/// its next frame. The first read switches the window's tree on.
pub(super) fn current(
    name: &str,
    window: &mut Window,
    cx: &mut App,
    bounds: bool,
    seen: &mut Seen,
) -> Vec<AxNode> {
    if !window.is_a11y_active() {
        window.activate_a11y();
    }
    // ponytail: gpui keeps a window's dirty flag private, so every read
    // draws; a wait's polls draw 20 times a second, and only with the door
    window.draw(cx).clear(cx);
    let Some(tree) = window.a11y_tree() else {
        return Vec::new();
    };
    seen.saw(name, tree);
    snapshot(name, window, bounds)
}

/// Performs `action` on the node `id` names in window `name`, through the
/// path an assistive technology's request takes; `type` and `set_value` both
/// focus the node first, then act. False when no such node is showing.
pub(super) fn perform_by_id(
    name: &str,
    window: &mut Window,
    cx: &mut App,
    id: &str,
    action: &str,
    value: &str,
) -> bool {
    let node = snapshot(name, window, false)
        .into_iter()
        .find_map(|node| (node.id == id).then_some(node.node));
    if let Some(node) = node {
        perform(window, cx, node, action, value);
    }
    node.is_some()
}

fn perform(window: &mut Window, cx: &mut App, node: NodeId, action: &str, value: &str) {
    let request = |action, data| ActionRequest {
        action,
        target_tree: TreeId::ROOT,
        target_node: node,
        data,
    };
    match action {
        "press" => window.dispatch_a11y_action(request(Action::Click, None), cx),
        "focus" => window.dispatch_a11y_action(request(Action::Focus, None), cx),
        "scroll_into_view" => {
            window.dispatch_a11y_action(request(Action::ScrollIntoView, None), cx)
        }
        "set_value" => {
            // Focus first, exactly as `type` does below: a multi-line guest
            // editor (`TextEditor::observed`, editor/text.rs) only forwards
            // an edit to the guest's own document while its native field is
            // focused — a guard against replaying its OWN programmatic
            // `install()` syncs back at the guest as a fresh edit. A single-
            // line `Node::Input` field has no such guard, so `set_value`
            // reached its guest either way; a multi-line editor's Send (or
            // any other guest state gated on the document) silently never
            // saw the value SetValue just set, with no fault and no refusal.
            window.dispatch_a11y_action(request(Action::Focus, None), cx);
            window.dispatch_a11y_action(
                request(Action::SetValue, Some(ActionData::Value(value.into()))),
                cx,
            );
        }
        "type" => {
            window.dispatch_a11y_action(request(Action::Focus, None), cx);
            type_text(window, cx, value);
        }
        _ => {}
    }
}

/// `text` as keys, one per character, to whatever holds focus.
fn type_text(window: &mut Window, cx: &mut App, text: &str) {
    for ch in text.chars() {
        let (key, text) = match ch {
            '\n' => ("enter".to_owned(), None),
            '\t' => ("tab".to_owned(), None),
            ' ' => ("space".to_owned(), Some(" ".to_owned())),
            ch => (ch.to_string(), Some(ch.to_string())),
        };
        window.dispatch_keystroke(
            gpui_kit::Keystroke {
                modifiers: Default::default(),
                key,
                key_char: text,
            },
            cx,
        );
    }
}

#[cfg(test)]
mod ax_editor_tests {
    use super::*;
    use crate::editor::wire::EditorStore;
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
    /// `chat_shaped_tree` in `runtime::widget_tests`): `chat-viewport >
    /// chat-root > chat-panes > chat-room > draft-general >
    /// draft-general/editor`. Every one of those named ancestors is owned by
    /// a different file, none of which the editor's own key threads through.
    fn chat_shaped_editor(editable: bool) -> (wire::Node, crate::render::AuthoredPath) {
        let editor = wire::Node::Editor {
            options: Box::new(wire::EditorOptions {
                binding: Some(Box::new(wire::EditorBinding {
                    authored: false,
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
            editable,
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

    /// Put an empty document into the store the honest way: answer the
    /// request the store makes for the one document it has no text for.
    fn seed_empty_document(store: &EditorStore) {
        use wire::editor_document::{EditorDocumentMessage as Message, EditorTransfer};
        let asked = store.drain().into_iter().find_map(|event| match event {
            wire::Event::EditorDocument {
                message: Message::Request { id, target },
                ..
            } => Some((id, target)),
            _ => None,
        });
        let (id, target) = asked.expect("the store asks for the document it has no text for");
        // A zero-byte document's assembler starts with `bytes.len() ==
        // expected` (both 0), so a Chunk message — even an empty one — is
        // out of order; Begin then Complete is the whole transfer.
        store
            .frame(&wire::Frame {
                editor_documents: vec![
                    Message::Transfer(EditorTransfer::Begin {
                        id: id.clone(),
                        target,
                    }),
                    Message::Transfer(EditorTransfer::Complete { id }),
                ],
                ..Default::default()
            })
            .expect("the answer to the store's own request");
    }

    /// The QA runner's `type` AX action typed into the composer's
    /// AX-visible native field (focus and value both showed it) but the
    /// guest's own document never moved: Send stayed disabled forever.
    /// Drives the exact AX door path a QA runner uses (`perform_by_id`,
    /// `"type"`) against a tree shaped exactly like the real composer's
    /// nested ancestry, and asserts the store the guest reads ends up
    /// holding what was typed.
    #[gpui_kit::test]
    fn ax_type_on_a_nested_editor_reaches_the_guests_document(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (root, path) = chat_shaped_editor(true);
        let store = EditorStore::new(1);
        store.replace(&root).expect("mount editor references");
        seed_empty_document(&store);
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
            .update(|window, cx| perform_by_id("t", window, cx, "t:editor-field", "type", "hello"));
        assert!(performed, "the editor's AX field must be in the tree");
        native.update(|window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        });
        store.ready().expect("no fault after AX `type`");
        let text = store
            .projection(&path)
            .and_then(|projection| projection.text)
            .unwrap_or_default();
        assert_eq!(
            &*text, "hello",
            "AX `type` on the composer's editor must reach the guest's own document"
        );
    }

    /// The same drive, through AX `set_value` instead of `type`: the AX
    /// door's own `perform()` dispatches `set_value` WITHOUT an explicit
    /// Focus first, unlike `type`.
    #[gpui_kit::test]
    fn ax_set_value_on_a_nested_editor_reaches_the_guests_document(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (root, path) = chat_shaped_editor(true);
        let store = EditorStore::new(1);
        store.replace(&root).expect("mount editor references");
        seed_empty_document(&store);
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
        let performed = native.update(|window, cx| {
            perform_by_id("t", window, cx, "t:editor-field", "set_value", "hello")
        });
        assert!(performed, "the editor's AX field must be in the tree");
        native.update(|window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        });
        store.ready().expect("no fault after AX `set_value`");
        let text = store
            .projection(&path)
            .and_then(|projection| projection.text)
            .unwrap_or_default();
        assert_eq!(
            &*text, "hello",
            "AX `set_value` on the composer's editor must reach the guest's own document"
        );
    }
}
