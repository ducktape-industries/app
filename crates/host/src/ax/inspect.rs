//! The inspector's door (J-094): every view's tree as the host holds it,
//! found, read and pressed by the path the host files a node under, and a
//! window drawn to a picture. It reads no accessibility tree.

use super::{Door, Reply, deadline, passed, settled};
use crate::render::{Inspected, ViewTree};
use gpui_kit::{AnyWindowHandle, App, AsyncApp, Entity};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::time::Instant;

/// One seated view: the window it is drawn in, its program and its tree.
pub type Seated = (AnyWindowHandle, &'static str, Entity<ViewTree>);

/// What `/inspect/*` asks.
#[derive(Debug, PartialEq)]
pub enum Ask {
    /// `GET /inspect/tree?in=W[:V]`: every node, or those in one window or
    /// view.
    Tree(Option<String>),
    Find(Find),
    Press(Press),
    Type(Type),
    Wait(Wait),
    /// `GET /inspect/screenshot?window=W`: the named window, else the first.
    Screenshot(Option<String>),
}

pub(super) async fn answer(ask: Ask, door: &Door<'_>, cx: &mut AsyncApp) -> Reply {
    match ask {
        Ask::Tree(scope) => {
            let scope = Find {
                scope,
                ..Default::default()
            };
            found(door, scope, cx).await
        }
        Ask::Find(find) => found(door, find, cx).await,
        Ask::Press(asked) => press(door, asked, cx).await,
        Ask::Type(asked) => typed(door, asked, cx).await,
        Ask::Wait(asked) => wait(door, asked, cx).await,
        Ask::Screenshot(window) => screenshot(door, window, cx).await,
    }
}

/// What a node is found by: each one given must hold. `id` is the whole
/// id; `role` matches as named, any case; `label` and `text` match a part
/// of the node's, any case; `in` keeps a window (`console`) or a view
/// (`console:chat`).
#[derive(Debug, Default, Deserialize, PartialEq)]
pub struct Find {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "in")]
    scope: Option<String>,
}

/// `POST /inspect/press`: what `press` sends (`click` unless named).
#[derive(Debug, Deserialize, PartialEq)]
pub struct Press {
    id: String,
    #[serde(default = "click")]
    press: String,
}

fn click() -> String {
    "click".to_owned()
}

/// `POST /inspect/type`: `text`, typed into the field `id`.
#[derive(Debug, Deserialize, PartialEq)]
pub struct Type {
    id: String,
    text: String,
}

/// `POST /inspect/wait`: until a node `find` names is there (or, with
/// `gone`, no longer is), or the client's deadline passed.
#[derive(Debug, Deserialize, PartialEq)]
pub struct Wait {
    #[serde(flatten)]
    find: Find,
    #[serde(default)]
    gone: bool,
    deadline_ms: u64,
}

/// A node as the door answers it: its id, `<window>:<view>/<path>`, and
/// what the renderer says of it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    pub id: String,
    #[serde(flatten)]
    pub node: Inspected,
}

impl Find {
    fn matches(&self, node: &Node) -> bool {
        let part = |want: &Option<String>, have: &Option<String>| {
            want.as_deref().is_none_or(|want| {
                have.as_deref()
                    .is_some_and(|have| have.to_lowercase().contains(&want.to_lowercase()))
            })
        };
        self.id.as_deref().is_none_or(|id| node.id == id)
            && self.role.as_deref().is_none_or(|role| {
                node.node
                    .role
                    .as_deref()
                    .is_some_and(|have| have.eq_ignore_ascii_case(role))
            })
            && part(&self.label, &node.node.label)
            && part(&self.text, &node.node.text)
            && self.scope.as_deref().is_none_or(|scope| {
                node.id
                    .strip_prefix(scope)
                    .is_some_and(|rest| rest.starts_with([':', '/']) || rest.is_empty())
            })
    }
}

/// Every seated view the door serves, each named as its ids begin: the
/// window's door name, then the program, `<program>~2` for a second pane
/// of one program in one window.
fn views(door: &Door<'_>, cx: &App) -> Vec<(String, AnyWindowHandle, Entity<ViewTree>)> {
    let windows = (door.windows)(cx);
    let mut seen: HashMap<(String, &'static str), usize> = HashMap::new();
    let mut named = Vec::new();
    for (handle, program, tree) in (door.views)(cx) {
        let Some(window) = windows
            .iter()
            .find_map(|(name, _, other)| (*other == handle).then(|| name.clone()))
        else {
            continue;
        };
        let count = seen.entry((window.clone(), program)).or_default();
        *count += 1;
        let view = match *count {
            1 => program.to_owned(),
            n => format!("{program}~{n}"),
        };
        named.push((format!("{window}:{view}"), handle, tree));
    }
    named
}

/// Every node of every view, once the seats have settled (or the client's
/// deadline for its call passed).
async fn read(door: &Door<'_>, until: Option<Instant>, cx: &mut AsyncApp) -> Vec<Node> {
    settled(door, until, cx).await;
    cx.update(|cx| {
        let mut nodes = Vec::new();
        for (view, handle, tree) in views(door, cx) {
            let read = handle.update(cx, |_, window, cx| tree.read(cx).inspect(window, cx));
            for node in read.unwrap_or_default() {
                let id = match node.path.is_empty() {
                    true => view.clone(),
                    false => format!("{view}/{}", node.path),
                };
                nodes.push(Node { id, node });
            }
        }
        nodes
    })
}

/// The view `id` is in and the node's path in it.
fn target(
    door: &Door<'_>,
    id: &str,
    cx: &App,
) -> Option<(AnyWindowHandle, Entity<ViewTree>, String)> {
    views(door, cx)
        .into_iter()
        .find_map(|(view, handle, tree)| {
            let rest = id.strip_prefix(&view)?;
            let path = match rest {
                "" => "",
                rest => rest.strip_prefix('/')?,
            };
            Some((handle, tree, path.to_owned()))
        })
}

fn missing(id: &str) -> Reply {
    Reply::new(404, json!({ "error": "no such node", "id": id }))
}

/// Every node `find` names, in document order.
async fn found(door: &Door<'_>, find: Find, cx: &mut AsyncApp) -> Reply {
    let nodes = read(door, None, cx).await;
    let kept: Vec<_> = nodes
        .into_iter()
        .filter(|node| find.matches(node))
        .collect();
    Reply::ok(json!(kept))
}

async fn press(door: &Door<'_>, press: Press, cx: &mut AsyncApp) -> Reply {
    settled(door, None, cx).await;
    let sent = cx.update(|cx| {
        let (handle, tree, path) = target(door, &press.id, cx).ok_or_else(|| missing(&press.id))?;
        handle
            .update(cx, |_, _, cx| {
                tree.update(cx, |tree, cx| tree.inspect_press(&path, &press.press, cx))
            })
            .map_err(|_| missing(&press.id))?
            .map_err(|error| Reply::new(400, json!({ "error": error })))
    });
    if let Err(reply) = sent {
        return reply;
    }
    settled(door, None, cx).await;
    Reply::ok(json!({ "pressed": press.id, "press": press.press }))
}

async fn typed(door: &Door<'_>, typed: Type, cx: &mut AsyncApp) -> Reply {
    settled(door, None, cx).await;
    let sent = cx.update(|cx| {
        let (handle, tree, path) = target(door, &typed.id, cx).ok_or_else(|| missing(&typed.id))?;
        handle
            .update(cx, |_, window, cx| {
                tree.update(cx, |tree, cx| {
                    tree.inspect_type(&path, &typed.text, window, cx)
                })
            })
            .map_err(|_| missing(&typed.id))?
            .map_err(|error| Reply::new(400, json!({ "error": error })))
    });
    if let Err(reply) = sent {
        return reply;
    }
    settled(door, None, cx).await;
    Reply::ok(json!({ "typed": typed.id }))
}

async fn wait(door: &Door<'_>, wait: Wait, cx: &mut AsyncApp) -> Reply {
    let until = deadline(wait.deadline_ms);
    loop {
        let nodes = read(door, until, cx).await;
        let found = nodes.into_iter().find(|node| wait.find.matches(node));
        match (found, wait.gone) {
            (Some(node), false) => return Reply::ok(json!({ "node": node })),
            (None, true) => return Reply::ok(json!({ "gone": true })),
            _ if passed(until) => {
                return Reply::new(408, json!({ "error": "deadline passed" }));
            }
            _ => cx.background_executor().timer(super::POLL).await,
        }
    }
}

/// `GET /inspect/screenshot?window`: the window as the platform last drew
/// it, a PNG in base64. The platform draws it (`Window::render_to_image`,
/// which gpui builds only with its `test-support`); a build or a platform
/// that cannot answers 501 in its own words.
async fn screenshot(door: &Door<'_>, window: Option<String>, cx: &mut AsyncApp) -> Reply {
    settled(door, None, cx).await;
    cx.update(|cx| {
        let served = (door.windows)(cx);
        let Some((name, _, handle)) = served
            .into_iter()
            .find(|(name, _, _)| window.as_deref().is_none_or(|want| want == name))
        else {
            return Reply::new(404, json!({ "error": "no such window" }));
        };
        let drawn = handle.update(cx, |_, window, _| picture(window));
        match drawn {
            Err(_) => Reply::new(404, json!({ "error": "no such window" })),
            Ok(Err(error)) => Reply::new(501, json!({ "error": error, "window": name })),
            Ok(Ok((width, height, png))) => Reply::ok(json!({
                "window": name,
                "width": width,
                "height": height,
                "png": base64(&png),
            })),
        }
    })
}

/// The window drawn to a PNG: its width, height and bytes.
#[cfg(any(test, feature = "test-support"))]
fn picture(window: &gpui_kit::Window) -> Result<(u32, u32, Vec<u8>), String> {
    let picture = window
        .render_to_image()
        .map_err(|error| error.to_string())?;
    let mut png = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok((picture.width(), picture.height(), png))
}

/// gpui renders a window to an image only in a build with its
/// `test-support` (`Window::render_to_image`).
#[cfg(not(any(test, feature = "test-support")))]
fn picture(_: &gpui_kit::Window) -> Result<(u32, u32, Vec<u8>), String> {
    Err("this build draws no picture of a window: gpui renders one only with its test-support feature; take the OS's screenshot".into())
}

/// Standard base64 with padding (RFC 4648 §4).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().enumerate().fold(0u32, |word, (at, byte)| {
            word | (u32::from(*byte) << (16 - 8 * at))
        });
        for at in 0..4 {
            match at <= chunk.len() {
                true => out.push(ALPHABET[((word >> (18 - 6 * at)) & 63) as usize] as char),
                false => out.push('='),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_rfc_4648() {
        for (bytes, text) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(bytes), text);
        }
    }

    fn node(id: &str, role: &str, label: &str) -> Node {
        Node {
            id: id.to_owned(),
            node: Inspected {
                role: Some(role.to_owned()),
                label: Some(label.to_owned()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn a_find_holds_every_part_it_names() {
        let send = node("console:chat/composer/send", "Button", "Send message");
        let find = |text: &str| serde_json::from_str::<Find>(text).unwrap().matches(&send);
        assert!(find(r#"{}"#));
        assert!(find(r#"{"role": "button", "label": "send"}"#));
        assert!(find(r#"{"in": "console"}"#));
        assert!(find(r#"{"in": "console:chat"}"#));
        assert!(find(r#"{"id": "console:chat/composer/send"}"#));
        assert!(!find(r#"{"in": "console:cha"}"#));
        assert!(!find(r#"{"role": "Link"}"#));
        assert!(!find(r#"{"label": "send", "text": "send"}"#));
        assert!(!find(r#"{"id": "console:chat/composer"}"#));
    }
}
