//! The inspector's half in the renderer (J-094): the frame a view's tree is
//! drawing, walked by view-wire's identity rule, and a press or typing that
//! goes in on the channel a real one takes.

use super::*;
use gpui_kit::EntityInputHandler as _;
use serde::Serialize;

/// One node of a view's frame, as the inspector shows it: what it is,
/// what assistive technology would call it, the words it shows and what
/// pressing it can do.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Inspected {
    /// Its place in the view: the authored path the host keys its native
    /// state by, ids joined by `/`. A node with no id of its own is
    /// `@<n>` after its nearest identified ancestor's path: its place
    /// among that scope's nodes, in document order.
    pub path: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// What `press` can send it (`click`, `aux_click`, `submit`,
    /// `dismiss`), and `type` for a field.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub presses: Vec<&'static str>,
}

/// One id as the inspector writes it in a path.
fn segment(id: &wire::ElementIdWire) -> String {
    match id {
        wire::ElementIdWire::Name(name) => name.to_string(),
        wire::ElementIdWire::Integer(index) => index.to_string(),
        id => format!("{id:?}"),
    }
}

fn kind(node: &wire::Node) -> &'static str {
    use wire::Node;
    match node {
        Node::Container(_) => "container",
        Node::Text(_) => "text",
        Node::RichText { .. } => "rich_text",
        Node::Image { .. } => "image",
        Node::Svg { .. } => "svg",
        Node::Field { .. } => "field",
        Node::UniformList { .. } => "uniform_list",
        Node::List { .. } => "list",
        Node::ResizeHandle { .. } => "resize_handle",
        Node::Sensor { .. } => "sensor",
        Node::Overlay { .. } => "overlay",
        Node::Sheet { .. } => "sheet",
        Node::Web { .. } => "web",
        Node::Camera { .. } => "camera",
        Node::Microphone { .. } => "microphone",
        Node::Remote { .. } => "remote",
        Node::Anchored { .. } => "anchored",
        Node::Deferred { .. } => "deferred",
        Node::Canvas { .. } => "canvas",
        Node::Space => "space",
    }
}

/// Every node of `root` in document order with its address and authored
/// path, handed to `visit`; the walk stops at the first `Some` it answers.
fn walk<T>(
    root: &wire::Node,
    visit: &mut dyn FnMut(&str, &AuthoredPath, &wire::Node) -> Option<T>,
) -> Option<T> {
    fn go<T>(
        node: &wire::Node,
        row: Option<usize>,
        path: &mut AuthoredPath,
        unnamed: &mut Vec<usize>,
        visit: &mut dyn FnMut(&str, &AuthoredPath, &wire::Node) -> Option<T>,
    ) -> Option<T> {
        let entered = enter_scope(node, row, path);
        let mut address = path.iter().map(segment).collect::<Vec<_>>().join("/");
        if entered {
            unnamed.push(0);
        } else if let Some(count) = unnamed.last_mut() {
            *count += 1;
            address = match address.is_empty() {
                true => format!("@{count}"),
                false => format!("{address}/@{count}"),
            };
        }
        let mut found = visit(&address, path, node);
        for (at, child) in node.children().iter().enumerate() {
            if found.is_some() {
                break;
            }
            found = go(child, wire::identity::row(node, at), path, unnamed, visit);
        }
        if entered {
            unnamed.pop();
            path.pop();
        }
        found
    }
    go(root, None, &mut Vec::new(), &mut vec![0], visit)
}

/// What the inspector says of `node`, filed at `address`.
fn inspected(node: &wire::Node, address: &str) -> Inspected {
    let heard = accessibility::accessible(node);
    let mut seen = Inspected {
        path: address.to_owned(),
        kind: kind(node),
        role: heard.role.map(|role| format!("{role:?}")),
        label: heard.name,
        description: heard.description,
        value: heard.value,
        text: node.text().map(str::to_owned),
        disabled: heard.disabled,
        presses: Vec::new(),
    };
    if let Some(interactivity) = node.interactivity() {
        let aria = &interactivity.aria;
        if let Some(role) = interactivity.role {
            seen.role = Some(format!("{role:?}"));
        }
        if let Some(label) = &aria.label {
            seen.label = Some(label.to_string());
        } else if interactivity.on_click.is_some() && seen.label.is_none() {
            seen.label = accessibility::descendant_text(node);
        }
        if let Some(description) = &aria.description {
            seen.description = Some(description.to_string());
        }
        if let Some(value) = &aria.value {
            seen.value = Some(value.to_string());
        }
        if let Some(disabled) = aria.disabled {
            seen.disabled = disabled;
        }
        if interactivity.on_click.is_some() {
            seen.presses.push("click");
        }
        if interactivity.on_aux_click.is_some() {
            seen.presses.push("aux_click");
        }
    }
    match node {
        wire::Node::Field { on_submit, .. } => {
            seen.presses.push("type");
            if on_submit.is_some() {
                seen.presses.push("submit");
            }
        }
        wire::Node::Overlay {
            on_dismiss: Some(_),
            ..
        }
        | wire::Node::Sheet {
            on_dismiss: Some(_),
            ..
        } => seen.presses.push("dismiss"),
        _ => {}
    }
    seen
}

impl ViewTree {
    /// Every node of the frame being drawn, in document order. A field
    /// reads as its engine holds it, a secure one never.
    pub fn inspect(&self, window: &Window, cx: &App) -> Vec<Inspected> {
        let mut nodes = Vec::new();
        walk::<()>(&self.root, &mut |address, path, node| {
            let mut seen = inspected(node, address);
            if let wire::Node::Field { secure, .. } = node {
                seen.value = match secure {
                    true => None,
                    false => self
                        .fields
                        .get(path)
                        .map(|field| field.presentation(window, cx).value)
                        .or(seen.value),
                };
            }
            nodes.push(seen);
            None
        });
        nodes
    }

    /// Sends what `press` on the node at `address` sends: its own handler's
    /// event, on the channel the tree's listeners emit on, after the
    /// activation a real press records. No hit test and no position, so
    /// what covers the node or scrolls it away does not change it.
    pub fn inspect_press(
        &mut self,
        address: &str,
        press: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let found = walk(&self.root, &mut |at, _, node| {
            (at == address).then(|| pressed(node, press))
        });
        let event = found.ok_or_else(|| format!("no node at {address}"))??;
        self.activate();
        cx.emit(event);
        Ok(())
    }

    /// Types `text` into the field at `address` as a keyboard would: into
    /// the engine at its selection, which tells the view as `Event::Text`.
    pub fn inspect_type(
        &mut self,
        address: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let path = walk(&self.root, &mut |at, path, node| {
            (at == address).then(|| match node {
                wire::Node::Field { .. } => Ok(path.clone()),
                _ => Err(format!("{address} is a {}, not a field", kind(node))),
            })
        });
        let path = path.ok_or_else(|| format!("no node at {address}"))??;
        let field = self
            .fields
            .get(&path)
            .ok_or_else(|| format!("{address} is not drawn yet"))?;
        self.activate();
        match &field.engine {
            inputs::Engine::Line(state) => state.update(cx, |state, cx| {
                state.replace_text_in_range(None, text, window, cx)
            }),
            inputs::Engine::Area(state) => state.update(cx, |state, cx| {
                state.replace_text_in_range(None, text, window, cx)
            }),
        }
        Ok(())
    }
}

/// The event `press` on `node` sends, or why it has none.
fn pressed(node: &wire::Node, press: &str) -> Result<wire::Event, String> {
    let keyboard = || wire::click::Click::Keyboard {
        button: wire::click::KeyboardButton::Enter,
        bounds: Bounds::default(),
    };
    let interactivity = node.interactivity();
    let route = match (press, node) {
        ("click", _) => interactivity
            .and_then(|i| i.on_click)
            .map(|handler| wire::Event::Click {
                handler,
                event: keyboard(),
            }),
        ("aux_click", _) => {
            interactivity
                .and_then(|i| i.on_aux_click)
                .map(|handler| wire::Event::AuxClick {
                    handler,
                    event: keyboard(),
                })
        }
        (
            "submit",
            wire::Node::Field {
                on_submit: Some(handler),
                ..
            },
        )
        | (
            "dismiss",
            wire::Node::Overlay {
                on_dismiss: Some(handler),
                ..
            }
            | wire::Node::Sheet {
                on_dismiss: Some(handler),
                ..
            },
        ) => Some(wire::Event::Message(*handler)),
        _ => None,
    };
    route.ok_or_else(|| {
        let can = inspected(node, "").presses;
        format!("a {} takes no {press}; it takes {can:?}", kind(node))
    })
}
