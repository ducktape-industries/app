//! The web pieces (`wire::Node::Web`, `Node::Artifact`) on this host, and every
//! call the rest of the app makes into the web feature. With feature `web` a
//! piece is a `ducktape_web::Page` (crates/web, where all of CEF lives), one
//! per node (an artifact's per node and link), kept from the frame that first has it
//! to the frame that no longer does; without it a piece is its address as a
//! link and every https link goes to the system browser. Callers need
//! no `cfg`:
//! - `main`: [`subprocess`] first, [`shutdown`] after the app returns;
//! - the tree: [`ViewTree::web`] and [`ViewTree::artifact`] draw a piece, [`Pages::retain`] keeps a
//!   frame's pages;
//! - a view's manifest: [`seen`] records whose declares `web` (a frame
//!   with a web piece from any other is refused where every piece's
//!   capability is checked, `runtime/view/requests.rs`);
//! - a link: [`view_for`] names the view an https link opens in.

use super::*;
use crate::a11y::Control as _;

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

// which view may draw a page is the runtime's rule (a frame is judged as
// it lands, a manifest as it is read); the links follow it here
pub use crate::runtime::web::{seen, view_for};

pub struct Opened(pub String);

impl EventEmitter<Opened> for ViewTree {}

/// The pages of a tree's web pieces, by authored path.
#[derive(Default)]
pub(super) struct Pages {
    #[cfg(feature = "web")]
    pages: HashMap<Key, Piece>,
}

#[cfg(feature = "web")]
type Key = (AuthoredPath, Option<String>);

#[cfg(feature = "web")]
fn key(path: &AuthoredPath, node: &wire::Node) -> Option<Key> {
    match node {
        wire::Node::Web { .. } => Some((path.clone(), None)),
        wire::Node::Artifact { link, .. } => Some((path.clone(), Some(link.clone()))),
        _ => None,
    }
}

/// One web piece's page, the commands of its last revision applied, and
/// the route its state goes to.
#[cfg(feature = "web")]
struct Piece {
    page: Entity<ducktape_web::Page>,
    revision: u64,
    on_state: std::rc::Rc<std::cell::Cell<Option<u32>>>,
    height: std::rc::Rc<std::cell::Cell<Option<u32>>>,
    _state: Subscription,
    _leave: Subscription,
    _follow: Option<(crate::runtime::artifact::Following, gpui_kit::Task<()>)>,
}

impl Pages {
    /// Keeps the pages whose piece `root` still has; the others close.
    pub(super) fn retain(&mut self, root: &wire::Node) {
        #[cfg(feature = "web")]
        if !self.pages.is_empty() {
            let mut kept = std::collections::HashSet::new();
            commands::walk_authored_paths(root, None, &mut Vec::new(), &mut |node, path| {
                kept.extend(key(path, node));
            });
            self.pages.retain(|key, _| kept.contains(key));
        }
        let _ = root;
    }
}

impl ViewTree {
    pub(super) fn web(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Web {
            id,
            url,
            style,
            revision,
            commands,
            on_state,
        } = node
        else {
            unreachable!()
        };
        #[cfg(feature = "web")]
        {
            let piece = self.page(node, url, *on_state, false, window, cx);
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
            let page = piece.page.clone();
            if !page.read(cx).unavailable() {
                return div()
                    .refine_style(&self.style(*style))
                    .id(native_id(id))
                    .overflow_hidden()
                    .bg(ground(cx))
                    .child(page)
                    .into_any_element();
            }
        }
        let _ = (window, style, revision, commands, on_state);
        link_line(id, url, cx)
    }

    pub(super) fn artifact(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Artifact { id, link, style } = node else {
            unreachable!()
        };
        #[cfg(feature = "web")]
        if crate::runtime::artifact::serves(link) {
            let piece = self.page(node, link, None, true, window, cx);
            let (page, height) = (piece.page.clone(), piece.height.get());
            if !page.read(cx).unavailable() {
                let height =
                    height.map_or(window.viewport_size().height, |height| px(height as f32));
                let palette = crate::theme::palette(cx);
                return div()
                    .refine_style(&self.style(*style))
                    .id(native_id(id))
                    .overflow_hidden()
                    .border_1()
                    .border_color(crate::theme::hsla(palette.border))
                    .rounded(px(design::radius::CARD as f32))
                    .bg(crate::theme::hsla(palette.surface))
                    .child(div().w_full().h(height).child(page))
                    .into_any_element();
            }
        }
        let _ = (window, style);
        link_line(id, link, cx)
    }

    #[cfg(feature = "web")]
    fn page(
        &mut self,
        node: &wire::Node,
        address: &str,
        on_state: Option<u32>,
        follows: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> &mut Piece {
        ducktape_web::color_scheme(gpui_kit::component::Theme::global(cx).is_dark());
        let key = key(&self.authored_path, node).expect("a web piece");
        if !self.webs.pages.contains_key(&key) {
            let piece = open(address, on_state, follows, self.perf_key, window, cx);
            self.webs.pages.insert(key.clone(), piece);
        }
        let piece = self.webs.pages.get_mut(&key).expect("made above");
        piece.on_state.set(on_state);
        piece
    }
}

#[cfg(feature = "web")]
fn open(
    address: &str,
    on_state: Option<u32>,
    follows: bool,
    key: Option<crate::perf::Key>,
    window: &mut Window,
    cx: &mut Context<ViewTree>,
) -> Piece {
    use futures::StreamExt as _;
    ducktape_web::serve(ducktape_web::Network {
        read: Box::new(|address, answer| {
            crate::runtime::artifact::spawn_read(address, move |read| {
                answer(ducktape_web::Served {
                    status: read.status,
                    mime: read.mime.to_owned(),
                    body: read.body,
                })
            })
        }),
        serves: crate::runtime::artifact::serves,
    });
    let page = cx.new(|cx| ducktape_web::Page::new(address, profile(key), window, cx));
    let route = std::rc::Rc::new(std::cell::Cell::new(on_state));
    let height = std::rc::Rc::new(std::cell::Cell::new(None));
    let (heard, measured) = (route.clone(), height.clone());
    let state = cx.subscribe(&page, move |_, _, state: &ducktape_web::PageState, cx| {
        if measured.replace(state.height) != state.height {
            cx.notify();
        }
        if let Some(handler) = heard.get() {
            let state = told(state);
            cx.emit(wire::Event::Web { handler, state });
        }
    });
    let leave = cx.subscribe(&page, |_, _, leave: &ducktape_web::Leave, cx| {
        cx.emit(Opened(leave.0.clone()))
    });
    let follow = follows
        .then(|| crate::runtime::artifact::follow(address))
        .flatten()
        .map(|(following, mut changed)| {
            let shown = page.downgrade();
            let reloads = cx.spawn(async move |_, cx| {
                while changed.next().await.is_some() {
                    if shown.update(cx, |page, _| page.reload()).is_err() {
                        return;
                    }
                }
            });
            (following, reloads)
        });
    Piece {
        page,
        revision: 0,
        on_state: route,
        height,
        _state: state,
        _leave: leave,
        _follow: follow,
    }
}

#[cfg(feature = "web")]
fn ground(cx: &App) -> gpui_kit::Hsla {
    crate::theme::hsla(crate::theme::palette(cx).surface)
}

fn link_line(id: &wire::ElementIdWire, link: &str, cx: &mut Context<ViewTree>) -> AnyElement {
    let opened = link.to_owned();
    crate::a11y::keyboard(
        div()
            .id(native_id(id))
            .control(gpui_kit::Role::Link, link.to_owned())
            .w_full()
            .min_w_0()
            .truncate()
            .cursor_pointer()
            .text_color(crate::theme::hsla(crate::theme::palette(cx).link))
            .on_click(cx.listener(move |_, _: &gpui_kit::ClickEvent, _, cx| {
                cx.emit(Opened(opened.clone()))
            }))
            .child(link.to_owned()),
        crate::a11y::ink(cx),
    )
    .into_any_element()
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
    crate::runtime::carried(&crate::backend::dirs().cache.join("web"), program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "web"))]
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

    #[cfg(not(feature = "web"))]
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

    const PLAN: &str = "duck://duckpond-725bcad1/files/shared/plan.html";

    #[cfg(not(feature = "web"))]
    fn artifact() -> wire::Node {
        wire::Node::Artifact {
            id: wire::ElementIdWire::Name("plan".into()),
            link: PLAN.into(),
            style: crate::render::plain_style(),
        }
    }

    #[cfg(not(feature = "web"))]
    #[test]
    fn without_the_web_view_a_piece_is_its_link_which_a_key_opens() {
        use gpui_kit::px;
        for (node, link) in [(artifact(), PLAN), (web(), "https://example.com")] {
            let native = ducktape_a11y_platform::headless::Harness::open_with(
                gpui_kit::init,
                gpui_kit::size(px(400.), px(300.)),
                |_, _| ViewTree::new(holder(vec![node])),
            );
            let tree = native.root();
            let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let _opened = native.update(|_, cx| {
                let heard = heard.clone();
                cx.subscribe(&tree, move |_, opened: &Opened, _| {
                    heard.borrow_mut().push(opened.0.clone())
                })
            });
            let focused = native.update(|window, cx| {
                native.activate_in(window);
                crate::render::draw_frame(window, cx);
                window.focus_next(cx);
                crate::render::draw_frame(window, cx);
                let tree = &native.tree();
                tree.nodes
                    .iter()
                    .find(|(id, _)| *id == tree.focus)
                    .map(|(_, node)| (node.role(), node.label().map(str::to_owned)))
            });
            assert_eq!(
                focused,
                Some((gpui_kit::accesskit::Role::Link, Some(link.to_owned())))
            );
            let keystroke = gpui_kit::Keystroke::parse("enter").unwrap();
            native.simulate_event(gpui_kit::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            native.simulate_event(gpui_kit::KeyUpEvent { keystroke });
            assert_eq!(*heard.borrow(), [link]);
        }
    }

    #[cfg(feature = "web")]
    #[test]
    fn an_artifact_keeps_its_page_while_its_link_holds_and_a_web_piece_while_its_node_does() {
        let path = AuthoredPath::default();
        let artifact = |link: &str| wire::Node::Artifact {
            id: wire::ElementIdWire::Name("plan".into()),
            link: link.into(),
            style: crate::render::plain_style(),
        };
        let web = |url: &str| wire::Node::Web {
            id: wire::ElementIdWire::Name("page".into()),
            url: url.into(),
            style: crate::render::plain_style(),
            revision: 0,
            commands: Vec::new(),
            on_state: None,
        };
        let other = "duck://duckpond-725bcad1/files/shared/other.html";
        assert_eq!(key(&path, &artifact(PLAN)), key(&path, &artifact(PLAN)));
        assert_ne!(key(&path, &artifact(PLAN)), key(&path, &artifact(other)));
        assert_eq!(
            key(&path, &web("https://example.com")),
            key(&path, &web("https://example.org"))
        );
        assert_eq!(key(&path, &wire::Node::Space), None);
    }

    #[test]
    fn an_artifact_is_a_file_or_a_blob_of_a_network() {
        use crate::runtime::artifact::serves;
        assert!(serves(PLAN));
        assert!(serves(&format!("{PLAN}?v=2")));
        assert!(serves(&format!(
            "duck://duckpond-725bcad1/blobs/{}",
            "ab".repeat(32)
        )));
        for address in [
            "duck://duckpond-725bcad1/chat/general",
            "duck://duckpond-725bcad1/files",
            "duck://duckpond-725bcad1/blobs/ab",
            "duck://chat",
            "https://example.com/plan.html",
        ] {
            assert!(!serves(address), "{address}");
        }
    }
}
