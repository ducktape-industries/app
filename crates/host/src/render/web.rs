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
//! - a view's manifest: [`seen`] records whose declares `web` (a frame
//!   with a web piece from any other is refused where every piece's
//!   capability is checked, `runtime/view/requests.rs`);
//! - a link: [`view_for`] names the view an https link opens in.

use super::*;
use std::collections::BTreeSet;
use std::sync::Mutex;
use view_wire::methods::Capability;

/// A CEF child process (this binary started with `--type=...`) runs CEF
/// and exits here; the app's own process returns at once.
pub fn subprocess() {
    #[cfg(feature = "web")]
    if let Some(code) = ducktape_web::child() {
        std::process::exit(code);
    }
}

/// CEF down, if a page started it: after the app's run returns.
pub fn shutdown() {
    #[cfg(feature = "web")]
    ducktape_web::shutdown();
}

/// The programs whose view's manifest declares `web`, as last read.
fn web_views() -> &'static Mutex<BTreeSet<&'static str>> {
    static VIEWS: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    &VIEWS
}

/// `program`'s view manifest was read: whether it declares `web`.
pub fn seen(program: &'static str, capabilities: &[Capability]) {
    let mut views = web_views().lock().expect("web views");
    match capabilities.contains(&Capability::Web) {
        true => views.insert(program),
        false => views.remove(program),
    };
}

/// The view an http(s) `link` opens in: the program whose view declares
/// `web` (the first by name, if more do), when this build has a web view
/// and the address is one a page loads. `None`: the system browser's.
pub fn view_for(link: &str) -> Option<&'static str> {
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
                    let state = told(state);
                    cx.emit(wire::Event::Web { handler, state });
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

/// What a view hears of its page. The page writes its address and title
/// (`history.pushState`, `document.title`), so both are bounded as
/// `wire::WebState` says: the title cleaned as a `host.title` is, the
/// address whole or else its origin (one cut short is another address):
/// the origin when it is not plain ASCII without control characters, as a
/// canonical address is. Never empty while the page has a site: a
/// same-document `pushState` meets no `loadable`, and an empty address
/// would hide which site the page is.
#[cfg(feature = "web")]
fn told(state: &ducktape_web::PageState) -> wire::WebState {
    let url = &state.url;
    let plain = url.is_ascii() && !url.contains(char::is_control);
    wire::WebState {
        url: match plain {
            true => url.clone(),
            false => ducktape_web::origin(url).unwrap_or_default(),
        },
        title: crate::runtime::shown_title(&state.title),
        loading: state.loading,
        can_go_back: state.can_go_back,
        can_go_forward: state.can_go_forward,
    }
}

/// A program's own cookies and cache: `<cache>/web/<program>`, the program
/// of the seat this tree draws for.
#[cfg(feature = "web")]
fn profile(key: Option<crate::perf::Key>) -> std::path::PathBuf {
    let program = match key {
        Some(crate::perf::Key::View { program, .. }) => program,
        _ => "unknown",
    };
    crate::backend::dirs().cache.join("web").join(program)
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

    /// What a page says of itself reaches its view bounded: a long or
    /// unprintable address as none, the title as a window title.
    #[cfg(feature = "web")]
    #[test]
    fn a_page_tells_its_view_a_bounded_state() {
        let page = |url: &str, title: &str| ducktape_web::PageState {
            url: url.into(),
            title: title.into(),
            loading: true,
            ..Default::default()
        };
        let state = told(&page(
            "https://example.com/a?b",
            "  Example\u{202e}\n page  ",
        ));
        assert_eq!(state.url, "https://example.com/a?b");
        assert_eq!(state.title, "Example page");
        assert!(state.loading);
        let long = format!("https://example.com/{}", "a".repeat(4096));
        assert_eq!(
            told(&page(&long, "")).url,
            long,
            "a long address is told whole"
        );
        // one that cannot be told whole is told as its site
        for url in ["https://example.com/\u{2028}", "https://example.com/\t"] {
            assert_eq!(told(&page(url, "")).url, "https://example.com/", "{url:?}");
        }
        let title = told(&page("", &"x".repeat(500))).title;
        assert_eq!(title.chars().count(), 500);
    }

    /// A build without the web view draws a web piece as its line, and a
    /// frame without the piece leaves nothing behind.
    #[cfg(not(feature = "web"))]
    #[gpui_kit::test]
    fn a_build_without_the_web_view_draws_the_piece_as_a_line(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::px;
        cx.update(gpui_kit::init);
        let window = cx.open_window(gpui_kit::size(px(200.), px(100.)), |_, _| {
            ViewTree::new(holder(vec![web()]))
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }
}
