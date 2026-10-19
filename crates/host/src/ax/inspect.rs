//! The inspector's door: every view's tree as the host holds it and the
//! shell's own controls as they were drawn, found, read and pressed by id,
//! and a window drawn to a picture. It reads no accessibility tree.

use super::natives::{self, Drawn};
use super::shown::{self, Shown};
use super::{Door, Reply, deadline, passed, settled};
use crate::render::{Inspected, ViewTree};
use gpui_kit::{AnyWindowHandle, App, AsyncApp, Entity, Pixels, Point, point, px};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
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
    Scroll(Scroll),
    /// `GET /inspect/screenshot?window=W`: the named window, else the first.
    Screenshot(Option<String>),
    Memory {
        regions: bool,
    },
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
        Ask::Scroll(asked) => scroll(door, asked, cx).await,
        Ask::Screenshot(window) => screenshot(door, window, cx).await,
        Ask::Memory { regions } => memory(door, regions, cx),
    }
}

fn memory(door: &Door<'_>, regions: bool, cx: &mut AsyncApp) -> Reply {
    cx.update(|cx| {
        let windows: Vec<_> = (door.windows)(cx)
            .into_iter()
            .map(|(name, _, handle)| (name, handle))
            .collect();
        let seated = views(door, cx);
        let mut reading = crate::memory::read(regions);
        crate::memory::drawn(&mut reading, &seated, &windows, cx);
        Reply::ok(json!(reading))
    })
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

#[derive(Debug, Deserialize, PartialEq)]
pub struct Scroll {
    id: String,
    #[serde(default)]
    by: Option<By>,
    #[serde(default)]
    to: Option<End>,
    #[serde(default)]
    deadline_ms: Option<u64>,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct By {
    #[serde(default)]
    x: f32,
    #[serde(default)]
    y: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum End {
    Start,
    End,
}

/// A node as the door answers it: its id, `<window>:<view>/<path>` for a
/// view's node or `<window>:<element id>` for one of the shell's own, and
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
        let Some(window) = window_name(&windows, handle) else {
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

/// The door's name for the window `handle`, when it serves it.
fn window_name(windows: &[super::Served], handle: AnyWindowHandle) -> Option<String> {
    windows
        .iter()
        .find_map(|(name, _, other)| (*other == handle).then(|| name.clone()))
}

/// Where a node is: in a view's tree at a path, or a native element.
enum Place {
    View(AnyWindowHandle, Entity<ViewTree>, String),
    Native(AnyWindowHandle, Rc<RefCell<Drawn>>),
}

/// What the inspector says of a native element.
fn native(drawn: &Drawn) -> Inspected {
    Inspected {
        path: drawn.at.clone(),
        kind: "native",
        role: drawn.native.role.map(|role| format!("{role:?}")),
        label: drawn.native.label.as_ref().map(ToString::to_string),
        value: drawn.native.value.clone(),
        presses: drawn.presses(),
        ..Default::default()
    }
}

/// Every node of every view, then every native element, each with its
/// place; an id two of them share is in it twice.
fn every(door: &Door<'_>, cx: &mut App) -> Vec<(Node, Place)> {
    let mut nodes = Vec::new();
    for (view, handle, tree) in views(door, cx) {
        let read = handle.update(cx, |_, window, cx| tree.read(cx).inspect(window, cx));
        for node in read.unwrap_or_default() {
            let id = match node.path.is_empty() {
                true => view.clone(),
                false => format!("{view}/{}", node.path),
            };
            let place = Place::View(handle, tree.clone(), node.path.clone());
            nodes.push((Node { id, node }, place));
        }
    }
    let windows = (door.windows)(cx);
    for drawn in natives::drawn(cx) {
        let (handle, node) = {
            let seen = drawn.borrow();
            let Some(window) = window_name(&windows, seen.shown.window) else {
                continue;
            };
            let id = format!("{window}:{}", seen.id);
            (
                seen.shown.window,
                Node {
                    id,
                    node: native(&seen),
                },
            )
        };
        nodes.push((node, Place::Native(handle, drawn)));
    }
    nodes
}

/// Every node whose id no other node has, once the seats have settled (or
/// the client's deadline for its call passed). A shared id names no one
/// node, so neither is read; pressing it says which they are.
async fn read(door: &Door<'_>, until: Option<Instant>, cx: &mut AsyncApp) -> Vec<Node> {
    settled(door, until, cx).await;
    cx.update(|cx| {
        unshared(every(door, cx))
            .into_iter()
            .map(|(node, _)| node)
            .collect()
    })
}

/// The nodes whose id no other node has.
fn unshared<T>(nodes: Vec<(Node, T)>) -> Vec<(Node, T)> {
    let mut count: HashMap<String, usize> = HashMap::new();
    for (node, _) in &nodes {
        *count.entry(node.id.clone()).or_default() += 1;
    }
    nodes
        .into_iter()
        .filter(|(node, _)| count[&node.id] == 1)
        .collect()
}

/// Where the one node `id` names is.
fn target(door: &Door<'_>, id: &str, cx: &mut App) -> Result<Place, Reply> {
    the_one(id, every(door, cx))
}

/// The one node of `nodes` that has `id`: 404 when none has it, 409
/// naming each one when more than one does.
fn the_one<T>(id: &str, nodes: Vec<(Node, T)>) -> Result<T, Reply> {
    let mut named: Vec<_> = nodes
        .into_iter()
        .filter(|(node, _)| node.id == id)
        .collect();
    match named.len() {
        0 => Err(missing(id)),
        1 => Ok(named.remove(0).1),
        _ => {
            let nodes: Vec<_> = named
                .iter()
                .map(|(node, _)| {
                    json!({
                        "kind": node.node.kind,
                        "path": node.node.path,
                        "role": node.node.role,
                        "label": node.node.label,
                    })
                })
                .collect();
            Err(Reply::new(
                409,
                json!({ "error": "more than one node has this id", "id": id, "nodes": nodes }),
            ))
        }
    }
}

fn missing(id: &str) -> Reply {
    Reply::new(404, json!({ "error": "no such node", "id": id }))
}

fn refused((status, error): natives::Refused) -> Reply {
    Reply::new(status, json!({ "error": error }))
}

fn shown_of(place: &Place, cx: &App) -> Option<Shown> {
    match place {
        Place::View(_, tree, address) => {
            let path = tree.read(cx).authored(address)?;
            shown::view(tree.entity_id(), &path, cx)
        }
        Place::Native(_, drawn) => Some(drawn.borrow().shown.clone()),
    }
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
    if let Err(reply) = into_view(door, &press.id, cx).await {
        return reply;
    }
    let sent = cx.update(|cx| match target(door, &press.id, cx)? {
        Place::View(handle, tree, path) => handle
            .update(cx, |_, _, cx| {
                tree.update(cx, |tree, cx| tree.inspect_press(&path, &press.press, cx))
            })
            .map_err(|_| missing(&press.id))?
            .map_err(|error| Reply::new(400, json!({ "error": error }))),
        Place::Native(handle, drawn) => handle
            .update(cx, |_, window, cx| {
                natives::press(&drawn, &press.press, window, cx)
            })
            .map_err(|_| missing(&press.id))?
            .map_err(refused),
    });
    if let Err(reply) = sent {
        return reply;
    }
    settled(door, None, cx).await;
    Reply::ok(json!({ "pressed": press.id, "press": press.press }))
}

async fn typed(door: &Door<'_>, typed: Type, cx: &mut AsyncApp) -> Reply {
    settled(door, None, cx).await;
    if let Err(reply) = into_view(door, &typed.id, cx).await {
        return reply;
    }
    let sent = cx.update(|cx| match target(door, &typed.id, cx)? {
        Place::View(handle, tree, path) => handle
            .update(cx, |_, window, cx| {
                tree.update(cx, |tree, cx| {
                    tree.inspect_type(&path, &typed.text, window, cx)
                })
            })
            .map_err(|_| missing(&typed.id))?
            .map_err(|error| Reply::new(400, json!({ "error": error }))),
        Place::Native(handle, drawn) => handle
            .update(cx, |_, window, cx| {
                natives::type_into(&drawn, &typed.text, window, cx)
            })
            .map_err(|_| missing(&typed.id))?
            .map_err(refused),
    });
    if let Err(reply) = sent {
        return reply;
    }
    settled(door, None, cx).await;
    Reply::ok(json!({ "typed": typed.id }))
}

async fn scroll(door: &Door<'_>, scroll: Scroll, cx: &mut AsyncApp) -> Reply {
    let given = (
        scroll.by.is_some(),
        scroll.to.is_some(),
        scroll.deadline_ms.is_some(),
    );
    if !matches!(given, (true, false, _) | (false, true, true)) {
        return Reply::new(
            400,
            json!({ "error": "give by {x, y}, or to start|end with deadline_ms" }),
        );
    }
    let until = scroll.deadline_ms.and_then(deadline);
    settled(door, until, cx).await;
    let aimed = cx.update(|cx| -> Result<_, Reply> {
        let place = target(door, &scroll.id, cx)?;
        let drawn = shown_of(&place, cx)
            .ok_or_else(|| refused((409, format!("{} is not drawn", scroll.id))))?;
        let at = drawn
            .window
            .update(cx, |_, window, _| drawn.aim(&scroll.id, window))
            .map_err(|_| missing(&scroll.id))?
            .map_err(refused)?;
        Ok((drawn.window, at, drawn.visible().size.height))
    });
    let (window, at, height) = match aimed {
        Ok(aimed) => aimed,
        Err(reply) => return reply,
    };
    let by = match (&scroll.by, scroll.to) {
        (Some(by), _) => point(px(by.x), px(by.y)),
        (None, Some(End::Start)) => point(Pixels::ZERO, height),
        (None, _) => point(Pixels::ZERO, -height),
    };
    if scroll.to.is_none() {
        let moved = wheeled(door, window, (at, by), until, cx).await;
        return Reply::ok(json!({ "scrolled": scroll.id, "moved": moved }));
    }
    let mut wheels = 0u64;
    while wheeled(door, window, (at, by), until, cx).await {
        wheels += 1;
        if passed(until) {
            return Reply::new(408, json!({ "error": "deadline passed", "wheels": wheels }));
        }
    }
    Reply::ok(json!({ "scrolled": scroll.id, "to": scroll.to, "wheels": wheels }))
}

async fn wheeled(
    door: &Door<'_>,
    window: AnyWindowHandle,
    (at, by): (Point<Pixels>, Point<Pixels>),
    until: Option<Instant>,
    cx: &mut AsyncApp,
) -> bool {
    let before = cx.update(|cx| {
        let _ = window.update(cx, |_, window, cx| shown::redraw(window, cx));
        shown::seen(window, cx)
    });
    let _ = cx.update(|cx| window.update(cx, |_, window, cx| shown::wheel(at, by, window, cx)));
    settled(door, until, cx).await;
    cx.update(|cx| {
        let _ = window.update(cx, |_, window, cx| shown::redraw(window, cx));
        before != shown::seen(window, cx)
    })
}

async fn into_view(door: &Door<'_>, id: &str, cx: &mut AsyncApp) -> Result<(), Reply> {
    let mut shown_before: Option<f32> = None;
    loop {
        let drawn = cx.update(|cx| target(door, id, cx).map(|place| shown_of(&place, cx)))?;
        let Some(drawn) = drawn else {
            return Ok(());
        };
        let by = drawn.toward();
        let area = drawn.area();
        let there = by.x == Pixels::ZERO && by.y == Pixels::ZERO;
        if there || shown_before.is_some_and(|before| area <= before) {
            return match there || area > 0. {
                true => Ok(()),
                false => Err(Reply::new(
                    409,
                    json!({ "error": format!("{id} is not shown, and a wheel at what shows it brings it no closer") }),
                )),
            };
        }
        shown_before = Some(area);
        let at = drawn.mask.center();
        cx.update(|cx| {
            drawn
                .window
                .update(cx, |_, window, cx| shown::wheel(at, by, window, cx))
        })
        .map_err(|_| missing(id))?;
        settled(door, None, cx).await;
        cx.update(|cx| {
            drawn
                .window
                .update(cx, |_, window, cx| shown::redraw(window, cx))
        })
        .map_err(|_| missing(id))?;
    }
}

async fn wait(door: &Door<'_>, wait: Wait, cx: &mut AsyncApp) -> Reply {
    let until = deadline(wait.deadline_ms);
    loop {
        let nodes = read(door, until, cx).await;
        let found = nodes.iter().find(|node| wait.find.matches(node));
        match (found, wait.gone) {
            (Some(node), false) => return Reply::ok(json!({ "node": node })),
            (None, true) => return Reply::ok(json!({ "gone": true })),
            _ if passed(until) => {
                return Reply::new(408, json!({ "error": "deadline passed", "tree": nodes }));
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

    /// An id two nodes share is read as neither, and asking for it names
    /// both; an id one node has finds it.
    #[test]
    fn a_shared_id_names_no_one_node() {
        let mut chat = node("console:send", "Button", "Send");
        chat.node.path = "chat.send".into();
        let mut shell = node("console:send", "Button", "Send it");
        shell.node.path = "desk.send".into();
        let close = node("console:close", "Button", "Close");
        let nodes = || vec![(chat.clone(), 1), (close.clone(), 2), (shell.clone(), 3)];
        let read: Vec<_> = unshared(nodes()).into_iter().map(|(_, at)| at).collect();
        assert_eq!(read, [2]);
        assert_eq!(the_one("console:close", nodes()).ok(), Some(2));
        assert_eq!(the_one("console:gone", nodes()).err().unwrap().status, 404);
        let shared = the_one("console:send", nodes()).err().unwrap();
        assert_eq!(shared.status, 409);
        assert!(
            shared.body.contains("chat.send") && shared.body.contains("desk.send"),
            "{}",
            shared.body
        );
    }
}
