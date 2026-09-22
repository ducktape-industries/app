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
/// path an assistive technology's request takes; `type` focuses it and sends
/// each character as a key. False when no such node is showing.
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
        "set_value" => window.dispatch_a11y_action(
            request(Action::SetValue, Some(ActionData::Value(value.into()))),
            cx,
        ),
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
