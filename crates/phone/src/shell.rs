//! The phone's one window: the host started once per process ([`launch`]),
//! the window opened over it ([`open_window`]: once on iOS, once per
//! Activity on Android), what the host hands on acted on (Back, a changed
//! inset, memory pressure, the consent queue, a link), and in the gate
//! build the AX door.
//!
//! The window draws the join ([`join`]) until the phone is on a network
//! with an account, then the [`stack`]: a tab root (Home, Open) under the
//! tab bar, or the program on top, its view seated full screen under the
//! capsule; the shell's own pages over either (a program's page, its
//! capabilities, from the capsule's About; App settings and Switch network
//! from the account menu); a sheet over all (the account menu, the
//! capsule's menu, the consent sheet). Each program opened keeps one seat,
//! placed with the safe body and in front only while it is on top and the
//! window is: a program left stays warm where it was (wasm-mobile SPEC
//! §7.1).
//!
//! It moves as the Expo build's moves (`ducktape_host::motion`): a program
//! (or its page) pushed slides in from the right over the screen it left,
//! which moves left under a dim, and a pop is the reverse; iOS's left-edge
//! swipe drags the screen on top off under the finger (inside a program
//! above its root page, that page, by the view's own tree); a sheet slides up
//! over a fading scrim and down again; the alert and the toast fade. Tab
//! switches do not move (Expo's tabs do not). Nothing moves while nothing
//! changes: a frame is asked for only while something is under way.

mod about;
mod debug;
mod home;
mod join;
mod look;
mod running;
mod settings;
mod sheet;
pub mod stack;

use ducktape_host::a11y::Control as _;
use ducktape_host::backend::Appearance;
use ducktape_host::contract::{Boot, KeyStorage, OsServices, Signal, SoftKeyboard};
use ducktape_host::motion::{self, Shown, Slide, Tween};
use ducktape_host::render::ViewTree;
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
    /// This phone's push tokens kept registered with the push server, for
    /// as long as the app runs.
    _registrar: Entity<crate::push::Registrar>,
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
    // debug mode left on: the counters on from the start (never turned off
    // here, where `DUCKTAPE_PERF=1` may have turned them on)
    if host.shared.prefs.read(cx).get().debug {
        ducktape_host::perf::switch(true);
    }
    if let Some(target) = Session::boot_target() {
        host.shared
            .session
            .update(cx, |session, cx| session.connect(target, cx));
    }
    let (signals, mut asks) = (host.signals, host.consent);
    let stack = cx.new(|_| Stack::default());
    let registrar = crate::push::Registrar::start(host.shared.clone(), cx);
    cx.set_global(Phone {
        shared: host.shared,
        stack,
        window: None,
        waiting: Vec::new(),
        _registrar: registrar,
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

/// `appearance` (App settings › Appearance) onto the kit theme, which the
/// phone's palette (`look::palette`), every view's theme and the host's own
/// chrome read; under System, the window's own; then the system bars over
/// it. Called as the window opens, when the choice moves and when the
/// system's does.
fn sync_appearance(appearance: Appearance, window: &mut Window, cx: &mut App) {
    use gpui_kit::component::{Theme, ThemeMode};
    match appearance {
        Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
        Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
        Appearance::System => Theme::sync_system_appearance(Some(window), cx),
    }
    crate::os::system_bars(Theme::global(cx).is_dark());
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

/// What the stack shows full screen: a tab root, the program on top, or
/// one of the shell's own pages pushed over them (a program's page, App
/// settings, its Appearance, Switch network).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Root(Tab),
    Program(&'static str),
    About(&'static str),
    Settings,
    Appearance,
    Switch,
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
            Page::Settings => "screen::settings".into(),
            Page::Appearance => "screen::appearance".into(),
            Page::Switch => "screen::switch".into(),
        }
    }

    /// The program seated full screen on it, if any.
    fn program(self) -> Option<&'static str> {
        match self {
            Page::Program(program) => Some(program),
            Page::Root(_) | Page::About(_) | Page::Settings | Page::Appearance | Page::Switch => {
                None
            }
        }
    }

    /// One of the shell's own pages, pushed over the stack.
    fn pushed(self) -> bool {
        matches!(
            self,
            Page::About(_) | Page::Settings | Page::Appearance | Page::Switch
        )
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
    /// The shell's own pages over the stack, the top last: a program's
    /// page, App settings, Switch network (over App settings, or alone).
    pages: Vec<Page>,
    /// Where this phone's key is kept, as the store said when App settings
    /// opened (Expo reads it once per keyring): its words, or why the store
    /// did not say.
    storage: Option<Result<KeyStorage, String>>,
    /// The title each seat's view gave its window, by its instance.
    titles: BTreeMap<u64, String>,
    /// What `Seats` was last told: each open program's place, and the seat
    /// whose view may move the keys.
    placed: (BTreeMap<u64, Place>, Option<u64>),
    /// Links that came before the network's program list did, in order.
    waiting: Vec<String>,
    /// The appearance last put on the theme (`sync_appearance`).
    appearance: Appearance,
    /// Debug mode's numbers, read while the Open tab is on screen.
    readout: debug::Readout,
    /// Home's filter, and whether it is open (its magnifier pressed).
    search: Entity<InputState>,
    filtering: bool,
    /// Each placed seat, observed: a tree, a standin or a fault that moved
    /// draws the window again.
    watched: BTreeMap<u64, Subscription>,
    /// The stack moving, until its motion ends.
    moving: Option<Move>,
    /// How far in (points) iOS's left-edge swipe has come while it drags
    /// the screen on top off.
    dragged: Option<f32>,
    /// The view's tree whose page the swipe drags instead, until let go.
    swiped: Option<WeakEntity<ViewTree>>,
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
            cx.observe_in(&shared.prefs, window, |root, prefs, window, cx| {
                let appearance = prefs.read(cx).get().appearance;
                if appearance != root.appearance {
                    root.appearance = appearance;
                    sync_appearance(appearance, window, cx);
                }
                cx.notify();
            }),
            cx.observe(&stack, |_, _, cx| cx.notify()),
            cx.observe(&shared.seats, |root, _, cx| root.watch_seats(cx)),
            cx.subscribe(&shared.seats, |root, _, event: &SeatsEvent, cx| {
                root.seats_said(event, cx)
            }),
            cx.subscribe(
                &shared.session,
                |root, _, event: &SessionEvent, cx| match event {
                    SessionEvent::LeftNetwork => root.left_network(cx),
                    // the same chain answered (another chain left it first);
                    // only the host's `connect_answered` sends it today, a
                    // new sender would pop Switch network too
                    SessionEvent::Connected => root.switched(cx),
                    // only the host's `connect_failed` sends it (a reach
                    // failed while on a network): taken as the switch's
                    SessionEvent::Toast(said) => root.switch_failed(said.clone(), cx),
                },
            ),
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
            cx.observe_window_appearance(window, |root, window, cx| {
                sync_appearance(root.appearance, window, cx);
                cx.notify();
            }),
        ];
        subscriptions.extend(crate::os::follow(window, cx));
        let appearance = shared.prefs.read(cx).get().appearance;
        sync_appearance(appearance, window, cx);
        let join = cx.new(|cx| join::Join::new(shared.clone(), window, cx));
        subscriptions.push(cx.subscribe(&join, |root, _, _: &join::Stayed, cx| root.switched(cx)));
        Self {
            shared,
            join,
            stack,
            sheet: None,
            consent: None,
            alert: None,
            pages: Vec::new(),
            storage: None,
            titles: BTreeMap::new(),
            placed: Default::default(),
            waiting: Vec::new(),
            appearance,
            readout: debug::Readout::default(),
            search,
            filtering: false,
            watched: BTreeMap::new(),
            moving: None,
            dragged: None,
            swiped: None,
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

    /// Home's filter opened, the keys in its field; or closed, its words
    /// gone and every program back.
    fn filter(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.filtering = open;
        self.search.update(cx, |field, cx| match open {
            true => field.focus(window, cx),
            false => field.set_value("", window, cx),
        });
        if !open {
            window.blur(cx);
        }
        cx.notify();
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
        self.pages.clear();
        let instance = self.stack.update(cx, |stack, cx| {
            let instance = stack.push(program);
            cx.notify();
            instance
        });
        self.moved(from, false, cx);
        instance
    }

    /// One of the shell's own pages pushed over the screen, the sheet it
    /// came from closed.
    fn push_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.sheet = None;
        let from = self.page(cx);
        self.pages.push(page);
        self.moved(from, false, cx);
        cx.notify();
    }

    /// The shell's page on top popped.
    fn pop_page(&mut self, cx: &mut Context<Self>) {
        let from = self.page(cx);
        self.pages.pop();
        self.moved(from, true, cx);
        cx.notify();
    }

    /// `program`'s page pushed over the stack.
    fn show_about(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.push_page(Page::About(program), cx);
    }

    /// App settings pushed (Expo `you.tsx`'s and `settings.tsx`'s), where
    /// this phone's key is kept read once from the store as it opens.
    fn show_settings(&mut self, cx: &mut Context<Self>) {
        let keyring = self.shared.account.read(cx).keyring().to_owned();
        self.storage = (!keyring.is_empty()).then(|| {
            ducktape_host::contract::os()
                .key_storage(&keyring)
                .map_err(|refused| refused.sentence())
        });
        self.push_page(Page::Settings, cx);
    }

    /// Switch network pushed, its field holding the last node reached.
    fn show_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.join
            .update(cx, |join, cx| join.open_switch(window, cx));
        self.push_page(Page::Switch, cx);
    }

    /// The switch landed on the chain in hand (another node of it, or the
    /// node in hand itself): Switch network goes back where the reader
    /// was. Another chain left the network first, which took the page.
    fn switched(&mut self, cx: &mut Context<Self>) {
        if self.page(cx) == Page::Switch {
            self.pop_page(cx);
        }
    }

    /// The switch did not land: on Switch network its words go under the
    /// field (Expo 75), not over the screen in the notice the host put up.
    fn switch_failed(&mut self, said: String, cx: &mut Context<Self>) {
        if self.page(cx) != Page::Switch {
            return;
        }
        self.join
            .update(cx, |join, cx| join.switch_failed(said, cx));
        // deferred: the notice is down after the host's `Toast` put it up,
        // whichever of the two heard the session first
        let toast = self.shared.toast.clone();
        cx.defer(move |cx| toast.update(cx, |toast, cx| toast.dismiss(cx)));
    }

    /// What the stack shows now: the shell's page on top (a program's page
    /// while the network lists it), else the program on top, else the
    /// tab's root.
    fn page(&self, cx: &App) -> Page {
        let listed = |page: &&Page| match page {
            Page::About(program) => home::row_of(&self.shared, program, cx).is_some(),
            _ => true,
        };
        if let Some(&page) = self.pages.last().filter(listed) {
            return page;
        }
        let stack = self.stack.read(cx);
        stack.top().map_or(Page::Root(stack.tab()), Page::Program)
    }

    /// The screen a pop would show: the page under the shell's page on
    /// top, else the one under the program on top; `None` on a root.
    fn under(&self, cx: &App) -> Option<Page> {
        let stack = self.stack.read(cx);
        let root = Page::Root(stack.tab());
        if self.page(cx).pushed() {
            let below = self.pages.len().checked_sub(2).map(|at| self.pages[at]);
            return Some(below.unwrap_or_else(|| stack.top().map_or(root, Page::Program)));
        }
        stack.top()?;
        Some(stack.under().map_or(root, Page::Program))
    }

    /// The stack went from `from` to the screen it shows now, by a push, or
    /// a pop when `pop`: the new screen slides in over the old, or the old
    /// one off the new. A move under way ends where it was going, unless
    /// this one undoes it (Back during a push): that turns back from where
    /// it is. A pop the finger let go of goes on from where it let go. Tab
    /// switches do not move.
    fn moved(&mut self, from: Page, pop: bool, cx: &mut Context<Self>) {
        let dragged = self.dragged.take();
        let to = self.page(cx);
        if from == to || matches!((from, to), (Page::Root(_), Page::Root(_))) {
            return;
        }
        let (motion, slide) = motion::stack(pop, cx);
        let start = match (dragged, self.moving) {
            (Some(x), _) if pop && self.width > 0. => (x / self.width).clamp(0., 1.),
            (None, Some(m)) if m.left == to && m.pop != pop => 1. - m.tween.at(motion::now(cx)),
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
    /// When the view on top hears Back (a page above its root), its own
    /// page follows inside it instead, if its tree keeps the page under
    /// (`ViewTree::drag_page`), and that tree hears the release, whatever is
    /// on top by then. Under Reduce Motion nothing follows; the
    /// release is Back as before. A swipe begun while the stack moves does
    /// not follow either.
    fn drag(&mut self, x: f32, cx: &mut Context<Self>) {
        let progress = x / self.width.max(1.);
        // the view's page the finger holds stays its, while it follows
        if let Some(tree) = self.swiped.as_ref().and_then(WeakEntity::upgrade) {
            if tree.update(cx, |tree, cx| tree.drag_page(progress, cx)) {
                return;
            }
            self.swiped = None;
        }
        let now = motion::now(cx);
        let busy = self.moving.is_some_and(|moving| !moving.tween.done(now));
        if self.dragged.is_none() && (busy || !self.can_drag(cx)) {
            if !busy
                && let Some(tree) = self.view_swiped(cx)
                && tree.update(cx, |tree, cx| tree.drag_page(progress, cx))
            {
                self.swiped = Some(tree.downgrade());
            }
            return;
        }
        // a move that has ended may not be cleared yet: it is over
        self.moving = None;
        self.dragged = Some(x.max(0.));
        cx.notify();
    }

    fn can_drag(&self, cx: &App) -> bool {
        if self.covered(cx) || motion::reduced(cx) || self.under(cx).is_none() {
            return false;
        }
        self.page(cx).pushed()
            || self
                .top_seat(cx)
                .is_none_or(|seat| !seat.read(cx).hears_back())
    }

    /// Whether the join, or a sheet, the consent sheet or an alert of the
    /// shell's, is over the stack.
    fn covered(&self, cx: &App) -> bool {
        let covered = self.consent.is_some() || self.alert.is_some() || self.sheet.is_some();
        covered || join::step(*self.shared.screen.read(cx).get()).is_some()
    }

    /// The tree of the view on top when the edge swipe is the view's own:
    /// nothing of the shell's over it, and the view hears Back.
    fn view_swiped(&self, cx: &App) -> Option<Entity<ViewTree>> {
        if self.covered(cx) || !self.pages.is_empty() {
            return None;
        }
        let seat = self.top_seat(cx)?;
        let seat = seat.read(cx);
        seat.hears_back().then(|| seat.tree()).flatten()
    }

    /// The swipe let go short of Back: the screen on top slides back into
    /// place over the one under it, or the view's page inside it.
    fn drag_cancelled(&mut self, cx: &mut Context<Self>) {
        if let Some(tree) = self.swiped.take().and_then(|tree| tree.upgrade()) {
            tree.update(cx, |tree, cx| tree.drag_page_cancelled(cx));
        }
        let Some(x) = self.dragged.take() else {
            return;
        };
        cx.notify();
        // no travel: already in place, nothing to spring back
        if x <= 0. {
            return;
        }
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

    /// Back (Android's button, iOS's edge): on the join, the account step
    /// walks back on Android, the code to the name, the name to the address
    /// (iOS's swipe does nothing there), and the address is the OS's own;
    /// else the consent
    /// sheet's ask is refused, an alert dismissed, a sheet closed, the
    /// shell's page on top popped, each first; else the view on top goes back
    /// inside itself when its last frame's root asks to hear it; else the
    /// stack pops; at Home, the OS's own (SPEC §7.6). A drag under way
    /// that this Back does not pop springs back; a view's page under the
    /// finger that the view does not hear it for slides back.
    fn back(&mut self, cx: &mut Context<Self>) {
        let swiped = self.swiped.take().and_then(|tree| tree.upgrade());
        self.walk_back(cx);
        if let Some(tree) = swiped {
            tree.update(cx, |tree, cx| tree.drag_page_cancelled(cx));
        }
        if self.dragged.take().is_some() {
            cx.notify();
        }
    }

    fn walk_back(&mut self, cx: &mut Context<Self>) {
        match join::step(*self.shared.screen.read(cx).get()) {
            Some(join::Step::Node) => return crate::os::back_at_root(),
            // iOS's Back is only the edge swipe, which the account step
            // refuses as Expo's code does (`gestureEnabled: false`): its way
            // out is its own Back button
            Some(_) if cfg!(target_os = "ios") => return,
            Some(_) => return self.join.update(cx, |join, cx| join.back(cx)),
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
        if !self.pages.is_empty() {
            return self.pop_page(cx);
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
        self.pages.clear();
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
                    crate::os::open_settings(page, cx);
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
        // the shell's pages cover the program on top
        let top = stack
            .top()
            .and_then(|program| stack.instance(program))
            .filter(|_| self.pages.is_empty());
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

    /// The account menu (the shell boards' 40, Expo `you.tsx`): who is
    /// reading, the account's own program (the `programs` view) opened by
    /// name, App settings and Switch network.
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
        menu.child(
            sheet::row(
                "sheet/settings",
                Some("icons/settings.svg"),
                "App settings",
                Some("node · this phone · about".into()),
                palette,
            )
            .on_click(cx.listener(|root, _, _, cx| root.show_settings(cx))),
        )
        .child(
            sheet::row(
                "sheet/switch",
                Some("icons/refresh-cw.svg"),
                "Switch network",
                None,
                palette,
            )
            .on_click(cx.listener(|root, _, window, cx| root.show_switch(window, cx))),
        )
        .into_any_element()
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
            Page::Settings => {
                let page = settings::page(&self.shared, self.storage.as_ref(), palette, cx);
                let body = safe().flex().flex_col().pb(edges.bottom).child(page);
                (layer.child(body), edges.bottom + px(look::gap::LG))
            }
            Page::Appearance => {
                let page = settings::appearance(&self.shared, palette, cx);
                let body = safe().flex().flex_col().pb(edges.bottom).child(page);
                (layer.child(body), edges.bottom + px(look::gap::LG))
            }
            Page::Switch => {
                let back = cx.listener(|root, _, _, cx| root.pop_page(cx));
                let header = look::header("switch/back", back, palette);
                let body = safe()
                    .flex()
                    .flex_col()
                    .pb(edges.bottom)
                    .child(header)
                    .child(self.join.clone());
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
                        home::home(
                            &self.shared,
                            &self.search,
                            self.filtering,
                            searching,
                            palette,
                            cx,
                        )
                    }
                    Tab::Open => {
                        let opened = self.stack.read(cx).open().to_vec();
                        let debug = self.shared.prefs.read(cx).get().debug;
                        let readout = debug.then_some(&self.readout);
                        home::open(&self.shared, &opened, &self.titles, readout, palette, cx)
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
        let palette = look::palette(cx);
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
        // debug mode's numbers: read while the Open tab is on screen
        let open = join::step(*self.shared.screen.read(cx).get()).is_none()
            && self.page(cx) == Page::Root(Tab::Open);
        let shown = open && self.shared.prefs.read(cx).get().debug && window.is_window_active();
        self.readout.follow(shown, window, cx);
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
        // the stack moving: the page it left drawn too, and taking no press;
        // `left_over` when that page is the one on top (a pop sliding it
        // off), not under a push or under the finger
        let under_finger = self.dragged.zip(self.under(cx));
        let moving = match (under_finger, self.moving) {
            (Some((x, under)), _) => {
                let progress = (x / self.width.max(1.)).clamp(0., 1.);
                let (_, slide) = motion::stack(true, cx);
                Some((under, false, slide.at(progress, progress, true)))
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
            Some((left, left_over, placed)) => {
                let (other, _) = self.layer(left, edges, window, palette, cx);
                let other = other.child(div().absolute().inset_0().occlude());
                let (over, under) = match left_over {
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
        let sheet = motion::idiom().sheet;
        let sheets = (sheet, sheet);
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
            let registrar = crate::push::Registrar::start(shared.clone(), cx);
            cx.set_global(Phone {
                shared: shared.clone(),
                stack: stack.clone(),
                window: None,
                waiting: Vec::new(),
                _registrar: registrar,
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

        // where Home is drawn, if it is: its filter field (opened here), the
        // one element the kit's test support observes there
        root.update(cx, |root, _| root.filtering = true);
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
        cx.executor()
            .advance_clock(motion::idiom().push.duration / 4);
        assert!(frames(cx) > 0, "a push under way asked for no frame");
        assert!(moving(&root, cx).is_some(), "the push ended early");
        let under = home(cx).expect("Home drawn under the push");
        assert!(
            under.left() < still.left(),
            "Home is not moving left under it"
        );
        cx.executor().advance_clock(motion::idiom().push.duration);
        frames(cx);
        assert_eq!(moving(&root, cx), None, "the push outlived its motion");
        assert_eq!(frames(cx), 0, "a frame asked for after the push ended");
        assert_eq!(home(cx), None, "Home still drawn under the program");

        // Back: the program slides off and Home lands exactly where it was
        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        assert_eq!(moving(&root, cx), Some((Page::Program(PROGRAM), true)));
        cx.executor()
            .advance_clock(motion::idiom().push.duration / 4);
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
        assert!(
            home(cx).expect("Home under the finger").left() < still.left(),
            "Home is not under the program, to the left"
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

        // a swipe begun during a push leaves it be; Back during it turns it
        // back from where it is
        root.update(cx, |root, cx| root.open(PROGRAM, cx));
        cx.run_until_parked();
        cx.executor()
            .advance_clock(motion::idiom().push.duration / 8);
        frames(cx);
        root.update(cx, |root, cx| root.drag(10., cx));
        assert_eq!(
            root.read_with(cx, |root, _| (root.dragged, root.moving.is_some())),
            (None, true),
            "a swipe took over a push"
        );
        let pushed = root.read_with(cx, |root, cx| {
            root.moving.map(|it| it.tween.at(motion::now(cx)))
        });
        let pushed = pushed.expect("a push under way");
        assert!(0. < pushed && pushed < 1., "{pushed}");
        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        let back = root.read_with(cx, |root, _| {
            root.moving.map(|it| (it.left, it.pop, it.tween.from))
        });
        assert_eq!(
            back,
            Some((Page::Program(PROGRAM), true, 1. - pushed)),
            "Back during a push jumped"
        );
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

        // Back while a sheet still comes up turns it back from where it is
        root.update(cx, |root, cx| root.account_sheet(cx));
        cx.run_until_parked();
        cx.executor()
            .advance_clock(motion::idiom().sheet.duration / 8);
        frames(cx);
        let coming = root.read_with(cx, |root, cx| {
            root.drawn_sheet
                .as_ref()
                .map(|it| it.tween.at(motion::now(cx)))
        });
        let coming = coming.expect("a sheet coming");
        assert!(0. < coming && coming < 1., "{coming}");
        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        let going = root.read_with(cx, |root, _| {
            root.drawn_sheet
                .as_ref()
                .map(|it| (it.leaving, it.tween.from))
        });
        assert_eq!(going, Some((true, coming)), "Back made the sheet jump");
        run_out(cx);
        assert_eq!(sheet(cx), None);

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

    /// The account sheet's App settings and Switch network close it and
    /// push their pages over the screen it was on (Expo `you.tsx`'s
    /// `leave`).
    #[gpui_kit::test]
    fn the_account_sheets_rows_open_app_settings_and_switch_network(cx: &mut TestAppContext) {
        let (root, _, cx) = phone(cx);
        for (row, page) in [
            ("sheet/settings", Page::Settings),
            ("sheet/switch", Page::Switch),
        ] {
            root.update(cx, |root, cx| root.account_sheet(cx));
            run_out(cx);
            let at = cx.debug_bounds(row).expect("the row drawn");
            cx.simulate_click(at.center(), gpui_kit::Modifiers::none());
            cx.run_until_parked();
            let shown = root.read_with(cx, |root, cx| (root.sheet, root.page(cx)));
            assert_eq!(shown, (None, page), "{row}");
            run_out(cx);
            root.update(cx, |root, cx| root.back(cx));
            run_out(cx);
        }
    }

    /// App settings' Light and dark row opens Appearance; the palette (and
    /// the kit theme every view reads) follows the choice made there, not
    /// the system's, and System hands it back to the window's own.
    #[gpui_kit::test]
    fn the_palette_follows_the_appearance_chosen_not_the_system(cx: &mut TestAppContext) {
        let (root, _, cx) = phone(cx);
        let click = |id: &'static str, cx: &mut VisualTestContext| {
            let at = cx.debug_bounds(id).expect(id);
            cx.simulate_click(at.center(), gpui_kit::Modifiers::none());
            run_out(cx);
        };
        root.update(cx, |root, cx| root.show_settings(cx));
        run_out(cx);
        click("settings/appearance", cx);
        assert_eq!(
            root.read_with(cx, |root, cx| root.page(cx)),
            Page::Appearance
        );
        let system = cx.update(|window, _| {
            gpui_kit::component::ThemeMode::from(window.appearance()).is_dark()
        });
        let dark = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| {
                let theme = gpui_kit::component::Theme::global(cx).is_dark();
                (look::palette(cx).dark(), theme)
            })
        };
        for (choice, chosen) in [
            ("appearance/dark", true),
            ("appearance/light", false),
            ("appearance/system", system),
        ] {
            click(choice, cx);
            assert_eq!(dark(cx), (chosen, chosen), "{choice}");
        }
    }

    /// Debug mode off: no cost on Open and the perf counters off; on, each
    /// open program's cost under its name and the app's line over the
    /// list, the counters on; off again, both gone.
    #[gpui_kit::test]
    fn debug_mode_draws_costs_and_counts_only_while_on(cx: &mut TestAppContext) {
        let _perf = ducktape_host::perf::off_for_test();
        let (root, _, cx) = phone(cx);
        root.update(cx, |root, cx| _ = root.open(PROGRAM, cx));
        run_out(cx);
        root.update(cx, |root, cx| root.go_home(cx));
        run_out(cx);
        root.update(cx, |root, cx| root.pick(Tab::Open, cx));
        run_out(cx);
        let prefs = root.read_with(cx, |root, _| root.shared.prefs.clone());
        let shown = |cx: &mut VisualTestContext| {
            (
                cx.debug_bounds("open/phone-root-test/cost").is_some(),
                cx.debug_bounds("open/app").is_some(),
                ducktape_host::perf::on(),
            )
        };
        assert_eq!(shown(cx), (false, false, false), "off");
        prefs.update(cx, |prefs, cx| prefs.set_debug(true, cx));
        run_out(cx);
        assert_eq!(shown(cx), (true, true, true), "on");
        prefs.update(cx, |prefs, cx| prefs.set_debug(false, cx));
        run_out(cx);
        assert_eq!(shown(cx), (false, false, false), "off again");
    }

    /// Back on Switch network closes an alert over it first, as on any
    /// page, then pops it, to App settings when it came from there, then
    /// App settings off to where the sheet was: never the address screen's
    /// own Back, which puts the app behind the others.
    #[gpui_kit::test]
    fn back_on_switch_network_closes_an_alert_first_then_pops_it_and_never_leaves_the_app(
        cx: &mut TestAppContext,
    ) {
        let (root, _, cx) = phone(cx);
        root.update(cx, |root, cx| root.show_settings(cx));
        run_out(cx);
        cx.update(|window, cx| root.update(cx, |root, cx| root.show_switch(window, cx)));
        run_out(cx);
        let pages = |cx: &mut VisualTestContext| root.read_with(cx, |root, _| root.pages.clone());
        assert_eq!(pages(cx), [Page::Settings, Page::Switch]);
        root.update(cx, |root, cx| root.alert("Another network", "x".into(), cx));
        root.update(cx, |root, cx| root.back(cx));
        let alert = root.read_with(cx, |root, _| root.alert.is_some());
        assert_eq!(
            (alert, pages(cx)),
            (false, vec![Page::Settings, Page::Switch]),
            "Back did not close the alert alone"
        );
        run_out(cx);
        root.update(cx, |root, cx| root.back(cx));
        assert_eq!(
            pages(cx),
            [Page::Settings],
            "Back did not pop Switch network"
        );
        run_out(cx);
        root.update(cx, |root, cx| root.back(cx));
        assert_eq!(pages(cx), [], "Back did not pop App settings");
        let page = root.read_with(cx, |root, cx| root.page(cx));
        assert_eq!(page, Page::Root(Tab::Home));
    }

    /// On a network, Continue on the node in hand (the field holds it) pops
    /// Switch network at once; a switch whose node does not answer says why
    /// under the field, not in a notice over it, and the page stays; one
    /// the chain in hand answers pops the page back where the reader was.
    #[gpui_kit::test]
    fn a_switch_that_stays_or_lands_pops_the_page_and_a_failed_one_says_why_under_the_field(
        cx: &mut TestAppContext,
    ) {
        use ducktape_host::backend::RecentEndpoint;
        use ducktape_host::session::SessionState;
        use ducktape_host::session::tests::{source, status};
        use join::tests::until;
        const ORIGIN: &str = "http://127.0.0.1:1";
        let _seat = ducktape_host::backend::seat_serial();
        let (root, _, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        shared.account.update(cx, |account, _| {
            account.seed_network(ORIGIN, "testkit", "testkit")
        });
        let connected = SessionState {
            connected: true,
            connected_rpc: ORIGIN.into(),
            network: "testkit".into(),
            recent_endpoints: vec![RecentEndpoint {
                url: ORIGIN.into(),
                network: "testkit".into(),
                ..RecentEndpoint::default()
            }],
            ..SessionState::default()
        };
        shared
            .session
            .update(cx, |session, cx| session.seed(connected, cx));
        let switch = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| root.update(cx, |root, cx| root.show_switch(window, cx)));
            run_out(cx);
        };
        switch(cx);
        let at = cx.debug_bounds("join/connect").expect("Continue drawn");
        cx.simulate_click(at.center(), gpui_kit::Modifiers::none());
        cx.run_until_parked();
        let pages = root.read_with(cx, |root, _| root.pages.clone());
        assert_eq!(pages, [], "Continue on the node in hand stayed on the page");
        switch(cx);

        let (silent, _) =
            source(|_| Err("error sending request for url (http://127.0.0.1:9/v1/status)".into()));
        shared.session.update(cx, |session, cx| {
            session.connect_through("http://127.0.0.1:9".into(), silent, cx)
        });
        let join = root.read_with(cx, |root, _| root.join.clone());
        until(cx, |cx| {
            join.read_with(cx, |join, _| join.refused.is_some())
        });
        let said = join.read_with(cx, |join, _| join.refused.clone().unwrap_or_default());
        assert!(
            said.starts_with("Nothing answered at 127.0.0.1:9."),
            "{said}"
        );
        let notice = shared.toast.read_with(cx, |toast, _| toast.get().clone());
        assert_eq!(notice, "", "the failure was also said over the page");
        assert_eq!(root.read_with(cx, |root, cx| root.page(cx)), Page::Switch);

        let (answers, _) = source(|_| Ok(status(1)));
        shared.session.update(cx, |session, cx| {
            session.connect_through("http://127.0.0.1:2".into(), answers, cx)
        });
        until(cx, |cx| root.read_with(cx, |root, _| root.pages.is_empty()));
        let page = root.read_with(cx, |root, cx| root.page(cx));
        assert_eq!(page, Page::Root(Tab::Home), "the switch did not land here");
    }

    /// The window over a phone on a network with an account (the stack, not
    /// the join), its root, through the host's own entities: a link held
    /// until the program list comes, then opened; the program on top placed
    /// in front with the keys; the capsule's menu withholding them; Back
    /// closing the sheet, then popping (the seat kept, out of front); a
    /// consent ask refused by Back; a memory warning letting the seat of a
    /// program left go with no draw; leaving the network taking its program
    /// list with it, so a link is held again until the next network's list,
    /// never opened against the old one.
    #[gpui_kit::test]
    fn the_root_holds_links_places_seats_and_walks_back(cx: &mut TestAppContext) {
        // leaving the network locks the key: the seat is one for the process
        let _seat = ducktape_host::backend::seat_serial();
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let shared = cx.update(|cx| Shared::for_test(Default::default(), Default::default(), cx));
        let stack = cx.new(|_| Stack::default());
        cx.update(|cx| {
            let registrar = crate::push::Registrar::start(shared.clone(), cx);
            cx.set_global(Phone {
                shared: shared.clone(),
                stack: stack.clone(),
                window: None,
                waiting: Vec::new(),
                _registrar: registrar,
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

        shared
            .session
            .update(cx, |session, cx| session.disconnect(cx));
        cx.run_until_parked();
        assert!(
            shared.rail.read_with(cx, |rail, _| rail.rows().is_empty()),
            "the left network's programs are still listed"
        );
        root.update(cx, |root, cx| {
            root.take_link(format!("duck://{PROGRAM}"), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            top(&stack, cx),
            None,
            "a link opened against the left network's list"
        );
        shared.rail.update(cx, |rail, cx| {
            rail.read_off(Roster::listing(&[PROGRAM]), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            top(&stack, cx),
            Some(PROGRAM),
            "the link held over the leave did not open on the next list"
        );
    }

    /// A view's frame root showing the page `key`, `depth` over its root,
    /// hearing Back when `back`.
    fn view_page(key: &str, depth: u32, back: bool) -> view_wire::Node {
        let style = ducktape_host::render::test_style(div().size_full().style().clone());
        let page = view_wire::Node::Container(view_wire::ContainerNode {
            id: Some(view_wire::ElementIdWire::Name(key.into())),
            style,
            interactivity: Some(Box::new(view_wire::Interactivity {
                page: Some(depth),
                ..Default::default()
            })),
            children: Vec::new(),
        });
        view_wire::Node::Container(view_wire::ContainerNode {
            id: Some(view_wire::ElementIdWire::Name("view".into())),
            style,
            interactivity: Some(Box::new(view_wire::Interactivity {
                on_back: back.then_some(5),
                ..Default::default()
            })),
            children: vec![page],
        })
    }

    /// The tree of the view on top.
    fn view_tree(root: &Entity<Root>, cx: &mut VisualTestContext) -> Entity<ViewTree> {
        root.read_with(cx, |root, cx| root.top_seat(cx)?.read(cx).tree())
            .expect("the view's tree")
    }

    /// Inside a program above its root page, iOS's edge swipe drags the
    /// view's own page, not the program: the shell moves nothing of its
    /// own and no frame is asked for; let go short, the page slides back
    /// and the view hears nothing; let go past the rule, the view hears
    /// Back, and a page it did not pop to (it answered nothing new) slides
    /// back. Under an alert of the shell's, nothing of the view's follows.
    #[gpui_kit::test]
    fn the_edge_swipe_drags_a_views_page_inside_it(cx: &mut TestAppContext) {
        const SWIPED: &str = "phone-swipe-test";
        let _seat = ducktape_host::backend::seat_serial();
        ducktape_host::contract::testing::touch_first_for_test(true);
        // the view's root page, a page pushed over it on the next tick,
        // then nothing new on every tick after, a Back's included
        let frame = |root, busy| view_wire::Frame {
            root: Some(root),
            busy,
            ..Default::default()
        };
        ducktape_host::runtime::seat_ticking_for_test(
            SWIPED,
            320,
            &[
                frame(view_page("rooms", 0, false), true),
                frame(view_page("room", 1, true), false),
                view_wire::Frame {
                    unchanged: true,
                    ..Default::default()
                },
            ],
        );
        let (root, stack, cx) = phone(cx);
        root.update(cx, |root, cx| root.open(SWIPED, cx));
        run_out(cx);
        run_out(cx);
        let tree = view_tree(&root, cx);
        let finger = |cx: &mut VisualTestContext| tree.read_with(cx, |tree, _| tree.page_finger());
        let sliding = |cx: &mut VisualTestContext| tree.read_with(cx, |tree, _| tree.page_moving());
        let backs = || {
            ducktape_host::runtime::ticks_for_test(SWIPED)
                .iter()
                .flatten()
                .filter(|event| matches!(event, view_wire::Event::Back { .. }))
                .count()
        };
        let shell_moves = |cx: &mut VisualTestContext| {
            root.read_with(cx, |root, _| (root.dragged, root.moving.is_some()))
        };

        let width = root.read_with(cx, |root, _| root.width);
        root.update(cx, |root, cx| root.drag(width / 4., cx));
        cx.run_until_parked();
        assert_eq!(
            finger(cx),
            Some((0.25, false)),
            "the view's page did not follow"
        );
        assert_eq!(
            shell_moves(cx),
            (None, false),
            "the shell moved the program"
        );
        assert_eq!(
            frames(cx),
            0,
            "a drag asks for no frame: the finger moves it"
        );

        root.update(cx, |root, cx| root.drag_cancelled(cx));
        cx.run_until_parked();
        assert_eq!(finger(cx), None);
        assert!(sliding(cx), "let go short, the page did not slide back");
        run_out(cx);
        assert_eq!(backs(), 0, "a swipe let go short sent the view Back");
        assert_eq!(frames(cx), 0, "a frame asked for once still");

        root.update(cx, |root, cx| root.drag(width / 2., cx));
        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        assert_eq!(backs(), 1, "Back did not reach the view");
        assert_eq!(top(&stack, cx), Some(SWIPED), "the shell left the program");
        assert_eq!(
            shell_moves(cx),
            (None, false),
            "the shell moved the program"
        );
        assert_eq!(finger(cx), None, "the page held on past the view's answer");
        assert!(sliding(cx), "a page the view did not pop to stayed off");
        run_out(cx);
        assert_eq!(frames(cx), 0, "a frame asked for once still");

        root.update(cx, |root, _| {
            root.alert = Some(("Alert", "Over the program".into()))
        });
        root.update(cx, |root, cx| root.drag(width / 4., cx));
        assert_eq!(finger(cx), None, "the view's page followed under an alert");
        ducktape_host::contract::testing::touch_first_for_test(false);
    }

    /// Let go past the rule halfway in, the view answers its Back with the
    /// page under: on the first frame after, the page going is still where
    /// the finger left it, halfway across, and it slides off from there.
    #[gpui_kit::test]
    fn a_views_page_let_go_as_back_pops_from_the_finger(cx: &mut TestAppContext) {
        const SWIPED: &str = "phone-swipe-pop-test";
        let _seat = ducktape_host::backend::seat_serial();
        ducktape_host::contract::testing::touch_first_for_test(true);
        // the root page, the page pushed over it on the next tick, and the
        // root page again on every tick after: the Back's answer
        let frame = |root, busy| view_wire::Frame {
            root: Some(root),
            busy,
            ..Default::default()
        };
        ducktape_host::runtime::seat_ticking_for_test(
            SWIPED,
            320,
            &[
                frame(view_page("rooms", 0, false), true),
                frame(view_page("room", 1, true), false),
                frame(view_page("rooms", 0, false), false),
            ],
        );
        let (root, _stack, cx) = phone(cx);
        root.update(cx, |root, cx| root.open(SWIPED, cx));
        run_out(cx);
        run_out(cx);
        assert_eq!(
            ducktape_host::runtime::ticks_for_test(SWIPED).len(),
            2,
            "the view ticked past its page before the swipe"
        );
        let tree = view_tree(&root, cx);
        let left = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                window.render_frame(cx);
                let page = ducktape_host::render::native_id(&view_wire::ElementIdWire::Name(
                    "room".into(),
                ));
                window
                    .try_find(page)
                    .map(|it| f32::from(it.bounds().left()))
            })
        };
        let width = root.read_with(cx, |root, _| root.width);
        root.update(cx, |root, cx| root.drag(width / 2., cx));
        root.update(cx, |root, cx| root.back(cx));
        cx.run_until_parked();
        assert_eq!(tree.read_with(cx, |tree, _| tree.page_finger()), None);
        assert!(tree.read_with(cx, |tree, _| tree.page_moving()));
        let going = left(cx).expect("the page going drawn");
        assert!(
            (going - width / 2.).abs() < 1.,
            "the pop did not start at the finger: {going}, not {}",
            width / 2.
        );
        run_out(cx);
        assert_eq!(left(cx), None, "the page popped is still drawn");
        assert_eq!(frames(cx), 0, "a frame asked for once still");
        ducktape_host::contract::testing::touch_first_for_test(false);
    }
}
