//! The web piece (`wire::Node::Web`) on this host, and every call the rest
//! of the app makes into the web feature. With feature `web` a piece is a
//! `ducktape_web::Page` (crates/web, where all of CEF lives), one per node,
//! kept by the node's authored path from the frame that first has it to the
//! frame that no longer does; without it the piece says the build has no
//! web view and every https link goes to the system browser. Callers need
//! no `cfg`:
//! - `main`: [`subprocess`] first, [`shutdown`] after the app returns;
//! - the tree: [`ViewTree::web`] draws a piece, [`Pages::retain`] keeps a
//!   frame's pages;
//! - a view's frame: [`declared`] refuses a web piece from a view whose
//!   manifest does not declare `web`; [`seen`] records whose does;
//! - a link: [`view_for`] names the view an https link opens in.

use super::*;
use std::collections::BTreeSet;
use std::sync::Mutex;
use view_wire::methods::Capability;

/// A CEF child process (this binary started with `--type=...`) runs CEF
/// and exits here; the app's own process returns at once.
pub(crate) fn subprocess() {
    #[cfg(feature = "web")]
    if let Some(code) = ducktape_web::child() {
        std::process::exit(code);
    }
}

/// CEF down, if a page started it: after the app's run returns.
pub(crate) fn shutdown() {
    #[cfg(feature = "web")]
    ducktape_web::shutdown();
}

/// Why `frame` is refused, if it draws a web piece (in its tree, a patch
/// or a tooltip) and its view's manifest does not declare `web`: a view
/// draws a page only by saying it does. The refusal ends the view, as a
/// frame that does not decode does: sent again, it would be refused again.
pub(crate) fn declared(frame: &wire::Frame, capabilities: &[Capability]) -> Result<(), String> {
    fn has_web(node: &wire::Node) -> bool {
        matches!(node, wire::Node::Web { .. }) || node.children().iter().any(has_web)
    }
    if capabilities.contains(&Capability::Web) {
        return Ok(());
    }
    let patched = frame.patches.iter().filter_map(|patch| match patch {
        wire::Patch::Replace { node, .. }
        | wire::Patch::Props { node, .. }
        | wire::Patch::Insert { node, .. } => Some(node),
        wire::Patch::Remove { .. } | wire::Patch::Move { .. } => None,
    });
    let tooltips = frame
        .tooltip_responses
        .iter()
        .filter_map(|response| response.content.as_deref());
    match frame
        .root
        .iter()
        .chain(patched)
        .chain(tooltips)
        .any(has_web)
    {
        true => Err("the view draws a web page, and its manifest does not declare `web`".into()),
        false => Ok(()),
    }
}

/// The modules whose view's manifest declares `web`, as last read.
fn web_views() -> &'static Mutex<BTreeSet<&'static str>> {
    static VIEWS: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    &VIEWS
}

/// `module`'s view manifest was read: whether it declares `web`.
pub(crate) fn seen(module: &'static str, capabilities: &[Capability]) {
    let mut views = web_views().lock().expect("web views");
    match capabilities.contains(&Capability::Web) {
        true => views.insert(module),
        false => views.remove(module),
    };
}

/// The view an http(s) `link` opens in: the module whose view declares
/// `web` (the first by name, if more do), when this build has a web view
/// and the address is one a page loads. `None`: the system browser's.
pub(crate) fn view_for(link: &str) -> Option<&'static str> {
    if !loadable(link) {
        return None;
    }
    web_views().lock().expect("web views").first().copied()
}

fn loadable(link: &str) -> bool {
    #[cfg(feature = "web")]
    return ducktape_web::loadable(link);
    #[cfg(not(feature = "web"))]
    {
        let _ = link;
        false
    }
}

/// The pages of a tree's web pieces, by authored path.
#[derive(Default)]
pub(super) struct Pages {
    #[cfg(feature = "web")]
    pages: HashMap<AuthoredPath, Piece>,
}

/// One web piece's page, the commands of its last revision applied, and
/// the route its state goes to.
#[cfg(feature = "web")]
struct Piece {
    page: Entity<ducktape_web::Page>,
    revision: u64,
    on_state: std::rc::Rc<std::cell::Cell<Option<u32>>>,
    _state: Subscription,
}

impl Pages {
    /// Keeps the pages whose piece `root` still has; the others close.
    pub(super) fn retain(&mut self, root: &wire::Node) {
        #[cfg(feature = "web")]
        if !self.pages.is_empty() {
            let mut kept = std::collections::HashSet::new();
            commands::walk_authored_paths(root, None, &mut Vec::new(), &mut |node, path| {
                if matches!(node, wire::Node::Web { .. }) {
                    kept.insert(path.clone());
                }
            });
            self.pages.retain(|path, _| kept.contains(path));
        }
        let _ = root;
    }
}

impl ViewTree {
    /// A web piece: its page in the node's box, or the line that says this
    /// build has none.
    pub(super) fn web(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Web { id, style, .. } = node else {
            unreachable!()
        };
        let element = div()
            .refine_style(&self.style(*style))
            .id(native_id(id))
            .overflow_hidden();
        #[cfg(feature = "web")]
        let element = element.child(self.page(node, window, cx));
        #[cfg(not(feature = "web"))]
        let element = {
            let _ = (window, cx);
            element
                .flex()
                .items_center()
                .justify_center()
                .child("This build has no web view")
        };
        element.into_any_element()
    }

    /// The node's page: made the first time its path is drawn, then handed
    /// the commands of each revision it has not seen.
    #[cfg(feature = "web")]
    fn page(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ducktape_web::Page> {
        let wire::Node::Web {
            url,
            revision,
            commands,
            on_state,
            ..
        } = node
        else {
            unreachable!()
        };
        let path = self.authored_path.clone();
        if !self.webs.pages.contains_key(&path) {
            let profile = profile(self.perf_key);
            let page = cx.new(|cx| ducktape_web::Page::new(url, profile, window, cx));
            let route = std::rc::Rc::new(std::cell::Cell::new(*on_state));
            let heard = route.clone();
            let state = cx.subscribe(&page, move |_, _, state: &ducktape_web::PageState, cx| {
                if let Some(handler) = heard.get() {
                    cx.emit(wire::Event::Web {
                        handler,
                        state: wire::WebState {
                            url: state.url.clone(),
                            title: state.title.clone(),
                            loading: state.loading,
                            can_go_back: state.can_go_back,
                            can_go_forward: state.can_go_forward,
                        },
                    });
                }
            });
            let piece = Piece {
                page,
                revision: 0,
                on_state: route,
                _state: state,
            };
            self.webs.pages.insert(path.clone(), piece);
        }
        let piece = self.webs.pages.get_mut(&path).expect("made above");
        piece.on_state.set(*on_state);
        if piece.revision != *revision {
            piece.revision = *revision;
            piece.page.update(cx, |page, cx| {
                for command in commands {
                    match command {
                        wire::WebCommand::Load(url) => page.load(url, cx),
                        wire::WebCommand::Back => page.back(),
                        wire::WebCommand::Forward => page.forward(),
                        wire::WebCommand::Reload => page.reload(),
                        wire::WebCommand::Stop => page.stop(),
                    }
                }
            });
        }
        piece.page.clone()
    }
}

/// A program's own cookies and cache: `<cache>/web/<module>`, the module
/// of the seat this tree draws for.
#[cfg(feature = "web")]
fn profile(key: Option<crate::perf::Key>) -> std::path::PathBuf {
    let module = match key {
        Some(crate::perf::Key::View { module, .. }) => module,
        _ => "unknown",
    };
    crate::backend::cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("ducktape"))
        .join("web")
        .join(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web() -> wire::Node {
        wire::Node::Web {
            id: wire::ElementIdWire::Name("page".into()),
            url: "https://example.com".into(),
            style: crate::render::plain_style(),
            revision: 0,
            commands: Vec::new(),
            on_state: None,
        }
    }

    fn holder(children: Vec<wire::Node>) -> wire::Node {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("root".into())),
            style: crate::render::plain_style(),
            interactivity: Default::default(),
            children,
        })
    }

    /// A web piece anywhere in a frame (its tree, a patch, a tooltip) is
    /// refused from a view that does not declare `web`, and taken from one
    /// that does; a frame with none is everyone's.
    #[test]
    fn a_web_piece_needs_the_web_capability() {
        let undeclared = [Capability::Host];
        let declaring = [Capability::Host, Capability::Web];
        let tree = wire::Frame {
            root: Some(holder(vec![holder(vec![web()])])),
            ..Default::default()
        };
        let patch = wire::Frame {
            patches: vec![wire::Patch::Insert {
                path: Vec::new(),
                index: 0,
                node: web(),
            }],
            ..Default::default()
        };
        let tooltip = wire::Frame {
            tooltip_responses: vec![wire::TooltipResponse {
                request: 1,
                character_index: None,
                content: Some(Box::new(web())),
            }],
            ..Default::default()
        };
        for frame in [&tree, &patch, &tooltip] {
            assert!(declared(frame, &undeclared).is_err(), "{frame:?}");
            assert!(declared(frame, &declaring).is_ok());
        }
        let plain = wire::Frame {
            root: Some(holder(Vec::new())),
            ..Default::default()
        };
        assert!(declared(&plain, &undeclared).is_ok());
    }
}
