//! Chat rows holding an artifact, drawn by the host from chat-view's own frame
//! with the page live in CEF: each page sits in its row at the height it tells,
//! with no more under it than under a row of words, and a press on a link in it reaches the app.

use std::cell::RefCell;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ducktape_host::render::{Tree, ViewTree, pointer, web::Opened};
use ducktape_web::{Network, Page, PageState, Served};
use gpui_kit::accesskit::{Rect, Role};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
    StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, WindowHandle, div,
    point, px,
};

const INNER: &str = "DUCKTAPE_HOST_ARTIFACT_ROW_INNER";
const FRAME_LINE: f64 = 1.;
const FRAME: &str = include_str!("../testdata/chat-artifact-row.json");
const OUTLINE: &str = include_str!("../../web/testdata/outline.html");
const PAGE: &str = "duck://testnet-0a1b2c3d/files/shared/artifacts/plan.html";
const SITE: &str = "duck://testnet-0a1b2c3d/files/";
const ROOM: &str = "duck://testnet-0a1b2c3d/chat/general";
const SHORT: &str = "<!doctype html><meta charset=utf-8><title>loading</title><body style=\"margin: 0\"><div style=\"height: 300px\"></div></body></html>";
const LINKED: &str = "<a href=\"ROOM\" style=\"position: fixed; inset: 0\">the room</a><script>const size = () => { document.title = 'size:' + innerHeight + ':' + Math.ceil(document.documentElement.getBoundingClientRect().height); }; addEventListener('resize', size); new ResizeObserver(size).observe(document.documentElement); addEventListener('mousedown', (event) => { document.title = 'down:' + (event.target.closest('a') ? 'link' : event.target.tagName) + ':' + event.clientX + ',' + event.clientY; }, true);</script></html>";

const CASES: &[(&str, &str)] = &[("short", SHORT), ("outline", OUTLINE)];

static SHOWN: AtomicUsize = AtomicUsize::new(0);

fn main() {
    if let Some(code) = ducktape_web::child() {
        std::process::exit(code);
    }
    let code = match std::env::var_os(INNER) {
        Some(_) => inner(),
        None => outer(),
    };
    std::process::exit(code);
}

fn outer() -> i32 {
    let exe = std::env::current_exe().expect("this test's binary");
    let runtime = exe
        .parent()
        .and_then(Path::parent)
        .expect("the directory cef-dll-sys copies CEF's runtime files into");
    let runner = runtime.join("ducktape-host-artifact-row");
    let _ = std::fs::remove_file(&runner);
    if std::fs::hard_link(&exe, &runner).is_err() {
        std::fs::copy(&exe, &runner).expect("this test's binary beside CEF's runtime files");
    }
    let code = Command::new(&runner)
        .arg("--ozone-platform=headless")
        .env(INNER, "1")
        .status()
        .expect("the run beside CEF's runtime files")
        .code()
        .unwrap_or(1);
    let _ = std::fs::remove_file(&runner);
    code
}

fn served(address: &str) -> Served {
    match address == PAGE {
        true => {
            let (_, page) = CASES[SHOWN.load(Ordering::SeqCst)];
            let linked = LINKED.replace("ROOM", ROOM);
            Served {
                status: 200,
                mime: "text/html; charset=utf-8".to_owned(),
                body: page.replace("</html>", &linked).into_bytes(),
            }
        }
        false => Served {
            status: 404,
            mime: "text/plain".to_owned(),
            body: Vec::new(),
        },
    }
}

fn serves(address: &str) -> bool {
    address.starts_with(SITE)
}

struct Mount(Entity<ViewTree>);

impl Render for Mount {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(pointer::Layer).child(
            self.0
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Place {
    top: f64,
    bottom: f64,
    left: f64,
    right: f64,
}

impl Place {
    fn of(rect: Rect, scale: f64) -> Self {
        Self {
            top: rect.y0 / scale,
            bottom: rect.y1 / scale,
            left: rect.x0 / scale,
            right: rect.x1 / scale,
        }
    }

    fn height(&self) -> f64 {
        self.bottom - self.top
    }
}

#[derive(Default)]
struct Seen {
    rows: Vec<(usize, Place)>,
    page: Option<Place>,
    page_focused: bool,
    under: Option<Place>,
    height: f64,
}

impl Seen {
    fn row(&self, at: usize) -> Option<Place> {
        self.rows
            .iter()
            .find(|(position, _)| *position == at)
            .map(|(_, place)| *place)
    }
}

struct Room {
    cx: TestAppContext,
    visual: Option<VisualTestContext>,
    view: Option<Entity<ViewTree>>,
    opened: Rc<RefCell<Vec<String>>>,
    heard: Rc<RefCell<Vec<String>>>,
    told: String,
}

impl Room {
    fn look(&mut self) -> Seen {
        let visual = self.visual.as_mut().expect("a room drawn");
        visual.update(|window, cx| {
            window.activate_a11y();
            window.draw(cx).clear(cx);
            let scale = f64::from(window.scale_factor());
            let mut seen = Seen {
                height: f64::from(window.viewport_size().height),
                ..Seen::default()
            };
            let Some(tree) = window.a11y_tree() else {
                return seen;
            };
            let mut page = None;
            for (id, node) in tree.nodes.iter() {
                let Some(rect) = node.bounds() else {
                    continue;
                };
                let at = Place::of(rect, scale);
                if let (Role::Row, Some(position)) = (node.role(), node.position_in_set()) {
                    seen.rows.push((position, at));
                }
                let words = node.value().or(node.label());
                let the_words = words == Some("said under the page") && node.role() != Role::Row;
                if the_words {
                    seen.under = Some(at);
                }
                let last = window
                    .a11y_element_id(*id)
                    .and_then(|element| element.iter().last().map(|last| format!("{last:?}")));
                if last.is_some_and(|last| last.contains("\"web-page\"")) {
                    seen.page = Some(at);
                    page = Some(*id);
                }
            }
            let nodes: std::collections::HashMap<_, _> =
                tree.nodes.iter().map(|(id, node)| (*id, node)).collect();
            let mut inside: Vec<_> = page.into_iter().collect();
            while let Some(id) = inside.pop() {
                if id == tree.focus {
                    seen.page_focused = true;
                }
                if let Some(node) = nodes.get(&id) {
                    inside.extend(node.children().iter().copied());
                }
            }
            seen
        })
    }

    fn pages(&self) -> Vec<Entity<Page>> {
        let view = self.view.as_ref().expect("a room drawn");
        view.read_with(&self.cx, |tree, _| tree.pages_for_test())
    }

    fn state(&self, page: &Entity<Page>) -> PageState {
        page.read_with(&self.cx, |page, _| page.state().clone())
    }

    fn until(&mut self, mut done: impl FnMut(&Self, &Seen) -> bool) -> Seen {
        loop {
            let seen = self.look();
            let state = self.pages().first().map(|page| self.state(page));
            let told = format!(
                "page {:?} focused {} rows {:?} state {:?}",
                seen.page,
                seen.page_focused,
                seen.rows,
                state.map(|state| (state.url, state.title, state.loading, state.height))
            );
            if told != self.told {
                println!("artifact-row: {told}");
                self.told = told;
            }
            if done(self, &seen) {
                return seen;
            }
            self.cx.dispatcher.run_until_parked();
            self.cx.dispatcher.advance_clock_to_next_timer();
        }
    }

    fn open(&mut self) -> WindowHandle<Mount> {
        let mut frame: view_wire::Frame = serde_json::from_str(FRAME).expect("chat-view's frame");
        let mut styles = view_wire::Styles::default();
        view_wire::sanitize(&mut frame, &mut styles).expect("a frame the host takes");
        let tree = Tree {
            root: frame.root.expect("a root"),
            styles,
        };
        let window = self
            .cx
            .add_window(move |_, cx| Mount(cx.new(|_| ViewTree::new(tree))));
        let mount = window.root(&mut self.cx).expect("the room in its window");
        let view = mount.read_with(&self.cx, |mount, _| mount.0.clone());
        self.cx.update(|cx| {
            let opened = self.opened.clone();
            cx.subscribe(&view, move |_, event: &Opened, _| {
                println!("artifact-row: the app was handed {}", event.0);
                opened.borrow_mut().push(event.0.clone())
            })
            .detach();
            let heard = self.heard.clone();
            cx.subscribe(&view, move |_, event: &view_wire::Event, _| {
                println!("artifact-row: the view heard {event:?}");
                heard.borrow_mut().push(format!("{event:?}"))
            })
            .detach();
        });
        self.visual = Some(VisualTestContext::from_window(window.into(), &self.cx));
        self.view = Some(view);
        self.opened.borrow_mut().clear();
        window
    }

    fn close(&mut self, window: WindowHandle<Mount>) {
        self.visual = None;
        self.view = None;
        let _ = self
            .cx
            .update_window(window.into(), |_, window, _| window.remove_window());
        self.cx.dispatcher.run_until_parked();
    }

    fn case(&mut self, name: &str) -> Vec<String> {
        println!("artifact-row {name}: opens");
        let window = self.open();
        self.until(|room, _| !room.pages().is_empty());
        let page = self.pages().remove(0);
        assert!(
            !page.read_with(&self.cx, |page, _| page.unavailable()),
            "the web view did not start"
        );
        let seen = self.until(|room, seen| settled(&room.state(&page), seen));
        let told = self.state(&page).height.expect("a told height");
        let drawn = seen.page.expect("the page in the tree");
        let mut faults = Vec::new();
        if (drawn.height() - f64::from(told)).abs() > 0.5 {
            faults.push(format!(
                "the page is drawn {} tall in its row; it tells {told}",
                drawn.height()
            ));
        }
        match (seen.row(2), seen.row(3), seen.under) {
            (Some(row), Some(next), Some(text)) => {
                let under_page = row.bottom - drawn.bottom;
                let under_text = next.bottom - text.bottom;
                println!(
                    "artifact-row {name}: told {told}; the page {drawn:?} in its row {row:?}, {under_page} above the row's end; the next row {next:?}, its words {text:?}, {under_text} above its end"
                );
                if (under_page - under_text - FRAME_LINE).abs() > 0.5 {
                    faults.push(format!(
                        "the page's row ends {under_page} under the page, where a row of words ends {under_text} under them and the page's frame is {FRAME_LINE}"
                    ));
                }
            }
            found => faults.push(format!(
                "the page's row, the next row and its words are not all drawn: {found:?}"
            )),
        }
        let top = drawn.top.max(0.);
        let bottom = drawn.bottom.min(seen.height);
        let at = point(
            px(((drawn.left + drawn.right) / 2.) as f32),
            px(((top + bottom) / 2.) as f32),
        );
        println!("artifact-row {name}: a press at {at:?}");
        let heard = self.heard.borrow().len();
        self.visual
            .as_mut()
            .expect("a room drawn")
            .simulate_click(at, Modifiers::none());
        let pressed = self.look();
        match pressed.page_focused {
            false => faults.push(format!(
                "the press never reached the page; the view heard {:?}",
                &self.heard.borrow()[heard..]
            )),
            true => {
                self.until(|room, _| {
                    let state = room.state(&page);
                    let elsewhere =
                        state.title.starts_with("down:") && !state.title.starts_with("down:link");
                    let moved = !state.url.is_empty() && state.url != PAGE;
                    !room.opened.borrow().is_empty() || elsewhere || moved
                });
                let opened = self.opened.borrow().clone();
                if opened != [ROOM] {
                    let state = self.state(&page);
                    faults.push(format!(
                        "a press on the link handed the app {opened:?}; the page is at {} titled {}",
                        state.url, state.title
                    ));
                }
            }
        }
        self.close(window);
        faults
            .into_iter()
            .map(|fault| format!("{name}: {fault}"))
            .collect()
    }
}

fn settled(state: &PageState, seen: &Seen) -> bool {
    let (Some(told), Some(page)) = (state.height, seen.page) else {
        return false;
    };
    state.title == format!("size:{}:{told}", page.height().round() as i64)
}

fn inner() -> i32 {
    ducktape_web::serve(Network {
        read: Box::new(|address, answer| answer(served(&address))),
        serves,
    });
    let cx = TestAppContext::single();
    cx.executor().allow_parking();
    cx.update(gpui_kit::init);
    let mut room = Room {
        cx,
        visual: None,
        view: None,
        opened: Rc::default(),
        heard: Rc::default(),
        told: String::new(),
    };
    let mut faults = Vec::new();
    for (at, (name, _)) in CASES.iter().enumerate() {
        SHOWN.store(at, Ordering::SeqCst);
        faults.extend(room.case(name));
    }
    for fault in &faults {
        println!("artifact-row: FAULT: {fault}");
    }
    if faults.is_empty() {
        println!("artifact-row: as it should");
    }
    ducktape_web::shutdown();
    i32::from(!faults.is_empty())
}
