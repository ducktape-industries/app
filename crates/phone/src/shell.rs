//! The phone's one window: the host started once per process ([`launch`]),
//! the window opened over it ([`open_window`]: once on iOS, once per
//! Activity on Android), what the host hands on acted on (Back, a changed
//! inset, memory pressure, the consent queue, a link), and in the gate
//! build the AX door.
//!
//! The window draws the join ([`join`]) until the phone is on a network
//! with an account, then the [`stack`]: a tab root (Home, Open) under the
//! tab bar, or the program on top, its view seated full screen under the
//! capsule; a program's page (its capabilities, from the capsule's About)
//! over either; a sheet over all (the account menu, the capsule's menu,
//! the consent sheet). Each program opened keeps one seat, placed with the
//! safe body and in front only while it is on top and the window is: a
//! program left stays warm where it was (wasm-mobile SPEC §7.1).
//!
//! It moves as the Expo build's moves (`ducktape_host::motion`): a program
//! (or its page) pushed slides in from the right over the screen it left,
//! which moves left under a dim, and a pop is the reverse; iOS's left-edge
//! swipe drags the screen on top off under the finger; a sheet slides up
//! over a fading scrim and down again; the alert and the toast fade. Tab
//! switches do not move (Expo's tabs do not). Nothing moves while nothing
//! changes: a frame is asked for only while something is under way.

mod about;
mod home;
mod join;
mod look;
mod running;
mod sheet;
pub mod stack;

use ducktape_host::a11y::Control as _;
use ducktape_host::contract::{Boot, OsServices, Signal, SoftKeyboard};
use ducktape_host::motion::{self, Shown, Slide, Tween};
use ducktape_host::runtime::notify::{Opened as Banner, Permission};
use ducktape_host::runtime::{Back, Placed, consent};
use ducktape_host::session::{Opening, Place, Seats, SeatsEvent, Session, SessionEvent, Shared};
use futures::StreamExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, AsyncApp, ClipboardItem, Context, Entity,
    Focusable as _, FontWeight, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity,
    Window, WindowOptions, div, px,
};
use sheet::Sheet;
use stack::{Stack, Tab};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// The door's name for the phone's one window: its ids read
/// `phone:home/<program>`.
pub const WINDOW: &str = "phone";

/// The port the gate build's door listens on: a phone launch takes no
/// environment, so it is compiled in (wasm-mobile SPEC §11.1). 127.0.0.1
/// only; the walk reaches it through `adb forward` or the simulator's
/// shared loopback.
#[cfg(feature = "ax-door")]
pub const DOOR_PORT: u16 = 7777;

/// The system program whose view is the account (`ARCHITECTURE.md` §1: the
/// host names `identity` and `programs` only), opened by name as the
/// desktop's Account menu opens it (SPEC §7.4).
const ACCOUNT_PROGRAM: &str = "programs";

/// What the shell keeps for the process: the host's entities, the stack
/// (an Android Activity made again finds it as it was), the window on
/// screen, and the links the OS handed over before there was one.
struct Phone {
    shared: Shared,
    stack: Entity<Stack>,
    window: Option<(AnyWindowHandle, WeakEntity<Root>)>,
    waiting: Vec<String>,
}

impl Global for Phone {}

/// Starts the host on `os`, once per process: the app's faces and theme,
/// the host's stores and entities over the wasmtime runner, a connect to
/// the node this phone used last, and the loops that hand the shell what
/// the host and the OS send.
pub fn launch(os: Arc<dyn OsServices>, cx: &mut App) {
    // first, as the desktop's `main` has it: `DUCKTAPE_PERF=1` in the
    // launch's environment turns the counters on (iOS:
    // `SIMCTL_CHILD_DUCKTAPE_PERF=1 xcrun simctl launch`; Android has no
    // launch environment, so `MainActivity` makes one of the
    // `-e DUCKTAPE_PERF 1` extra), and the sampler reads the OS facts, so
    // they are installed first (the instance `start` installs again)
    ducktape_host::contract::install(os.clone());
    ducktape_host::perf::start();
    gpui_kit::init(cx);
    let faces = ducktape_host::fonts::BUNDLED_FACES
        .iter()
        .copied()
        .map(std::borrow::Cow::Borrowed)
        .collect();
    if let Err(error) = cx.text_system().add_fonts(faces) {
        tracing::error!(target: "ducktape::phone", %error, "the bundled faces could not be registered");
    }
    ducktape_host::theme::configure_native_theme(cx);
    let runner = crate::os::runner(os.dirs());
    let boot = Boot {
        os,
        runner,
        open: banner_opened,
    };
    let (host, inbound) = ducktape_host::start::start(boot, cx);
    crate::os::install_inbound(inbound);
    if let Some(target) = Session::boot_target() {
        host.shared
            .session
            .update(cx, |session, cx| session.connect(target, cx));
    }
    let (signals, mut asks) = (host.signals, host.consent);
    let stack = cx.new(|_| Stack::default());
    cx.set_global(Phone {
        shared: host.shared,
        stack,
        window: None,
        waiting: Vec::new(),
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(signal) = signals.recv().await {
            cx.update(|cx| handed_on(signal, cx));
        }
    })
    .detach();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while asks.next().await.is_some() {
            cx.update(|cx| match root(cx) {
                Some(root) => root.update(cx, |root, cx| root.consent_moved(cx)),
                // nowhere to ask: nothing is signed
                None => consent::refuse_all(),
            });
        }
    })
    .detach();
    let links = crate::os::links_to_open();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(link) = links.recv().await {
            cx.update(|cx| linked(link, cx));
        }
    })
    .detach();
}

/// What a banner's click names, from the host's notification centre: a
/// link opens as any other; the phone's app store updates the app, so
/// there is no Updates page.
fn banner_opened(opened: Banner) {
    match opened {
        Banner::Link(link) => crate::os::link(link),
        Banner::Updates => {}
    }
}

/// The window's root, while the window is open.
fn root(cx: &App) -> Option<Entity<Root>> {
    cx.try_global::<Phone>()?.window.as_ref()?.1.upgrade()
}

/// A link the OS handed over: to the window, or kept for the one to come.
fn linked(link: String, cx: &mut App) {
    match root(cx) {
        Some(root) => root.update(cx, |root, cx| root.take_link(link, cx)),
        None => cx.global_mut::<Phone>().waiting.push(link),
    }
}

/// Opens the phone's window over the host [`launch`] started.
pub fn open_window(cx: &mut App) {
    let Some((shared, stack)) = cx
        .try_global::<Phone>()
        .map(|phone| (phone.shared.clone(), phone.stack.clone()))
    else {
        tracing::error!(target: "ducktape::phone", "a window was asked for before the host started");
        return;
    };
    let opened = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        cx.new(|cx| Root::new(shared, stack, window, cx))
    });
    match opened {
        Ok((handle, root)) => {
            let phone = cx.global_mut::<Phone>();
            phone.window = Some((handle, root.downgrade()));
            let waiting = std::mem::take(&mut phone.waiting);
            for link in waiting {
                root.update(cx, |root, cx| root.take_link(link, cx));
            }
            root.update(cx, |root, cx| root.consent_moved(cx));
            #[cfg(feature = "ax-door")]
            open_door(cx);
        }
        Err(error) => {
            tracing::error!(target: "ducktape::phone", %error, "the phone's window did not open");
        }
    }
}

/// What the host hands on rather than consumes: Back walks the window (and
/// past its last screen, the OS's own); a changed inset redraws; memory
/// pressure lets go of every program not on the stack (SPEC §7.5).
fn handed_on(signal: Signal, cx: &mut App) {
    match signal {
        Signal::MemoryWarning => {
            let phone = cx.global::<Phone>();
            let (stack, seats) = (phone.stack.clone(), phone.shared.seats.clone());
            stack.update(cx, |stack, cx| {
                stack.trim();
                cx.notify();
            });
            let_go_closed(&stack, &seats, cx);
        }
        Signal::Back => {
            if let Some(root) = root(cx) {
                root.update(cx, |root, cx| root.back(cx));
            }
        }
        Signal::BackDrag(x) => {
            if let Some(root) = root(cx) {
                root.update(cx, |root, cx| root.drag(x, cx));
            }
        }
        Signal::BackDragCancelled => {
            if let Some(root) = root(cx) {
                root.update(cx, |root, cx| root.drag_cancelled(cx));
            }
        }
        Signal::InsetsChanged => {
            if let Some(root) = root(cx) {
                root.update(cx, |_, cx| cx.notify());
            }
        }
        // the host consumes these
        Signal::Lifecycle(_) | Signal::BannerClick(_) => {}
    }
}

/// Every seat of a program no longer open let go now, drawn or not: an
/// Android trim in the background comes with no surface, so no draw would
/// (SPEC §7.5). The seats that stay are kept where they are; the next draw
/// places them again.
fn let_go_closed(stack: &Entity<Stack>, seats: &Entity<Seats>, cx: &mut App) {
    let kept = stack
        .read(cx)
        .open()
        .iter()
        .map(|opened| (opened.instance, (opened.program, None, Placed::default())))
        .collect();
    seats.update(cx, |seats, cx| seats.place(kept, cx));
}

/// The AX door over the phone's window, on [`DOOR_PORT`]; the door file in
/// `Dirs::runtime` for the walk to read its token from.
#[cfg(feature = "ax-door")]
fn open_door(cx: &mut App) {
    use ducktape_host::ax;
    static OPENED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if OPENED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let Some((calls, written)) = ax::open_at(DOOR_PORT) else {
        return;
    };
    let mut written = Some(written);
    cx.on_app_quit(move |_| {
        drop(written.take());
        async {}
    })
    .detach();
    let seats = cx.global::<Phone>().shared.seats.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let windows = |cx: &App| {
            cx.try_global::<Phone>()
                .and_then(|phone| phone.window.as_ref())
                .map(|(handle, _)| {
                    vec![(
                        WINDOW.to_owned(),
                        ducktape_host::runtime::WindowKey(0),
                        *handle,
                    )]
                })
                .unwrap_or_default()
        };
        // whether every seat's tick has landed: a read parks until then
        let settle = move |cx: &mut App| seats.update(cx, |seats, cx| seats.settle(cx));
        let keep = |_: &Window, _: &App| None;
        let chords = Vec::new;
        let door = ax::Door {
            windows: &windows,
            settle: &settle,
            keep: &keep,
            chords: &chords,
        };
        ax::serve(calls, door, cx).await;
    })
    .detach();
}

/// What the stack shows full screen: a tab root, the program on top, or a
/// program's page over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Root(Tab),
    Program(&'static str),
    About(&'static str),
}

impl Page {
    /// Its layer's element id: a `::` name, which the AX door's paths leave
    /// out (`ax::tree::element_path`), so the ids inside read as before.
    fn layer(self) -> SharedString {
        match self {
            Page::Root(Tab::Home) => "screen::home".into(),
            Page::Root(Tab::Open) => "screen::open".into(),
            Page::Program(program) => format!("screen::program/{program}").into(),
            Page::About(program) => format!("screen::about/{program}").into(),
        }
    }

    /// The program seated full screen on it, if any.
    fn program(self) -> Option<&'static str> {
        match self {
            Page::Program(program) => Some(program),
            Page::Root(_) | Page::About(_) => None,
        }
    }
}

/// The stack moving from the screen it `left`: pushed over it, or popped
/// off it (`pop`), as `slide` moves the two.
#[derive(Clone, Copy, Debug)]
struct Move {
    left: Page,
    pop: bool,
    slide: Slide,
    tween: Tween,
}

/// The window's root: the join, or the stack over the host's entities, the
/// seats of the programs open, and the sheet over them.
pub struct Root {
    shared: Shared,
    join: Entity<join::Join>,
    stack: Entity<Stack>,
    sheet: Option<Sheet>,
    /// The consent queue's front, shown over any sheet until it is
    /// answered, and when it came up (Approve keeps no press in its first
    /// moment, `consent::SETTLE`).
    consent: Option<(u64, std::time::Instant)>,
    /// An alert over everything but the consent sheet (Expo `Alert`): its
    /// title and its sentence, dismissed by OK or Back.
    alert: Option<(&'static str, String)>,
    /// The program whose page is over the stack.
    about: Option<&'static str>,
    /// The title each seat's view gave its window, by its instance.
    titles: BTreeMap<u64, String>,
    /// What `Seats` was last told: each open program's place, and the seat
    /// whose view may move the keys.
    placed: (BTreeMap<u64, Place>, Option<u64>),
    /// Links that came before the network's program list did, in order.
    waiting: Vec<String>,
    /// Home's filter.
    search: Entity<InputState>,
    /// Each placed seat, observed: a tree, a standin or a fault that moved
    /// draws the window again.
    watched: BTreeMap<u64, Subscription>,
    /// The stack moving, until its motion ends.
    moving: Option<Move>,
    /// How far in (points) iOS's left-edge swipe has come while it drags
    /// the screen on top off.
    dragged: Option<f32>,
    /// The window's width at the last draw: a drag's share of it.
    width: f32,
    /// The sheet, the consent sheet, the alert and the toast as drawn, each
    /// coming and going; and each sheet's panel height, as last laid out.
    drawn_sheet: Option<Shown<Sheet>>,
    drawn_consent: Option<Shown<(u64, Instant, consent::Words)>>,
    drawn_alert: Option<Shown<(&'static str, String)>>,
    drawn_toast: Option<Shown<String>>,
    sheet_height: Rc<Cell<Option<gpui_kit::Pixels>>>,
    consent_height: Rc<Cell<Option<gpui_kit::Pixels>>>,
    _subscriptions: Vec<Subscription>,
}

impl Root {
    fn new(
        shared: Shared,
        stack: Entity<Stack>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by name"));
        let focus = search.read(cx).focus_handle(cx);
        let mut subscriptions = vec![
            cx.observe(&shared.rail, |root, _, cx| root.rail_moved(cx)),
            cx.observe(&shared.session, |_, _, cx| cx.notify()),
            cx.observe(&shared.screen, |_, _, cx| cx.notify()),
            cx.observe(&shared.account, |_, _, cx| cx.notify()),
            cx.observe(&shared.toast, |_, _, cx| cx.notify()),
            cx.observe(&shared.prefs, |_, _, cx| cx.notify()),
            cx.observe(&stack, |_, _, cx| cx.notify()),
            cx.observe(&shared.seats, |root, _, cx| root.watch_seats(cx)),
            cx.subscribe(&shared.seats, |root, _, event: &SeatsEvent, cx| {
                root.seats_said(event, cx)
            }),
            cx.subscribe(&shared.session, |root, _, event: &SessionEvent, cx| {
                if let SessionEvent::LeftNetwork = event {
                    root.left_network(cx);
                }
            }),
            cx.subscribe_in(&search, window, |_, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            // the keyboard for the filter: gm is told, gpui_ios raises it
            // with the focused field itself (`OsServices::soft_keyboard`)
            cx.on_focus(&focus, window, |_, window, cx| {
                let up = SoftKeyboard::Show(Default::default());
                ducktape_host::contract::os().soft_keyboard(window, up);
                cx.notify();
            }),
            cx.on_blur(&focus, window, |_, window, cx| {
                ducktape_host::contract::os().soft_keyboard(window, SoftKeyboard::Hide);
                cx.notify();
            }),
            // in front only while the window is
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
            cx.observe_window_appearance(window, |_, window, cx| {
                gpui_kit::component::Theme::sync_system_appearance(Some(window), cx);
                cx.notify();
            }),
        ];
        subscriptions.extend(crate::os::follow(window, cx));
        gpui_kit::component::Theme::sync_system_appearance(Some(window), cx);
        let join = cx.new(|cx| join::Join::new(shared.clone(), window, cx));
        Self {
            shared,
            join,
            stack,
            sheet: None,
            consent: None,
            alert: None,
            about: None,
            titles: BTreeMap::new(),
            placed: Default::default(),
            waiting: Vec::new(),
            search,
            watched: BTreeMap::new(),
            moving: None,
            dragged: None,
            width: 0.,
            drawn_sheet: None,
            drawn_consent: None,
            drawn_alert: None,
            drawn_toast: None,
            sheet_height: Rc::default(),
            consent_height: Rc::default(),
            _subscriptions: subscriptions,
        }
    }

    fn now(cx: &App) -> std::time::Instant {
        cx.background_executor().now()
    }

    fn pick(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let now = Self::now(cx);
        self.stack.update(cx, |stack, cx| {
            stack.pick(tab, now);
            cx.notify();
        });
    }

    /// `program` on top, in its seat, pushed over the screen it was on; the
    /// seat's instance.
    fn open(&mut self, program: &'static str, cx: &mut Context<Self>) -> u64 {
        self.sheet = None;
        let from = self.page(cx);
        self.about = None;
        let instance = self.stack.update(cx, |stack, cx| {
            let instance = stack.push(program);
            cx.notify();
            instance
        });
        self.moved(from, false, cx);
        instance
    }

    /// `program`'s page pushed over the stack.
    fn show_about(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.sheet = None;
        let from = self.page(cx);
        self.about = Some(program);
        self.moved(from, false, cx);
        cx.notify();
    }

    /// The program's page popped.
    fn close_about(&mut self, cx: &mut Context<Self>) {
        let from = self.page(cx);
        self.about = None;
        self.moved(from, true, cx);
        cx.notify();
    }

    /// What the stack shows now: a program's page (while the network lists
    /// it), else the program on top, else the tab's root.
    fn page(&self, cx: &App) -> Page {
        if let Some(program) = self
            .about
            .filter(|program| home::row_of(&self.shared, program, cx).is_some())
        {
            return Page::About(program);
        }
        let stack = self.stack.read(cx);
        stack.top().map_or(Page::Root(stack.tab()), Page::Program)
    }

    /// The screen a pop would show: the one under the page or the program
    /// on top; `None` on a root.
    fn under(&self, cx: &App) -> Option<Page> {
        let stack = self.stack.read(cx);
        let root = Page::Root(stack.tab());
        if let Page::About(_) = self.page(cx) {
            return Some(stack.top().map_or(root, Page::Program));
        }
        stack.top()?;
        Some(stack.under().map_or(root, Page::Program))
    }

    /// The stack went from `from` to the screen it shows now, by a push, or
    /// a pop when `pop`: the new screen slides in over the old, or the old
    /// one off the new. A move under way ends where it was going; a pop
    /// the finger let go of goes on from where it let go. Tab switches do
    /// not move.
    fn moved(&mut self, from: Page, pop: bool, cx: &mut Context<Self>) {
        let dragged = self.dragged.take();
        let to = self.page(cx);
        if from == to || matches!((from, to), (Page::Root(_), Page::Root(_))) {
            return;
        }
        let (motion, slide) = motion::stack(pop, cx);
        let start = match dragged {
            Some(x) if pop && self.width > 0. => (x / self.width).clamp(0., 1.),
            _ => 0.,
        };
        self.moving = Some(Move {
            left: from,
            pop,
            slide,
            tween: Tween::between(start, 1., motion, cx),
        });
        cx.notify();
    }

    /// iOS's left-edge swipe, `x` points in: the screen on top follows the
    /// finger while a Back would pop it (a program's page, or a program on
    /// top whose view does not hear Back itself) and nothing is over it.
    /// Under Reduce Motion nothing follows; the release is Back as before.
    fn drag(&mut self, x: f32, cx: &mut Context<Self>) {
        if self.dragged.is_none() && !self.can_drag(cx) {
            return;
        }
        self.moving = None;
        self.dragged = Some(x.max(0.));
        cx.notify();
    }

    fn can_drag(&self, cx: &App) -> bool {
        let covered = self.consent.is_some() || self.alert.is_some() || self.sheet.is_some();
        let joining = join::step(*self.shared.screen.read(cx).get()).is_some();
        if covered || joining || motion::reduced(cx) || self.under(cx).is_none() {
            return false;
        }
        matches!(self.page(cx), Page::About(_))
            || self
                .top_seat(cx)
                .is_none_or(|seat| !seat.read(cx).hears_back())
    }

    /// The swipe let go short of Back: the screen on top slides back into
    /// place over the one under it.
    fn drag_cancelled(&mut self, cx: &mut Context<Self>) {
        let Some(x) = self.dragged.take() else {
            return;
        };
        cx.notify();
        let Some(under) = self.under(cx) else {
            return;
        };
        let (motion, slide) = motion::stack(false, cx);
        let start = match self.width > 0. {
            true => 1. - (x / self.width).clamp(0., 1.),
            false => 1.,
        };
        self.moving = Some(Move {
            left: under,
            pop: false,
            slide,
            tween: Tween::between(start, 1., motion, cx),
        });
    }

    /// `program`'s own link (`duck://<chain>/<program>`) on the clipboard.
    fn copy_link(&mut self, program: &'static str, cx: &mut Context<Self>) {
        let chain = self.shared.session.read(cx).get().chain.clone();
        if let Some(link) = ducklink::mint(&chain, program, &[]) {
            cx.write_to_clipboard(ClipboardItem::new_string(link));
            self.notice("Copied".into(), cx);
        }
        self.sheet = None;
        cx.notify();
    }

    /// The capsule's ×: Home, the program left where it is, popped off.
    fn go_home(&mut self, cx: &mut Context<Self>) {
        let now = Self::now(cx);
        let from = self.page(cx);
        self.stack.update(cx, |stack, cx| {
            stack.home(now);
            cx.notify();
        });
        self.moved(from, true, cx);
    }

    /// Open's ×: the program's seat goes; it stays on Home.
    fn close_program(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.stack.update(cx, |stack, cx| {
            stack.close(program);
            cx.notify();
        });
    }

    /// `sheet` over the screen (under the consent sheet, when one is up).
    fn show(&mut self, sheet: Sheet, cx: &mut Context<Self>) {
        self.sheet = Some(sheet);
        cx.notify();
    }

    fn account_sheet(&mut self, cx: &mut Context<Self>) {
        self.show(Sheet::Account, cx);
    }

    fn capsule_menu(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.show(Sheet::Capsule(program), cx);
    }

    /// The sheet goes (the scrim, Back).
    fn close_sheet(&mut self, cx: &mut Context<Self>) {
        if self.sheet.take().is_some() {
            cx.notify();
        }
    }

    /// The consent sheet goes (its scrim, Back): its ask is refused, as
    /// Cancel refuses it, and the queue's next shows.
    fn refuse_consent(&mut self, cx: &mut Context<Self>) {
        if let Some((ask, _)) = self.consent.take() {
            consent::answer(ask, false);
            self.consent_moved(cx);
        }
    }

    /// The consent queue moved: its front shows in the consent sheet, over
    /// any other sheet; none, and the consent sheet goes.
    fn consent_moved(&mut self, cx: &mut Context<Self>) {
        self.consent = match (consent::front(), self.consent) {
            (Some((ask, _)), Some((shown, opened))) if shown == ask => Some((shown, opened)),
            (Some((ask, _)), _) => Some((ask, Self::now(cx))),
            (None, _) => None,
        };
        cx.notify();
    }

    /// An alert up over the window (Expo `Alert.alert` with one OK).
    fn alert(&mut self, title: &'static str, said: String, cx: &mut Context<Self>) {
        self.alert = Some((title, said));
        cx.notify();
    }

    /// Back (Android's button, iOS's edge): on the join, the code goes back
    /// to the address and the address is the OS's own; else the consent
    /// sheet's ask is refused, an alert dismissed, a sheet closed, a
    /// program's page closed, each first; else the view on top goes back
    /// inside itself when its last frame's root asks to hear it; else the
    /// stack pops; at Home, the OS's own (SPEC §7.6). A drag under way
    /// that this Back does not pop springs back.
    fn back(&mut self, cx: &mut Context<Self>) {
        self.walk_back(cx);
        if self.dragged.take().is_some() {
            cx.notify();
        }
    }

    fn walk_back(&mut self, cx: &mut Context<Self>) {
        match join::step(*self.shared.screen.read(cx).get()) {
            Some(join::Step::Code) => return self.join.update(cx, |join, cx| join.leave(cx)),
            Some(join::Step::Node) => return crate::os::back_at_root(),
            None => {}
        }
        if self.consent.is_some() {
            return self.refuse_consent(cx);
        }
        if self.alert.take().is_some() {
            return cx.notify();
        }
        if self.sheet.is_some() {
            return self.close_sheet(cx);
        }
        if self.about.is_some() {
            return self.close_about(cx);
        }
        if let Some(seat) = self.top_seat(cx)
            && seat.update(cx, |seat, cx| seat.back(cx)) == Back::Heard
        {
            return;
        }
        let now = Self::now(cx);
        let from = self.page(cx);
        let popped = self.stack.update(cx, |stack, cx| {
            let popped = stack.back(now);
            cx.notify();
            popped
        });
        if !popped {
            crate::os::back_at_root();
        }
        self.moved(from, true, cx);
    }

    /// The seat of the program on top.
    fn top_seat(&self, cx: &App) -> Option<Entity<ducktape_host::runtime::Seat>> {
        let stack = self.stack.read(cx);
        let instance = stack.instance(stack.top()?)?;
        self.shared.seats.read(cx).seat(instance)
    }

    /// The network in hand was left: nothing stays open (each seat let go
    /// now, drawn or not), and the links kept for it are dropped.
    fn left_network(&mut self, cx: &mut Context<Self>) {
        self.stack.update(cx, |stack, cx| {
            stack.clear();
            cx.notify();
        });
        let_go_closed(&self.stack, &self.shared.seats, cx);
        self.moving = None;
        self.dragged = None;
        self.sheet = None;
        self.alert = None;
        self.titles.clear();
        self.about = None;
        self.waiting.clear();
        cx.notify();
    }

    /// The network listed its programs: Home redraws, and the links kept
    /// until then open now, in the order they came.
    fn rail_moved(&mut self, cx: &mut Context<Self>) {
        if !self.shared.rail.read(cx).rows().is_empty() {
            for link in std::mem::take(&mut self.waiting) {
                self.open_link(&link, cx);
            }
        }
        cx.notify();
    }

    /// A link from the OS (`duck://`, or the Expo app's `ducktape://`):
    /// opened once the network lists its programs (`Rail::opening` reads
    /// the list, not any program's code).
    fn take_link(&mut self, link: String, cx: &mut Context<Self>) {
        let link = match link.strip_prefix("ducktape://") {
            Some(rest) => format!("duck://{rest}"),
            None => link,
        };
        if self.shared.rail.read(cx).rows().is_empty() {
            self.waiting.push(link);
        } else {
            self.open_link(&link, cx);
        }
    }

    /// A link, read as every shell reads it (`Rail::opening`): this chain's
    /// program comes on top and its view is handed the route; another
    /// chain's is said and not opened, as the Expo app says it.
    fn open_link(&mut self, link: &str, cx: &mut Context<Self>) {
        let session = self.shared.session.read(cx).get();
        let (chain, network) = (session.chain.clone(), session.network.clone());
        match self.shared.rail.read(cx).opening(link, &chain) {
            Opening::Seat {
                program,
                route,
                notice,
            } => {
                let instance = self.open(program, cx);
                if let Some(route) = route {
                    ducktape_host::runtime::route_to(program, instance, route);
                }
                if let Some(said) = notice {
                    self.notice(said, cx);
                }
            }
            // Expo's `Alert` (mobile `src/kit/navigate.ts`), its words
            Opening::Elsewhere { chain, .. } => {
                let here = if network.is_empty() {
                    "another network".to_owned()
                } else {
                    network
                };
                self.alert(
                    "Another network",
                    format!("This link is for {chain}, you're on {here}."),
                    cx,
                );
            }
            Opening::Browser(url) => cx.open_url(&url),
            Opening::Nothing(said) => self.notice(said, cx),
        }
    }

    /// A one-line notice; the same one again while it is up says nothing
    /// new, so it is not restarted either.
    fn notice(&self, said: String, cx: &mut App) {
        if self.shared.toast.read(cx).get() == &said {
            return;
        }
        self.shared
            .toast
            .update(cx, |toast, cx| toast.show(said, cx));
    }

    /// What a seat asked of the window: its title, or a link to open.
    fn seats_said(&mut self, event: &SeatsEvent, cx: &mut Context<Self>) {
        match event {
            SeatsEvent::Title(instance, title) => {
                self.titles.insert(*instance, title.clone());
                cx.notify();
            }
            SeatsEvent::OpenLink(_, _, link) => self.open_link(link, cx),
            // the reader's account: the account menu, as from a header's avatar
            SeatsEvent::Show(_, _, view_wire::methods::Place::Account) => self.account_sheet(cx),
            // a refused camera's or microphone's system settings, where this
            // host has that page (`render/av.rs`: none without `av`); the
            // kernel shows no other
            SeatsEvent::Show(_, _, place) => {
                if let Some(page) = ducktape_host::render::av::settings_page(*place) {
                    cx.open_url(page);
                }
            }
            // the desktop widens a window to a view that came; a phone
            // never does
            SeatsEvent::Seated(_) => {}
            // a phone floats no pane (`OsFacts::floats`): its views' kernel
            // knows no float method, so neither comes
            SeatsEvent::Float(..) | SeatsEvent::Unfloat(_) => {}
        }
    }

    /// A seat came or went: each placed seat is observed, none that left.
    fn watch_seats(&mut self, cx: &mut Context<Self>) {
        let seats = self.shared.seats.clone();
        let placed: Vec<u64> = self.placed.0.keys().copied().collect();
        self.watched.retain(|instance, _| placed.contains(instance));
        for instance in placed {
            if self.watched.contains_key(&instance) {
                continue;
            }
            if let Some(seat) = seats.read(cx).seat(instance) {
                let watching = cx.observe(&seat, |_, _, cx| cx.notify());
                self.watched.insert(instance, watching);
            }
        }
        cx.notify();
    }

    /// Every open program's seat placed in this window with the safe body,
    /// in front only on top of the stack in an active window, the keys its
    /// view's while nothing (a sheet, the consent sheet, an alert) is over
    /// it: told to `Seats` after this draw when it moved, and each placed
    /// seat observed (a Root made again re-places the seats there were, so
    /// `Seats` does not notify).
    fn place(&mut self, window: &Window, body: (f32, f32), cx: &mut Context<Self>) {
        let handle = window.window_handle();
        let active = window.is_window_active();
        // while the stack moves, the program it left (or the one under the
        // finger) is drawn too, so it stays in front until the move ends
        let left = match (self.moving, self.dragged) {
            (Some(moving), _) => Some(moving.left),
            (None, Some(_)) => self.under(cx),
            (None, None) => None,
        }
        .and_then(Page::program);
        let stack = self.stack.read(cx);
        let left = left.and_then(|program| stack.instance(program));
        // a program's page covers the program on top
        let top = stack
            .top()
            .and_then(|program| stack.instance(program))
            .filter(|_| self.about.is_none());
        let wanted: BTreeMap<u64, Place> = stack
            .open()
            .iter()
            .map(|opened| {
                let shown = top == Some(opened.instance) || left == Some(opened.instance);
                let in_front = active && shown;
                let place = (handle, Some(body), in_front);
                // a phone floats no pane (`OsFacts::floats`)
                (
                    opened.instance,
                    (opened.program, Some(place), Placed::default()),
                )
            })
            .collect();
        let covered = self.sheet.is_some() || self.consent.is_some() || self.alert.is_some();
        let keys = top.filter(|_| !covered);
        if (&wanted, keys) == (&self.placed.0, self.placed.1) {
            return;
        }
        self.placed = (wanted.clone(), keys);
        self.titles
            .retain(|instance, _| wanted.contains_key(instance));
        let seats = self.shared.seats.clone();
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            seats.update(cx, |seats, cx| seats.place(wanted.clone(), cx));
            for instance in wanted.keys() {
                if let Some(seat) = seats.read(cx).seat(*instance) {
                    let free = keys == Some(*instance);
                    seat.update(cx, |seat, cx| seat.set_keys_free(free, cx));
                }
            }
            this.update(cx, |root, cx| root.watch_seats(cx))
                .unwrap_or_default();
        });
    }

    /// The sheet up, drawn where `at` says: the frame over the window with
    /// its content.
    fn sheet(
        &mut self,
        sheet: Sheet,
        at: sheet::At,
        palette: look::Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (name, content): (&'static str, AnyElement) = match sheet {
            Sheet::Account => ("Account", self.account_menu(palette, cx)),
            Sheet::Capsule(program) => ("More", self.capsule_sheet(program, palette, cx)?),
            Sheet::Notifications(program) => (
                "Notifications",
                self.notifications_sheet(program, palette, cx),
            ),
        };
        let close = Self::close_sheet;
        Some(sheet::frame("sheet", name, content, at, close, palette, cx))
    }

    /// The consent sheet for `ask`, opened at `opened`, its `words`: the
    /// host's confirm sheet content in the phone's sheet, where `at` says.
    fn consent_sheet(
        &mut self,
        (ask, opened, words): (u64, Instant, consent::Words),
        at: sheet::At,
        palette: look::Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = words.title;
        let this = cx.weak_entity();
        let answer = move |yes: bool, cx: &mut App| {
            consent::answer(ask, yes);
            // the window may have closed: the next one shows the queue's
            // front
            this.update(cx, |root, cx| root.consent_moved(cx))
                .unwrap_or_default();
        };
        let approve = answer.clone();
        // Approve takes no press in the sheet's first moment: a second
        // press the view asked for lands before anyone read it
        let approved = move |cx: &mut App| {
            if Self::now(cx).saturating_duration_since(opened) >= consent::SETTLE {
                approve(true, cx);
            }
        };
        let body = consent::sheet(words, palette.dark(), approved, answer);
        let refuse = Self::refuse_consent;
        sheet::frame("consent", title, body, at, refuse, palette, cx)
    }

    /// The alert up (the shell boards' dialog, Expo `Alert` with one OK): a
    /// centred card over the scrim, its title, its sentence, OK at the
    /// right, `shown` as far as it has faded in. Going, it takes no press.
    fn alert_card(
        &self,
        (title, said): (&'static str, String),
        (shown, leaving): (f32, bool),
        palette: look::Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ok = div()
            .id("alert/ok")
            .control(gpui_kit::Role::Button, "OK")
            .h(px(look::HIT))
            .px(px(look::gap::LG))
            .flex()
            .items_center()
            .active(|style| style.opacity(look::PRESSED))
            .child(look::text(look::BODY, FontWeight::MEDIUM, palette.foreground()).child("OK"))
            .on_click(cx.listener(|root, _, _, cx| {
                root.alert = None;
                cx.notify();
            }));
        let card = div()
            .id("alert")
            .control(gpui_kit::Role::AlertDialog, title)
            .occlude()
            .mx(px(24.))
            .flex()
            .flex_col()
            .pt(px(20.))
            .px(px(20.))
            .pb(px(look::gap::LG))
            .bg(palette.background())
            .border_1()
            .border_color(palette.border_strong())
            .child(
                look::text(look::TITLE, FontWeight::MEDIUM, palette.foreground())
                    .mb(px(look::gap::SM))
                    .child(title),
            )
            .child(look::text(look::SMALL, FontWeight::NORMAL, palette.muted()).child(said))
            .child(
                div()
                    .mt(px(look::gap::BLOCK))
                    .flex()
                    .justify_end()
                    .gap(px(look::gap::XXS))
                    .child(ok),
            );
        let scrim = div()
            .id("alert/scrim")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .opacity(shown)
            .bg(palette.scrim())
            .flex()
            .flex_col()
            .justify_center();
        if leaving {
            let cover = div().absolute().inset_0().occlude();
            return scrim.child(card.child(cover)).into_any_element();
        }
        scrim
            .occlude()
            .child(ducktape_host::a11y::modal(card))
            .into_any_element()
    }

    /// The account menu (the shell boards' 40): who is reading, and the
    /// account's own program (the `programs` view), opened by name.
    fn account_menu(&mut self, palette: look::Palette, cx: &mut Context<Self>) -> AnyElement {
        let network = self.shared.session.read(cx).get().network.clone();
        // Expo `you.tsx`: the account's name, else "No account yet"; the
        // avatar draws the name's initial, or none
        let (avatar, name, caption) = match home::me(&self.shared, cx) {
            Some((number, name)) => (name.clone(), name, format!("account {number} · {network}")),
            None => (String::new(), "No account yet".to_owned(), network),
        };
        let head = sheet::head(palette).child(
            div()
                .flex()
                .items_center()
                .gap(px(look::gap::LG))
                .child(home::avatar(&avatar, 40., palette))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            look::text(look::BODY, FontWeight::MEDIUM, palette.foreground())
                                .truncate()
                                .child(name),
                        )
                        .child(
                            look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                                .font_family(ducktape_host::fonts::FAMILY_MONO)
                                .truncate()
                                .child(caption),
                        ),
                ),
        );
        let mut menu = div().flex().flex_col().child(head);
        if home::row_of(&self.shared, ACCOUNT_PROGRAM, cx).is_some() {
            menu = menu.child(
                sheet::row(
                    "sheet/account",
                    Some("icons/circle-user.svg"),
                    "Account",
                    Some("keys · agents · invites".into()),
                    palette,
                )
                .on_click(cx.listener(|root, _, _, cx| {
                    root.open(ACCOUNT_PROGRAM, cx);
                })),
            );
        }
        menu.into_any_element()
    }

    /// The capsule's menu (the shell boards' 30): the program, then its
    /// notices' word and its link. `None` once the network stopped listing
    /// it.
    fn capsule_sheet(
        &mut self,
        program: &'static str,
        palette: look::Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let row = home::row_of(&self.shared, program, cx)?;
        let session = self.shared.session.read(cx).get();
        let (chain, network) = (session.chain.clone(), session.network.clone());
        let head = sheet::head(palette).child(
            div()
                .flex()
                .items_center()
                .gap(px(look::gap::LG))
                .child(home::glyph(&row, 40., look::ICON_SHEET, palette))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            look::text(look::BODY, FontWeight::MEDIUM, palette.foreground())
                                .truncate()
                                .child(home::name_of(&row)),
                        )
                        .child(
                            look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                                .truncate()
                                .child(format!("program · {network}")),
                        ),
                ),
        );
        let word = self
            .shared
            .prefs
            .read(cx)
            .get()
            .notify
            .views
            .get(program)
            .map(|permission| SharedString::from(permission.word()));
        let about = sheet::row(
            "sheet/about",
            Some("icons/info.svg"),
            "About",
            Some("capabilities".into()),
            palette,
        )
        .on_click(cx.listener(move |root, _, _, cx| root.show_about(program, cx)));
        let mut menu = div().flex().flex_col().child(head).child(about).child(
            sheet::row(
                "sheet/notifications",
                Some("icons/bell.svg"),
                "Notifications",
                word,
                palette,
            )
            .on_click(cx.listener(move |root, _, _, cx| {
                root.sheet = Some(Sheet::Notifications(program));
                cx.notify();
            })),
        );
        if ducklink::mint(&chain, program, &[]).is_some() {
            menu = menu.child(
                sheet::row(
                    "sheet/copy-link",
                    Some("icons/link.svg"),
                    "Copy link",
                    None,
                    palette,
                )
                .on_click(cx.listener(move |root, _, _, cx| root.copy_link(program, cx))),
            );
        }
        Some(menu.into_any_element())
    }

    /// The three words for a program's notices, the one in force checked
    /// (the desktop's Allow, Silent, Block).
    fn notifications_sheet(
        &mut self,
        program: &'static str,
        palette: look::Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = home::row_of(&self.shared, program, cx)
            .map_or_else(|| program.to_owned(), |row| home::name_of(&row));
        let now = self
            .shared
            .prefs
            .read(cx)
            .get()
            .notify
            .views
            .get(program)
            .copied();
        let head = sheet::head(palette).child(
            look::text(look::BODY, FontWeight::MEDIUM, palette.foreground())
                .child(format!("Notifications from {name}")),
        );
        let choices = Permission::ALL.into_iter().map(|permission| {
            let icon = (now == Some(permission)).then_some("icons/check.svg");
            let id = format!("sheet/notifications/{}", permission.word().to_lowercase());
            sheet::row(id, icon, permission.word(), None, palette).on_click(cx.listener(
                move |root, _, _, cx| {
                    let shared = &root.shared;
                    ducktape_host::session::permission(
                        &shared.notifications,
                        &shared.prefs,
                        program,
                        permission,
                        cx,
                    );
                    root.sheet = None;
                    cx.notify();
                },
            ))
        });
        div()
            .flex()
            .flex_col()
            .child(head)
            .children(choices)
            .into_any_element()
    }

    /// The notice `said` (Expo `Toasts`): the foreground's block over the
    /// tab bar's line, the words in the background's colour, announced,
    /// `shown` as far as it has faded in.
    fn toast(
        said: String,
        bottom: gpui_kit::Pixels,
        shown: f32,
        palette: look::Palette,
    ) -> AnyElement {
        let words = ducktape_host::a11y::live(
            div().id("toast").role(gpui_kit::Role::Status),
            gpui_kit::accesskit::Live::Polite,
            said.clone(),
        )
        .bg(palette.foreground())
        .px(px(look::gap::BLOCK))
        .py(px(look::gap::SM))
        .child(look::text(look::SMALL, FontWeight::NORMAL, palette.background()).child(said));
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(bottom)
            .opacity(shown)
            .flex()
            .justify_center()
            .child(words)
            .into_any_element()
    }

    /// `page` as one layer over the whole window: its body in the safe
    /// area, over a program its capsule; and the toast's bottom over it.
    fn layer(
        &mut self,
        page: Page,
        edges: gpui_kit::Edges<gpui_kit::Pixels>,
        window: &Window,
        palette: look::Palette,
        cx: &mut Context<Self>,
    ) -> (gpui_kit::Stateful<gpui_kit::Div>, gpui_kit::Pixels) {
        let layer = div()
            .id(page.layer())
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .bg(palette.background());
        let safe = || {
            div()
                .flex_1()
                .min_h_0()
                .pt(edges.top)
                .pl(edges.left)
                .pr(edges.right)
        };
        match page {
            Page::About(program) => {
                let page = home::row_of(&self.shared, program, cx).map(|row| {
                    let listing = ducktape_host::runtime::listing(row.program);
                    about::page(&row, listing, palette, cx)
                });
                let body = safe().flex().flex_col().pb(edges.bottom).children(page);
                (layer.child(body), edges.bottom + px(look::gap::LG))
            }
            Page::Program(program) => {
                let instance = self.stack.read(cx).instance(program);
                let seat = instance.and_then(|instance| self.shared.seats.read(cx).seat(instance));
                let row = home::row_of(&self.shared, program, cx);
                let body = running::body(seat.as_ref(), row.as_ref(), palette, cx);
                let capsule = running::capsule(program, edges.top, edges.right, palette, cx);
                let layer = layer
                    .child(safe().pb(edges.bottom).child(body))
                    .child(capsule);
                (layer, edges.bottom + px(look::gap::LG))
            }
            Page::Root(tab) => {
                let screen = match tab {
                    Tab::Home => {
                        let searching = self.search.read(cx).focus_handle(cx).is_focused(window);
                        home::home(&self.shared, &self.search, searching, palette, cx)
                    }
                    Tab::Open => {
                        let opened = self.stack.read(cx).open().to_vec();
                        home::open(&self.shared, &opened, &self.titles, palette, cx)
                    }
                };
                let body = safe().flex().flex_col().child(screen).child(home::tab_bar(
                    tab,
                    edges.bottom,
                    palette,
                    cx,
                ));
                let bottom = edges.bottom + px(look::TAB_BAR + look::gap::LG);
                (layer.child(body), bottom)
            }
        }
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edges = ducktape_host::contract::os().insets(window).effective();
        let palette = look::palette(window);
        let viewport = window.viewport_size();
        // the safe body every seat is laid out in: the screen less its
        // insets (the keyboard's among them), at its own width whatever a
        // view's minimum
        let body = (
            f32::from(viewport.width - edges.left - edges.right).max(0.),
            f32::from(viewport.height - edges.top - edges.bottom).max(0.),
        );
        // a move at its end is over before anything reads it: `place` puts
        // the page it left out of front this draw
        let now = motion::now(cx);
        if self.moving.is_some_and(|moving| moving.tween.done(now)) {
            self.moving = None;
        }
        self.place(window, body, cx);
        let mut root = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(palette.background())
            .text_color(palette.foreground());
        if join::step(*self.shared.screen.read(cx).get()).is_some() {
            return root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .pt(edges.top)
                    .pl(edges.left)
                    .pr(edges.right)
                    .pb(edges.bottom)
                    .child(self.join.clone()),
            );
        }
        self.width = f32::from(viewport.width);
        let page = self.page(cx);
        let (layer, toast_bottom) = self.layer(page, edges, window, palette, cx);
        // the stack moving: the page it left drawn too, and taking no press
        let under_finger = self.dragged.zip(self.under(cx));
        let moving = match (under_finger, self.moving) {
            (Some((x, under)), _) => {
                let progress = (x / self.width.max(1.)).clamp(0., 1.);
                let (_, slide) = motion::stack(true, cx);
                Some((under, true, slide.at(progress, progress, true)))
            }
            (None, Some(moving)) => {
                let progress = moving.tween.frame(window, cx);
                let time = moving.tween.time(now);
                let placed = moving.slide.at(progress, time, moving.pop);
                Some((moving.left, moving.pop, placed))
            }
            (None, None) => None,
        };
        root = match moving {
            None => root.child(layer),
            Some((left, pop, placed)) => {
                let (other, _) = self.layer(left, edges, window, palette, cx);
                let other = other.child(div().absolute().inset_0().occlude());
                let (over, under) = match pop {
                    true => (other, layer),
                    false => (layer, other),
                };
                let width = self.width;
                let shift = |layer: gpui_kit::Stateful<gpui_kit::Div>, by: f32| {
                    layer.left(px(by * width)).right(px(-by * width))
                };
                let dim = div()
                    .absolute()
                    .inset_0()
                    .bg(gpui_kit::hsla(0., 0., 0., placed.dim));
                root.child(shift(under, placed.under))
                    .child(dim)
                    .child(shift(over, placed.top).opacity(placed.opacity))
            }
        };
        let said = self.shared.toast.read(cx).get().clone();
        let said = (!said.is_empty()).then_some(said);
        let toasts = (motion::TOAST_IN, motion::TOAST_OUT);
        if let Some(toast) = motion::follow(&mut self.drawn_toast, said, toasts, window, cx) {
            root = root.child(Self::toast(
                toast.what,
                toast_bottom,
                toast.progress,
                palette,
            ));
        }
        let room = viewport.height - edges.top;
        let sheets = (motion::SHEET_IN, motion::SHEET_OUT);
        let at = |(progress, slides, leaving), height: &Rc<Cell<_>>| sheet::At {
            bottom: edges.bottom,
            room,
            progress,
            slides,
            leaving,
            height: height.clone(),
        };
        if self.drawn_sheet.is_none() {
            self.sheet_height.set(None);
        }
        if let Some(shown) = motion::follow(&mut self.drawn_sheet, self.sheet, sheets, window, cx) {
            let at = at(shown.at(), &self.sheet_height);
            match self.sheet(shown.what, at, palette, cx) {
                Some(sheet) => root = root.child(sheet),
                // what a sheet was over is gone (a program no longer
                // listed): `place` withheld the keys for it this draw, so
                // the window draws again
                None => {
                    self.drawn_sheet = None;
                    if self.sheet.take().is_some() {
                        cx.notify();
                    }
                }
            }
        }
        let alerts = (motion::ALERT_IN, motion::ALERT_OUT);
        let alert = self.alert.clone();
        if let Some(shown) = motion::follow(&mut self.drawn_alert, alert, alerts, window, cx) {
            let faded = (shown.progress, shown.leaving);
            root = root.child(self.alert_card(shown.what, faded, palette, cx));
        }
        // an ask answered elsewhere is no longer the queue's front: its
        // sheet goes
        let asked = self.consent.and_then(|(ask, opened)| {
            let (_, words) = consent::front().filter(|(front, _)| *front == ask)?;
            Some((ask, opened, words))
        });
        if self.consent.is_some() && asked.is_none() {
            self.consent = None;
            cx.notify();
        }
        if self.drawn_consent.is_none() {
            self.consent_height.set(None);
        }
        if let Some(shown) = motion::follow(&mut self.drawn_consent, asked, sheets, window, cx) {
            let at = at(shown.at(), &self.consent_height);
            root = root.child(self.consent_sheet(shown.what, at, palette, cx));
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ducktape_host::runtime::Roster;
    use ducktape_host::session::Screen;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};

    const PROGRAM: &str = "phone-root-test";

    /// The program on top of the stack.
    fn top(stack: &Entity<Stack>, cx: &mut VisualTestContext) -> Option<&'static str> {
        stack.read_with(cx, |stack, _| stack.top())
    }

    /// The frames the last draw asked for, delivered (and drawn): how many
    /// were asked for.
    fn frames(cx: &mut VisualTestContext) -> usize {
        let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        asked
    }

    /// The clock past every motion under way, and the frame each asked for.
    fn run_out(cx: &mut VisualTestContext) {
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        frames(cx);
    }

    /// A phone's window on a network (the stack, not the join): its root,
    /// its stack, the host's entities.
    fn phone(cx: &mut TestAppContext) -> (Entity<Root>, Entity<Stack>, &mut VisualTestContext) {
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let shared = cx.update(|cx| Shared::for_test(Default::default(), Default::default(), cx));
        let stack = cx.new(|_| Stack::default());
        cx.update(|cx| {
            cx.set_global(Phone {
                shared: shared.clone(),
                stack: stack.clone(),
                window: None,
                waiting: Vec::new(),
            })
        });
        let (root, cx) =
            cx.add_window_view(|window, cx| Root::new(shared.clone(), stack.clone(), window, cx));
        cx.update(|window, _| window.activate_window());
        shared.screen.update(cx, |screen, cx| {
            screen.set(Screen::Desk, cx);
        });
        cx.run_until_parked();
        (root, stack, cx)
    }

    /// The stack's move under way: the page it left, whether it pops.
    fn moving(root: &Entity<Root>, cx: &mut VisualTestContext) -> Option<(Page, bool)> {
        root.read_with(cx, |root, _| root.moving.map(|it| (it.left, it.pop)))
    }

    /// A program opened from Home slides in over it, asking for frames only
    /// while it moves, and is gone to the still layout at its end, with no
    /// frame asked for after; a second push during a push ends the first
    /// where it was going; Back slides the program off; under Reduce Motion
    /// nothing slides (a cross-fade); the edge swipe drags the program off
    /// under the finger, springs back when let go short, and goes on from
    /// where the finger was when it is Back.
    #[gpui_kit::test]
    fn the_stack_moves_while_it_changes_and_never_after(cx: &mut TestAppContext) {
        let (root, stack, cx) = phone(cx);
        assert_eq!(frames(cx), 0, "a still window asked for a frame");

        // where Home is drawn, if it is: its filter field, the one element
        // the kit's test support observes there
        let search = root.read_with(cx, |root, _| root.search.entity_id().as_u64());
        let home = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                window.render_frame(cx);
                let field = gpui_kit::ElementId::NamedInteger("input".into(), search);
                window.try_find(field).map(|it| it.bounds())
            })
        };
        let still = home(cx).expect("Home drawn");
        root.update(cx, |root, cx| root.open(PROGRAM, cx));
        cx.run_until_parked();
        assert_eq!(moving(&root, cx), Some((Page::Root(Tab::Home), false)));
        assert!(frames(cx) > 0, "a push asked for no frame");
        cx.executor().advance_clock(motion::PUSH.duration / 4);
        assert!(frames(cx) > 0, "a push under way asked for no frame");
        assert!(moving(&root, cx).is_some(), "the push ended early");
        let under = home(cx).expect("Home drawn under the push");
        assert!(
            under.left() < still.left(),
            "Home is not moving left under it"
        );
        cx.executor().advance_clock(motion::PUSH.duration);
        frames(cx);
        assert_eq!(moving(&root, cx), None, "the push outlived its motion");
        assert_eq!(frames(cx), 0, "a frame asked for after the push ended");
        assert_eq!(home(cx), None, "Home still drawn under the program");

        // Back: the program slides off and Home lands exactly where it was
        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        assert_eq!(moving(&root, cx), Some((Page::Program(PROGRAM), true)));
        cx.executor().advance_clock(motion::POP.duration / 4);
        frames(cx);
        let coming = home(cx).expect("Home drawn under the pop");
        assert!(
            coming.left() < still.left(),
            "Home is not coming from the left"
        );
        run_out(cx);
        assert_eq!(home(cx), Some(still), "the pop ended off the still layout");
        assert_eq!(frames(cx), 0, "a frame asked for after the pop ended");

        // a second push during the first: from the page the first was going to
        root.update(cx, |root, cx| root.open("phone-root-second", cx));
        root.update(cx, |root, cx| root.open("phone-root-third", cx));
        cx.run_until_parked();
        assert_eq!(
            moving(&root, cx),
            Some((Page::Program("phone-root-second"), false))
        );
        run_out(cx);

        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        assert_eq!(top(&stack, cx), Some("phone-root-second"));
        assert_eq!(
            moving(&root, cx),
            Some((Page::Program("phone-root-third"), true)),
            "Back did not slide the program off"
        );
        run_out(cx);
        assert_eq!(frames(cx), 0, "a frame asked for after the pop ended");

        // the capsule's ×: Home, the program slid off it
        root.update(cx, |root, cx| root.go_home(cx));
        cx.run_until_parked();
        assert_eq!(
            moving(&root, cx),
            Some((Page::Program("phone-root-second"), true))
        );
        run_out(cx);

        cx.update(|_, cx| cx.set_reduce_motion(true));
        root.update(cx, |root, cx| root.open(PROGRAM, cx));
        let (fade, slide) = root.read_with(cx, |root, _| {
            let moving = root.moving.expect("no move under Reduce Motion");
            (moving.tween.motion, moving.slide)
        });
        assert_eq!(fade, motion::FADE, "Reduce Motion kept the slide's motion");
        assert_eq!((slide.over, slide.under, slide.dim), (0., 0., 0.));
        root.update(cx, |root, cx| root.drag(40., cx));
        assert!(
            root.read_with(cx, |root, _| root.dragged.is_none()),
            "a drag followed under Reduce Motion"
        );
        run_out(cx);
        cx.update(|_, cx| cx.set_reduce_motion(false));
        cx.run_until_parked();

        let width = root.read_with(cx, |root, _| root.width);
        assert!(width > 0.);
        root.update(cx, |root, cx| root.drag(width / 4., cx));
        cx.run_until_parked();
        assert_eq!(
            frames(cx),
            0,
            "a drag asks for no frame: the finger moves it"
        );
        root.update(cx, |root, cx| root.drag_cancelled(cx));
        cx.run_until_parked();
        let back = root.read_with(cx, |root, _| {
            root.moving.map(|it| (it.left, it.pop, it.tween.from))
        });
        assert_eq!(
            back,
            Some((Page::Root(Tab::Home), false, 0.75)),
            "let go short"
        );
        assert_eq!(top(&stack, cx), Some(PROGRAM), "a drag let go short popped");
        run_out(cx);

        root.update(cx, |root, cx| root.drag(width / 2., cx));
        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        let off = root.read_with(cx, |root, _| {
            root.moving.map(|it| (it.left, it.pop, it.tween.from))
        });
        assert_eq!(
            off,
            Some((Page::Program(PROGRAM), true, 0.5)),
            "Back from a drag"
        );
        assert_eq!(top(&stack, cx), None);
        run_out(cx);
        assert_eq!(frames(cx), 0);
    }

    /// A sheet slides up and stays up asking for no frame; closed, it goes
    /// down as it was drawn and is dropped at the end, with no frame asked
    /// for after; the alert and the toast fade in and out the same way.
    #[gpui_kit::test]
    fn the_overlays_come_and_go_and_then_stay_still(cx: &mut TestAppContext) {
        let (root, _, cx) = phone(cx);
        let sheet = |cx: &mut VisualTestContext| {
            root.read_with(cx, |root, _| {
                root.drawn_sheet.as_ref().map(|it| (it.what, it.leaving))
            })
        };
        root.update(cx, |root, cx| root.account_sheet(cx));
        cx.run_until_parked();
        assert_eq!(sheet(cx), Some((Sheet::Account, false)));
        assert!(frames(cx) > 0, "a sheet coming asked for no frame");
        run_out(cx);
        assert_eq!(sheet(cx), Some((Sheet::Account, false)));
        assert_eq!(frames(cx), 0, "a sheet up asked for a frame");

        root.update(cx, |root, cx| root.close_sheet(cx));
        cx.run_until_parked();
        assert_eq!(
            sheet(cx),
            Some((Sheet::Account, true)),
            "a sheet went at once"
        );
        assert!(frames(cx) > 0);
        run_out(cx);
        assert_eq!(sheet(cx), None, "a sheet gone is still drawn");
        assert_eq!(frames(cx), 0, "a frame asked for after the sheet went");

        root.update(cx, |root, cx| root.alert("Another network", "x".into(), cx));
        cx.run_until_parked();
        assert!(frames(cx) > 0, "an alert coming asked for no frame");
        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        let leaving = root.read_with(cx, |root, _| root.drawn_alert.as_ref().map(|it| it.leaving));
        assert_eq!(leaving, Some(true), "the alert went at once");
        run_out(cx);
        assert!(root.read_with(cx, |root, _| root.drawn_alert.is_none()));
        assert_eq!(frames(cx), 0);

        root.update(cx, |root, cx| root.notice("Copied".into(), cx));
        cx.run_until_parked();
        assert!(frames(cx) > 0, "a toast coming asked for no frame");
        run_out(cx);
        assert_eq!(frames(cx), 0, "a toast up asked for a frame");
    }

    /// The window over a phone on a network with an account (the stack, not
    /// the join), its root, through the host's own entities: a link held
    /// until the program list comes, then opened; the program on top placed
    /// in front with the keys; the capsule's menu withholding them; Back
    /// closing the sheet, then popping (the seat kept, out of front); a
    /// consent ask refused by Back; a memory warning letting the seat of a
    /// program left go with no draw.
    #[gpui_kit::test]
    fn the_root_holds_links_places_seats_and_walks_back(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let shared = cx.update(|cx| Shared::for_test(Default::default(), Default::default(), cx));
        let stack = cx.new(|_| Stack::default());
        cx.update(|cx| {
            cx.set_global(Phone {
                shared: shared.clone(),
                stack: stack.clone(),
                window: None,
                waiting: Vec::new(),
            })
        });
        let (root, cx) =
            cx.add_window_view(|window, cx| Root::new(shared.clone(), stack.clone(), window, cx));
        cx.update(|window, _| window.activate_window());
        shared.screen.update(cx, |screen, cx| {
            screen.set(Screen::Desk, cx);
        });
        cx.run_until_parked();

        root.update(cx, |root, cx| {
            root.take_link(format!("duck://{PROGRAM}"), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            top(&stack, cx),
            None,
            "a link opened before the program list"
        );
        ducktape_host::runtime::seat_for_test(PROGRAM, 320);
        shared.rail.update(cx, |rail, cx| {
            rail.read_off(Roster::listing(&[PROGRAM]), cx)
        });
        cx.run_until_parked();
        assert_eq!(top(&stack, cx), Some(PROGRAM), "the held link did not open");

        let instance = stack
            .read_with(cx, |stack, _| stack.instance(PROGRAM))
            .expect("opened");
        let seat = |cx: &mut VisualTestContext| {
            shared.seats.read_with(cx, |seats, _| seats.seat(instance))
        };
        let placed = |cx: &mut VisualTestContext| {
            seat(cx)
                .expect("a seat")
                .read_with(cx, |seat, _| (seat.in_front(), seat.keys_free()))
        };
        assert_eq!(placed(cx), (true, true), "on top: in front, the keys its");

        root.update(cx, |root, cx| root.capsule_menu(PROGRAM, cx));
        cx.run_until_parked();
        assert_eq!(
            placed(cx),
            (true, false),
            "the keys under the capsule's menu"
        );

        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        assert!(
            root.read_with(cx, |root, _| root.sheet.is_none()),
            "Back kept the sheet"
        );
        assert_eq!(
            placed(cx),
            (true, true),
            "the keys back with the sheet gone"
        );
        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        assert_eq!(top(&stack, cx), None, "Back did not pop");
        assert_eq!(
            placed(cx),
            (true, false),
            "a program sliding off is drawn, in front, without the keys"
        );
        run_out(cx);
        assert_eq!(
            placed(cx),
            (false, false),
            "a program left stays seated, out of front"
        );

        let words = consent::Words {
            title: consent::ACCOUNT,
            said: "Root test".into(),
            id: None,
            shown: None,
            described: false,
            card: Default::default(),
        };
        let mut told = consent::queue(PROGRAM, 0, words).expect("queued");
        root.update(cx, |root, cx| root.consent_moved(cx));
        cx.run_until_parked();
        assert!(
            root.read_with(cx, |root, _| root.consent.is_some()),
            "the ask did not show"
        );
        root.update(cx, |root, cx| root.back(cx));
        assert_eq!(
            futures::FutureExt::now_or_never(told.answer()),
            Some(Some(false)),
            "Back did not refuse the ask"
        );

        cx.update(|_, cx| handed_on(Signal::MemoryWarning, cx));
        assert!(
            seat(cx).is_none(),
            "the seat of a program left outlived the warning"
        );
    }
}
