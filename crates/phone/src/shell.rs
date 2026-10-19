//! The phone's one window: the host started once per process ([`launch`]),
//! the window opened over it ([`open_window`]: once on iOS, once per
//! Activity on Android), what the host hands on acted on (Back, a changed
//! inset, memory pressure, the consent queue, a link, a notice the OS
//! showed), and in the gate build the AX door.
//!
//! The window draws the join ([`join`]) until the phone is on a network
//! with an account, then the [`stack`]: a tab root (Home, Open,
//! Notifications, Account) under the tab bar, or the program on top, its
//! view seated full screen under the capsule; the shell's own pages over
//! either (a program's page, its capabilities, from the capsule's About); a
//! sheet over all (the capsule's menu, the Account tab's Add a device and
//! Switch network, the consent sheet). Each program
//! opened keeps one seat, placed with the safe body and in front only
//! while it is on top and the window is: a program left stays warm where
//! it was (wasm-mobile SPEC §7.1).
//!
//! It moves as the Expo build's moves (`ducktape_host::motion`): a program
//! (or its page) pushed slides in from the right over the screen it left,
//! which moves left under a dim, and a pop is the reverse; iOS's swipe back
//! (from the left edge, or from anywhere [`swipe`] lets it begin) drags the
//! screen on top off under the finger (inside a program above its root
//! page, that page, by the view's own tree), and its release carries the
//! finger's speed into the rest of the move; a sheet slides up
//! over a fading scrim and down again, or down under a finger
//! ([`sheet`]); the alert and the toast fade. A tab
//! picked in the bar does not move (Expo's tabs do not); a sideways swipe
//! at a tab root pages the tabs under the finger, the tab beside sliding
//! in with it, and its release goes on as a pop's does. Nothing moves
//! while nothing changes: a frame is asked for only while something is
//! under way.

mod about;
mod account;
mod approve;
mod debug;
mod home;
mod join;
mod look;
mod running;
mod sheet;
pub mod stack;
pub mod swipe;

use ducktape_host::a11y::Control as _;
use ducktape_host::backend::Appearance;
use ducktape_host::contract::{Boot, KeyStorage, OsServices, Signal, SoftKeyboard};
use ducktape_host::motion::{self, Shown, Slide, Tween};
use ducktape_host::render::ViewTree;
use ducktape_host::runtime::notify::{Entry, Opened as Banner, Permission};
use ducktape_host::runtime::{Back, Placed, consent};
use ducktape_host::session::{
    AccountEvent, AccountState, Opening, Place, Seats, SeatsEvent, Session, SessionEvent, Shared,
};
use futures::StreamExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, AsyncApp, ClipboardItem, Context, Entity,
    Focusable as _, FontWeight, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity,
    Window, WindowOptions, canvas, div, px,
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
    /// The OS's notices waiting for their chain's log ([`os_notice`]).
    notices: Vec<crate::os::OsNotice>,
    /// This phone's push tokens kept registered with the push server, for
    /// as long as the app runs; it knows which listed node serves a chain.
    registrar: Entity<crate::push::Registrar>,
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
    log_on_connect(&host.shared, cx);
    cx.set_global(Phone {
        shared: host.shared,
        stack,
        window: None,
        waiting: Vec::new(),
        notices: Vec::new(),
        registrar,
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
    let notices = crate::os::os_notices();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(notice) = notices.recv().await {
            cx.update(|cx| os_notice(notice, cx));
        }
    })
    .detach();
}

/// A program's notice the OS showed, as the push server carried it: into
/// the Notifications tab once its chain is the one in hand (another
/// chain's log is not open, and taking a chain up opens its log over
/// whatever the centre held), waiting here until then. One that came while
/// the app was in front on its chain goes: this phone runs its program's
/// notifier then, and that run decides whether it is a notice here (it
/// says nothing of what is on screen, which the push server cannot see).
/// The OS's list is handed over again at every return to the front: one
/// already waiting waits once.
fn os_notice(notice: crate::os::OsNotice, cx: &mut App) {
    let session = cx.global::<Phone>().shared.session.read(cx).get();
    if notice.front && session.connected && session.chain == notice.chain {
        return;
    }
    let waiting = &mut cx.global_mut::<Phone>().notices;
    if !waiting.contains(&notice) {
        waiting.push(notice);
    }
    log_os_notices(cx);
}

/// The OS's copies of each of `entries` out of its list, as its row is read
/// or cleared here: a notice the push server carried stands there under its
/// banner tag (`<program>/<tag>`), and the next read of the list
/// ([`os_notice`]) would log it again. The OS's list then says what the tab
/// says. A row with no tag has no name there to take it down by.
fn withdraw_shown<'a>(entries: impl IntoIterator<Item = &'a Entry>) {
    for entry in entries {
        if !entry.tag.is_empty() && !entry.host() {
            crate::os::withdraw_notice(&format!("{}/{}", entry.program, entry.tag));
        }
    }
}

/// A chain taken up opens its log: the OS's notices waiting for it go in.
/// The chain in hand is published for the OS's own threads
/// ([`crate::os::hold_chain`]) now and on each move of the session.
fn log_on_connect(shared: &Shared, cx: &mut App) {
    hold_chain(&shared.session, cx);
    cx.observe(&shared.session, |session, cx| {
        hold_chain(&session, cx);
        log_os_notices(cx);
    })
    .detach();
}

/// The chain `session` is connected to, or none, published.
fn hold_chain(session: &Entity<Session>, cx: &App) {
    let session = session.read(cx).get();
    crate::os::hold_chain(match session.connected {
        true => &session.chain,
        false => "",
    });
}

/// The OS's notices waiting for the chain in hand, logged.
fn log_os_notices(cx: &mut App) {
    let phone = cx.global::<Phone>();
    let session = phone.shared.session.read(cx).get();
    if !session.connected || phone.notices.is_empty() {
        return;
    }
    let chain = session.chain.clone();
    let notifications = phone.shared.notifications.clone();
    let waiting = std::mem::take(&mut cx.global_mut::<Phone>().notices);
    let (here, elsewhere) = waiting
        .into_iter()
        .partition::<Vec<_>, _>(|notice| notice.chain == chain);
    cx.global_mut::<Phone>().notices = elsewhere;
    notifications.update(cx, |notifications, cx| {
        for notice in here {
            notifications.carried(&notice.notice, notice.at, notice.opened, cx);
        }
    });
}

/// What a banner's click names, from the host's notification centre: a
/// link opens as any other; the phone's app store updates the app, so
/// there is no Updates page.
fn banner_opened(opened: Banner) {
    match opened {
        Banner::Link(link) => crate::os::link(link),
        // a phone's banners are all the chain in hand's (`os::PhoneBanners`)
        Banner::Row { .. } | Banner::Updates => {}
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
        Signal::Swipe { x, speed } => {
            if let Some(root) = root(cx) {
                root.update(cx, |root, cx| root.drag(x, speed, cx));
            }
        }
        // through the window: a swipe landing off Home blurs its filter
        Signal::SwipeLetGo => {
            let window = cx
                .try_global::<Phone>()
                .and_then(|phone| phone.window.clone());
            if let Some((handle, root)) = window
                && let Some(root) = root.upgrade()
            {
                // the window is gone only as the app quits: nothing to let go
                handle
                    .update(cx, |_, window, cx| {
                        root.update(cx, |root, cx| root.let_go(window, cx))
                    })
                    .ok();
            }
        }
        Signal::SwipeCancelled => {
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

/// `appearance` (the Account tab's Appearance) onto the app's iOS windows,
/// which UIKit's keyboard and alerts follow (`os::interface_style`), and
/// onto the kit theme, which the phone's palette (`look::palette`), every
/// view's theme and the host's own chrome read; under System, the window's
/// own; then the system bars over it. Called as the window opens, when the
/// choice moves and when the system's does.
fn sync_appearance(appearance: Appearance, window: &mut Window, cx: &mut App) {
    use gpui_kit::component::{Theme, ThemeMode};
    crate::os::interface_style(appearance);
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
        let seated = seats.clone();
        let views = move |cx: &App| seated.read(cx).seated(cx);
        let settle = move |cx: &mut App| seats.update(cx, |seats, cx| seats.settle(cx));
        let keep = |_: &Window, _: &App| None;
        let chords = Vec::new;
        let door = ax::Door {
            windows: &windows,
            settle: &settle,
            keep: &keep,
            chords: &chords,
            views: &views,
        };
        ax::serve(calls, door, cx).await;
    })
    .detach();
}

/// What the stack shows full screen: a tab root, the program on top, or
/// the shell's own page pushed over them (a program's page).
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
            Page::Root(Tab::Notifications) => "screen::notifications".into(),
            Page::Root(Tab::Account) => "screen::account".into(),
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

    /// One of the shell's own pages, pushed over the stack.
    fn pushed(self) -> bool {
        matches!(self, Page::About(_))
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

/// How far a tab's swipe settles once let go: as UIKit's pop, so it keeps
/// the finger's speed (`Curve::thrown`), on Android too, whose pager
/// (`ViewPager`) carries the fling into its settle as well.
const PAGER: motion::Motion = motion::IOS.push;

/// The tab roots moving sideways at a tab root ([`Root::drag`]).
#[derive(Clone, Copy, Debug)]
enum Paging {
    /// Under the finger, as [`Root::swipe`] says, counted from `from`: the
    /// finger's travel (a share of the width, to the right) that holds the
    /// tab on screen where the finger caught it, still settling; 0 from
    /// rest.
    Held { from: f32 },
    /// Let go: the tab on screen settling into place from where the finger
    /// left it (`tween`, its offset to the right as a share of the width,
    /// to 0), `beside` it the tab it came from, or would have gone to.
    Settling { beside: Option<Tab>, tween: Tween },
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
    /// page.
    pages: Vec<Page>,
    /// Where this phone's key is kept, as the store said for the keyring
    /// named (Expo reads it once per keyring): its words, or why the store
    /// did not say.
    storage: Option<(String, Result<KeyStorage, String>)>,
    /// The title each seat's view gave its window, by its instance.
    titles: BTreeMap<u64, String>,
    /// What `Seats` was last told: each open program's place, and the seat
    /// whose view may move the keys.
    placed: (BTreeMap<u64, Place>, Option<u64>),
    /// Links that came before the network's program list did, in order.
    waiting: Vec<String>,
    /// A link of another listed network's, its chain and the node the
    /// switch reaches, until that switch lands or ends another way.
    switching: Option<(ducklink::ChainId, String, String)>,
    /// The appearance last put on the theme (`sync_appearance`).
    appearance: Appearance,
    /// Debug mode's numbers, read while the Open tab is on screen.
    readout: debug::Readout,
    /// The programs Home's caption counts while they are being prepared.
    run: home::Run,
    /// Home's filter, and whether it is open (its magnifier pressed).
    search: Entity<InputState>,
    filtering: bool,
    /// Add a device's code, as typed (`approve::field`), and what its sheet
    /// showed as it went: drawn so while it slides off (what it found is
    /// forgotten at once, and the sheet going keeps only which it was).
    code: Entity<InputState>,
    approve_left: AccountState,
    /// Each placed seat, observed: a tree, a standin or a fault that moved
    /// draws the window again.
    watched: BTreeMap<u64, Subscription>,
    /// The stack moving, until its motion ends.
    moving: Option<Move>,
    /// How far in (points) iOS's swipe back has come while it drags the
    /// screen on top off.
    dragged: Option<f32>,
    /// The swipe under way, whoever it drags: how far in its finger is
    /// (points, rightward) and how fast it moves (points per second,
    /// rightward), as last told.
    swipe: Option<(f32, f32)>,
    /// The tabs' swipe, under the finger or settling.
    paging: Option<Paging>,
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
    /// A finger dragging the sheet down, until let go.
    sheet_drag: Option<sheet::Drag>,
    consent_height: Rc<Cell<Option<gpui_kit::Pixels>>>,
    /// Draws so far: a test tells a still window from one a timer redraws.
    #[cfg(test)]
    renders: usize,
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
            // an approval landed: Add a device goes, as the desktop's dialog
            // closes; the host's notice says so (the `Toast` entity's)
            cx.subscribe(
                &shared.account,
                |root, _, event: &AccountEvent, cx| match event {
                    AccountEvent::Approved if root.sheet == Some(Sheet::AddDevice) => {
                        root.close_sheet(cx)
                    }
                    AccountEvent::Approved | AccountEvent::Toast(_) | AccountEvent::Welcome => {}
                },
            ),
            cx.subscribe(
                &shared.session,
                |root, _, event: &SessionEvent, cx| match event {
                    SessionEvent::LeftNetwork => root.left_network(cx),
                    // a switch landed (another chain left the network first);
                    // only the host's `connect_answered` sends it today, a
                    // new sender would close Switch network too
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
        let (code, typed) = approve::field(window, cx);
        subscriptions.push(typed);
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
            switching: None,
            appearance,
            readout: debug::Readout::default(),
            run: home::Run::default(),
            search,
            filtering: false,
            code,
            approve_left: AccountState::default(),
            watched: BTreeMap::new(),
            moving: None,
            dragged: None,
            swipe: None,
            paging: None,
            swiped: None,
            width: 0.,
            drawn_sheet: None,
            drawn_consent: None,
            drawn_alert: None,
            drawn_toast: None,
            sheet_height: Rc::default(),
            sheet_drag: None,
            consent_height: Rc::default(),
            #[cfg(test)]
            renders: 0,
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

    /// `tab`'s root, still: a tabs' swipe settling is over.
    fn pick(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.paging = None;
        let now = Self::now(cx);
        self.stack.update(cx, |stack, cx| {
            stack.pick(tab, now);
            cx.notify();
        });
    }

    /// `program` on top, in its seat, pushed over the screen it was on; the
    /// seat's instance.
    fn open(&mut self, program: &'static str, cx: &mut Context<Self>) -> u64 {
        self.set_sheet(None, cx);
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
        self.set_sheet(None, cx);
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

    /// Where this phone's key is kept, read from the store for the keyring
    /// in hand when it is not the one last read (Expo reads it once per
    /// keyring); none with no keyring.
    fn read_storage(&mut self, cx: &App) {
        let keyring = self.shared.account.read(cx).keyring();
        if keyring.is_empty() {
            self.storage = None;
        } else if self
            .storage
            .as_ref()
            .is_none_or(|(read, _)| read != keyring)
        {
            let kept = ducktape_host::contract::os()
                .key_storage(keyring)
                .map_err(|refused| refused.sentence());
            self.storage = Some((keyring.to_owned(), kept));
        }
    }

    /// Switch network over the Account tab, its field holding the last node
    /// reached, the keys in it.
    fn show_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.join
            .update(cx, |join, cx| join.open_switch(window, cx));
        self.show(Sheet::Switch, cx);
    }

    /// Add a device over the Account tab (the desktop's "Add a device…"
    /// opening): what the last one found and typed gone, the keys in its
    /// field (Expo `autoFocus`).
    fn show_approve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shared
            .account
            .update(cx, |account, cx| account.approve_open(cx));
        self.code.update(cx, |code, cx| {
            code.set_value("", window, cx);
            code.focus(window, cx);
        });
        self.show(Sheet::AddDevice, cx);
    }

    /// A switch landed (another node of the chain in hand, the node in hand
    /// itself, or another chain, which left the network first): Switch
    /// network goes, before the screen goes where the host sends it (with
    /// no account there, the join, whole). A switch a link of another
    /// network's made: the link opens here, once the network lists its
    /// programs; a node that answered for another chain says whose link it
    /// is, as before. Another node landed (a switch of the reader's own
    /// took over): the link is let go, unsaid.
    fn switched(&mut self, cx: &mut Context<Self>) {
        if self.sheet == Some(Sheet::Switch) {
            self.close_sheet(cx);
        }
        let Some((chain, link, node)) = self.switching.take() else {
            return;
        };
        let session = self.shared.session.read(cx).get();
        if session.connected_rpc != node {
            return;
        }
        match session.chain == chain.to_string() {
            true => self.take_link(link, cx),
            false => self.said_elsewhere(&chain.label, cx),
        }
    }

    /// The switch did not land: while Switch network is up its words go
    /// under the field (Expo 75), not over the screen in the notice the
    /// host put up. A link that waited on it is dropped; the notice says
    /// why.
    fn switch_failed(&mut self, said: String, cx: &mut Context<Self>) {
        self.switching = None;
        if self.sheet != Some(Sheet::Switch) {
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
    /// it is. A pop the finger let go of goes on from where it let go, at
    /// the finger's speed. Tab switches do not move.
    fn moved(&mut self, from: Page, pop: bool, cx: &mut Context<Self>) {
        let dragged = self.dragged.take();
        let to = self.page(cx);
        if from == to || matches!((from, to), (Page::Root(_), Page::Root(_))) {
            return;
        }
        let (motion, slide) = motion::stack(pop, cx);
        let speed = self.swipe.map_or(0., |(_, speed)| speed);
        let (start, velocity) = match (dragged, self.moving) {
            (Some(x), _) if pop && self.width > 0. => {
                ((x / self.width).clamp(0., 1.), speed / self.width)
            }
            (None, Some(m)) if m.left == to && m.pop != pop => {
                (1. - m.tween.at(motion::now(cx)), 0.)
            }
            _ => (0., 0.),
        };
        self.moving = Some(Move {
            left: from,
            pop,
            slide,
            tween: Tween::thrown(start, 1., velocity, motion, cx),
        });
        cx.notify();
    }

    /// iOS's swipe back, `x` points in, moving at `speed` points per second:
    /// the screen on top follows the finger while a Back would pop it (a
    /// program's page, or a program on top whose view does not hear Back
    /// itself) and nothing is over it.
    /// When the view on top hears Back (a page above its root), its own
    /// page follows inside it instead, if its tree keeps the page under
    /// (`ViewTree::drag_page`), and that tree hears the release, whatever is
    /// on top by then. Under Reduce Motion nothing follows; the
    /// release is Back as before. A swipe begun while the stack moves does
    /// not follow either. At a tab root the swipe pages the tabs instead,
    /// either way, until let go; tabs still settling are caught where they
    /// are, as a pager's page is.
    fn drag(&mut self, x: f32, speed: f32, cx: &mut Context<Self>) {
        let paging = matches!(self.paging, Some(Paging::Held { .. }));
        if paging || (self.dragged.is_none() && self.swiped.is_none() && self.can_page(cx)) {
            if !paging {
                let from = self.caught(cx);
                self.paging = Some(Paging::Held { from });
            }
            self.swipe = Some((x, speed));
            return cx.notify();
        }
        // a swipe back goes only to the right
        let x = x.max(0.);
        self.swipe = Some((x, speed));
        let progress = x / self.width.max(1.);
        let shares = speed / self.width.max(1.);
        // the view's page the finger holds stays its, while it follows
        if let Some(tree) = self.swiped.as_ref().and_then(WeakEntity::upgrade) {
            if tree.update(cx, |tree, cx| tree.drag_page(progress, shares, cx)) {
                return;
            }
            self.swiped = None;
        }
        let busy = self.sliding(cx);
        if self.dragged.is_none() && (busy || !self.can_drag(cx)) {
            if !busy
                && let Some(tree) = self.view_swiped(cx)
                && tree.update(cx, |tree, cx| tree.drag_page(progress, shares, cx))
            {
                self.swiped = Some(tree.downgrade());
            }
            return;
        }
        // a move that has ended may not be cleared yet: it is over
        self.moving = None;
        self.dragged = Some(x);
        cx.notify();
    }

    /// The swipe let go: Back, or short of it ([`swipe::crosses`]) the
    /// screen on top springs back; paging the tabs, [`Root::paged_let_go`],
    /// and landed off Home, Home's filter lets the keys go, as a tab picked
    /// in the bar does.
    fn let_go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.paging, Some(Paging::Held { .. })) {
            let was = self.stack.read(cx).tab();
            self.paged_let_go(true, cx);
            if was == Tab::Home && self.stack.read(cx).tab() != Tab::Home {
                window.blur(cx);
            }
            return;
        }
        let Some((x, speed)) = self.swipe else {
            return;
        };
        match swipe::crosses(x, speed, self.width) {
            true => self.back(cx),
            false => self.drag_cancelled(cx),
        }
    }

    /// The tabs' swipe let go (`goes`; else ended short of its release):
    /// past the rule ([`swipe::crosses`]) toward a tab beside, that tab is
    /// chosen and slides the rest of the way in at the finger's speed;
    /// short of it, the tab on screen slides back into place, and at the
    /// bar's ends, back from the rubber band. Only a tab root still on
    /// screen goes on: a program pushed over it meanwhile stays.
    fn paged_let_go(&mut self, goes: bool, cx: &mut Context<Self>) {
        let along = self.along().unwrap_or_default();
        let speed = self.swipe.take().map_or(0., |(_, speed)| speed);
        let tab = self.stack.read(cx).tab();
        let (at, beside) = self.held_at(along, cx);
        let width = self.width.max(1.);
        let goes = goes && matches!(self.page(cx), Page::Root(_));
        // the way the screens went
        let way = if along < 0. { -1. } else { 1. };
        let (beside, from, velocity) = match beside {
            Some(next) if goes && swipe::crosses(along * width * way, speed * way, width) => {
                self.pick(next, cx);
                (Some(tab), at - way, speed / width)
            }
            Some(_) => (beside, at, speed / width),
            None => (None, at, 0.),
        };
        let tween = Tween::thrown(from, 0., velocity, PAGER, cx);
        self.paging = Some(Paging::Settling { beside, tween });
        cx.notify();
    }

    /// The finger's travel holding the tabs, a share of the width to the
    /// right, from where it caught them.
    fn along(&self) -> Option<f32> {
        let Some(Paging::Held { from }) = self.paging else {
            return None;
        };
        Some(from + self.swipe?.0 / self.width.max(1.))
    }

    /// Where a finger's travel `along` (a share of the width, to the right)
    /// has the tab on screen, its share of the width to the right, and the
    /// tab beside it that comes in from the other side: past the bar's
    /// ends none, and the screen resists.
    fn held_at(&self, along: f32, cx: &App) -> (f32, Option<Tab>) {
        let width = self.width.max(1.);
        match self.stack.read(cx).tab().beside(along < 0.) {
            Some(beside) => (along.clamp(-1., 1.), Some(beside)),
            None => (swipe::resisted(along * width, width) / width, None),
        }
    }

    /// The finger's travel that holds the tabs where they are now: where
    /// a tab settling is (out of its rubber band, past the bar's end), else
    /// 0.
    fn caught(&self, cx: &App) -> f32 {
        let Some(Paging::Settling { beside, tween }) = self.paging else {
            return 0.;
        };
        let at = tween.at(motion::now(cx));
        let width = self.width.max(1.);
        match beside {
            Some(_) => at,
            None => swipe::unresisted(at * width, width) / width,
        }
    }

    /// Where the tabs' swipe has the tab on screen, and the tab beside it,
    /// this frame ([`Root::held_at`]); the next frame asked for while they
    /// settle.
    fn paged(&self, window: &Window, cx: &App) -> Option<(f32, Option<Tab>)> {
        match self.paging? {
            Paging::Held { .. } => Some(self.held_at(self.along()?, cx)),
            Paging::Settling { beside, tween } => Some((tween.frame(window, cx), beside)),
        }
    }

    /// Whether the stack still slides.
    fn sliding(&self, cx: &App) -> bool {
        let now = motion::now(cx);
        self.moving.is_some_and(|moving| !moving.tween.done(now))
    }

    /// Whether a swipe now pages the tabs: a tab root on screen, nothing
    /// over it, the stack still (tabs settling are caught). Under Reduce
    /// Motion nothing follows a finger, so none does.
    fn can_page(&self, cx: &App) -> bool {
        matches!(self.page(cx), Page::Root(_))
            && !self.covered(cx)
            && !motion::reduced(cx)
            && !self.sliding(cx)
    }

    /// Whether a swipe back beginning now from anywhere on the screen
    /// drags a screen off one under it ([`swipe`]): nothing of the stack
    /// moves, no swipe is under way, and the screen on top follows a drag,
    /// or the view's page on top does inside it.
    fn swipe_opens(&self, cx: &App) -> bool {
        if self.sliding(cx) || self.swipe.is_some() {
            return false;
        }
        self.can_drag(cx)
            || self
                .view_swiped(cx)
                .is_some_and(|tree| tree.read(cx).can_drag_page(cx))
    }

    /// What this draw shows a swipe back's start ([`swipe::publish`]), read
    /// by a mark laid out last, not at this render: the view on top draws
    /// after it (a cached view renders in its prepaint), and a page motion
    /// of its that ends in that draw asks for no frame after it, so read
    /// here it would publish a swipe shut that no frame comes to open. By
    /// the mark, the view has drawn, and its tree knows what takes a
    /// sideways drag in it. The shell's own pages take none.
    fn swipe_shown(cx: &Context<Self>) -> impl IntoElement + use<> {
        let root = cx.weak_entity();
        canvas(
            move |_, _, cx| {
                let shown = root.upgrade().map(|root| {
                    let root = root.read(cx);
                    let tree = match root.pages.is_empty() {
                        true => root.top_seat(cx).and_then(|seat| seat.read(cx).tree()),
                        false => None,
                    };
                    swipe::Shown {
                        open: root.swipe_opens(cx),
                        tabs: root.swipe.is_none() && root.can_page(cx),
                        claimed: tree.map_or_else(Vec::new, |tree| tree.read(cx).sideways()),
                    }
                });
                swipe::publish(shown.unwrap_or_default());
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_0()
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
    /// place over the one under it, or the view's page inside it, at the
    /// finger's speed.
    fn drag_cancelled(&mut self, cx: &mut Context<Self>) {
        if matches!(self.paging, Some(Paging::Held { .. })) {
            return self.paged_let_go(false, cx);
        }
        let speed = self.swipe.take().map_or(0., |(_, speed)| speed);
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
        // back into place is the other way to the finger's
        let velocity = -speed / self.width.max(1.);
        self.moving = Some(Move {
            left: under,
            pop: false,
            slide,
            tween: Tween::thrown(start, 1., velocity, motion, cx),
        });
    }

    /// `program`'s own link (`duck://<chain>/<program>`) on the clipboard.
    fn copy_link(&mut self, program: &'static str, cx: &mut Context<Self>) {
        let chain = self.shared.session.read(cx).get().chain.clone();
        if let Some(link) = ducklink::mint(&chain, program, &[]) {
            cx.write_to_clipboard(ClipboardItem::new_string(link));
            self.notice("Copied".into(), cx);
        }
        self.set_sheet(None, cx);
    }

    /// `tab`'s root, whatever is over it popped off (the program left
    /// where it is, the shell's pages and the sheet gone): the capsule's ×
    /// to Home, a view's avatar to Account.
    fn go_to(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let now = Self::now(cx);
        let from = self.page(cx);
        self.paging = None;
        self.set_sheet(None, cx);
        self.pages.clear();
        self.stack.update(cx, |stack, cx| {
            stack.pick(tab, now);
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
        self.set_sheet(Some(sheet), cx);
    }

    fn capsule_menu(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.show(Sheet::Capsule(program), cx);
    }

    /// The sheet goes (the scrim, Back, a drag down, Cancel).
    fn close_sheet(&mut self, cx: &mut Context<Self>) {
        self.set_sheet(None, cx);
    }

    /// `sheet` over the screen now, or none: every change of the sheet
    /// comes through here. Add a device going, by any way (the scrim, Back,
    /// a drag down, Cancel, a program or a tab's root opened over it, its
    /// approval, the network left), forgets what it found and drops its
    /// call, as the desktop's dialog closing does; what it showed is kept
    /// to draw it going. A finger on the sheet lets go of it.
    fn set_sheet(&mut self, sheet: Option<Sheet>, cx: &mut Context<Self>) {
        if self.sheet == sheet {
            return;
        }
        if self.sheet == Some(Sheet::AddDevice) {
            self.approve_left = self.shared.account.read(cx).get().clone();
            self.shared
                .account
                .update(cx, |account, cx| account.approve_closed(cx));
        }
        self.sheet = sheet;
        self.sheet_drag = None;
        cx.notify();
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
    /// finger that the view does not hear it for slides back, and the
    /// tabs' swipe, held or settling, is over.
    fn back(&mut self, cx: &mut Context<Self>) {
        self.paging = None;
        let swiped = self.swiped.take().and_then(|tree| tree.upgrade());
        self.walk_back(cx);
        if let Some(tree) = swiped {
            tree.update(cx, |tree, cx| tree.drag_page_cancelled(cx));
        }
        self.swipe = None;
        if self.dragged.take().is_some() {
            cx.notify();
        }
    }

    fn walk_back(&mut self, cx: &mut Context<Self>) {
        match join::step(*self.shared.screen.read(cx).get()) {
            Some(join::Step::Node) => return crate::os::back_at_root(),
            // iOS's Back is only the swipe, which the account step refuses
            // as Expo's code does (`gestureEnabled: false`): its way out is
            // its own Back button
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
        self.paging = None;
        self.set_sheet(None, cx);
        self.alert = None;
        self.titles.clear();
        self.pages.clear();
        self.waiting.clear();
        // left for no node the link's switch reached (a disconnect, or a
        // switch of the reader's own): that switch is over
        let here = &self.shared.session.read(cx).get().connected_rpc;
        self.switching.take_if(|(_, _, node)| node != here);
        cx.notify();
    }

    /// The network listed its programs, or one's code moved on: Home's
    /// count follows and Home redraws, and the links kept until then open
    /// now, in the order they came.
    fn rail_moved(&mut self, cx: &mut Context<Self>) {
        self.run.follow(self.shared.rail.read(cx).rows());
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

    /// A row of the Notifications tab picked: read now, and what it is
    /// about opened as a tapped system notice opens it (its link; with none,
    /// its program, as `notify::clicked` has it), through the same routing.
    fn open_notice(&mut self, id: u64, cx: &mut Context<Self>) {
        let notifications = self.shared.notifications.clone();
        let Some(entry) = notifications.update(cx, |notifications, cx| notifications.open(id, cx))
        else {
            return;
        };
        withdraw_shown([&entry]);
        let link = match entry.link.is_empty() {
            true => format!("duck://{}", entry.program),
            false => entry.link,
        };
        self.take_link(link, cx);
    }

    /// "Mark all read" in the Notifications tab: every row read, and the
    /// OS's copy of each unread one out of its list ([`withdraw_shown`]).
    fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let notifications = self.shared.notifications.clone();
        let entries = notifications.read(cx).entries();
        withdraw_shown(entries.iter().filter(|entry| !entry.read));
        notifications.update(cx, |notifications, cx| notifications.mark_all_read(cx));
    }

    /// "Clear read" in the Notifications tab: the read rows gone, and the
    /// OS's copy of each out of its list too ([`withdraw_shown`]).
    fn clear_read(&mut self, cx: &mut Context<Self>) {
        let notifications = self.shared.notifications.clone();
        let entries = notifications.read(cx).entries();
        withdraw_shown(entries.iter().filter(|entry| entry.read));
        notifications.update(cx, |notifications, cx| notifications.clear_read(cx));
    }

    /// A link, read as every shell reads it (`Rail::opening`): this chain's
    /// program comes on top and its view is handed the route. Another
    /// chain's switches to the listed node on that chain and opens there
    /// once the switch lands; with none listed it is said and not opened,
    /// as the Expo app says it.
    fn open_link(&mut self, link: &str, cx: &mut Context<Self>) {
        let chain = self.shared.session.read(cx).get().chain.clone();
        if let Some((other, node)) = self.elsewhere(link, &chain, cx) {
            self.shared
                .session
                .update(cx, |session, cx| session.switch(node.clone(), cx));
            // no switch under way (that node is the one in hand: what the
            // registrar heard of it is stale): said as a link of a network
            // not listed is
            if self.shared.session.read(cx).get().reaching == node {
                self.switching = Some((other, link.to_owned(), node));
                return;
            }
        }
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
            Opening::Elsewhere { chain, .. } => self.said_elsewhere(&chain, cx),
            Opening::Browser(url) => cx.open_url(&url),
            Opening::Nothing(said) => self.notice(said, cx),
        }
    }

    /// The chain `link` names when it is a chain link of another chain
    /// than `here`, and the listed node on that chain to switch to
    /// (`push::Registrar::node_of`).
    fn elsewhere(&self, link: &str, here: &str, cx: &App) -> Option<(ducklink::ChainId, String)> {
        let ducktape_host::runtime::Link::Chain(parsed) =
            self.shared.rail.read(cx).parse_link(link)
        else {
            return None;
        };
        if parsed.chain.to_string() == here {
            return None;
        }
        let registrar = &cx.try_global::<Phone>()?.registrar;
        let node = registrar.read(cx).node_of(&parsed.chain.to_string(), cx)?;
        Some((parsed.chain, node))
    }

    /// Another network's link, with no listed node on its chain: Expo's
    /// `Alert` (mobile `src/kit/navigate.ts`), its words.
    fn said_elsewhere(&mut self, chain: &str, cx: &mut Context<Self>) {
        let network = self.shared.session.read(cx).get().network.clone();
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
            SeatsEvent::OpenLink(_, _, link) | SeatsEvent::OpenWindow(_, _, link) => {
                self.open_link(link, cx)
            }
            // the reader's account: the Account tab, as from a header's avatar
            SeatsEvent::Show(_, _, view_wire::methods::Place::Account) => {
                self.go_to(Tab::Account, cx)
            }
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
            SeatsEvent::Showing(..) => {}
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
    /// its content. Add a device going draws what it showed as it went.
    fn sheet(
        &mut self,
        sheet: Sheet,
        at: sheet::At,
        window: &Window,
        palette: look::Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (name, content): (&'static str, AnyElement) = match sheet {
            Sheet::Capsule(program) => ("More", self.capsule_sheet(program, palette, cx)?),
            Sheet::Notifications(program) => (
                "Notifications",
                self.notifications_sheet(program, palette, cx),
            ),
            Sheet::AddDevice => {
                let shown = match at.leaving {
                    true => self.approve_left.clone(),
                    false => self.shared.account.read(cx).get().clone(),
                };
                let focused = self.code.read(cx).focus_handle(cx).is_focused(window);
                let code = &self.code;
                let content = approve::content(&self.shared, &shown, code, focused, palette, cx);
                ("Add a device", content.into_any_element())
            }
            // the address screen's own scroller shrinks under the room
            Sheet::Switch => (
                "Switch network",
                div()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(sheet::title("Switch network", palette))
                    .child(self.join.clone())
                    .into_any_element(),
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
            .on_click(
                cx.listener(move |root, _, _, cx| root.show(Sheet::Notifications(program), cx)),
            ),
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
    /// (the desktop's Allow, Silent, Block): with none said, the phone's
    /// (`Settings::unasked`).
    fn notifications_sheet(
        &mut self,
        program: &'static str,
        palette: look::Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = home::row_of(&self.shared, program, cx)
            .map_or_else(|| program.to_owned(), |row| home::name_of(&row));
        let notify = &self.shared.prefs.read(cx).get().notify;
        let now = notify.views.get(program).copied().or(notify.unasked);
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
                    root.close_sheet(cx);
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
                let mut screen = self.tab_screen(tab, window, palette, cx);
                // the tabs' swipe: the tab on screen `at` its share of the
                // width to the right, the tab beside it next to it, the bar
                // still under both
                if let Some((at, beside)) = self.paged(window, cx) {
                    let slot = |screen: gpui_kit::Div, at: f32| {
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(gpui_kit::relative(at))
                            .w_full()
                            .flex()
                            .flex_col()
                            .child(screen)
                    };
                    let beside = beside.map(|beside| {
                        let other = self.tab_screen(beside, window, palette, cx);
                        // taking no press, as the page a stack's move left
                        slot(other, at - at.signum()).child(div().absolute().inset_0().occlude())
                    });
                    screen = div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .overflow_hidden()
                        .child(slot(screen, at))
                        .children(beside);
                }
                let unread = self.shared.notifications.read(cx).unread();
                let bar = home::tab_bar(tab, unread, edges.bottom, palette, cx);
                let body = safe().flex().flex_col().child(screen).child(bar);
                let bottom = edges.bottom + px(look::TAB_BAR + look::gap::LG);
                (layer.child(body), bottom)
            }
        }
    }

    /// `tab`'s root screen, over the tab bar.
    fn tab_screen(
        &mut self,
        tab: Tab,
        window: &Window,
        palette: look::Palette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        match tab {
            Tab::Home => {
                let searching = self.search.read(cx).focus_handle(cx).is_focused(window);
                home::home(
                    &self.shared,
                    &self.run,
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
            Tab::Notifications => home::notifications(&self.shared, palette, cx),
            Tab::Account => {
                self.read_storage(cx);
                let storage = self.storage.as_ref().map(|(_, kept)| kept);
                account::tab(&self.shared, storage, palette, cx)
            }
        }
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
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
        if let Some(Paging::Settling { tween, .. }) = self.paging
            && tween.done(now)
        {
            self.paging = None;
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
            swipe::publish(swipe::Shown::default());
            // nothing of the network left stays over the join, nor comes
            // back sliding off the next one
            self.drawn_sheet = None;
            self.set_sheet(None, cx);
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
        let at = |(progress, slides, leaving), height: &Rc<Cell<_>>, drags| sheet::At {
            bottom: edges.bottom,
            room,
            progress,
            slides,
            leaving,
            height: height.clone(),
            drags,
        };
        if self.drawn_sheet.is_none() {
            self.sheet_height.set(None);
        }
        if let Some(shown) = motion::follow(&mut self.drawn_sheet, self.sheet, sheets, window, cx) {
            let mut at = at(shown.at(), &self.sheet_height, true);
            // under the finger, where it has it
            if let Some(held) = self.sheet_held(cx).filter(|_| !shown.leaving) {
                at.progress = held;
            }
            match self.sheet(shown.what, at, window, palette, cx) {
                Some(sheet) => root = root.child(sheet),
                // what a sheet was over is gone (a program no longer
                // listed): `place` withheld the keys for it this draw, so
                // the window draws again
                None => {
                    self.drawn_sheet = None;
                    self.set_sheet(None, cx);
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
            let at = at(shown.at(), &self.consent_height, false);
            root = root.child(self.consent_sheet(shown.what, at, palette, cx));
        }
        root.child(Self::swipe_shown(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ducktape_host::runtime::Roster;
    use ducktape_host::session::Screen;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext, point};

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
            log_on_connect(&shared, cx);
            cx.set_global(Phone {
                shared: shared.clone(),
                stack: stack.clone(),
                window: None,
                waiting: Vec::new(),
                notices: Vec::new(),
                registrar,
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

    /// A swipe let go, through the window as the OS's is.
    fn let_go(root: &Entity<Root>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| root.update(cx, |root, cx| root.let_go(window, cx)));
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
        root.update(cx, |root, cx| root.go_to(Tab::Home, cx));
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
        root.update(cx, |root, cx| root.drag(40., 0., cx));
        assert!(
            root.read_with(cx, |root, _| root.dragged.is_none()),
            "a drag followed under Reduce Motion"
        );
        run_out(cx);
        cx.update(|_, cx| cx.set_reduce_motion(false));
        cx.run_until_parked();

        let width = root.read_with(cx, |root, _| root.width);
        assert!(width > 0.);
        root.update(cx, |root, cx| root.drag(width / 4., 0., cx));
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

        root.update(cx, |root, cx| root.drag(width / 2., 0., cx));
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
        root.update(cx, |root, cx| root.drag(10., 0., cx));
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
        root.update(cx, |root, cx| root.show(Sheet::Notifications(PROGRAM), cx));
        cx.run_until_parked();
        let up = Sheet::Notifications(PROGRAM);
        assert_eq!(sheet(cx), Some((up, false)));
        assert!(frames(cx) > 0, "a sheet coming asked for no frame");
        run_out(cx);
        assert_eq!(sheet(cx), Some((up, false)));
        assert_eq!(frames(cx), 0, "a sheet up asked for a frame");

        root.update(cx, |root, cx| root.close_sheet(cx));
        cx.run_until_parked();
        assert_eq!(sheet(cx), Some((up, true)), "a sheet went at once");
        assert!(frames(cx) > 0);
        run_out(cx);
        assert_eq!(sheet(cx), None, "a sheet gone is still drawn");
        assert_eq!(frames(cx), 0, "a frame asked for after the sheet went");

        // Back while a sheet still comes up turns it back from where it is
        root.update(cx, |root, cx| root.show(Sheet::Notifications(PROGRAM), cx));
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

    /// The account this phone holds, seeded, with what Add a device found:
    /// the joining key's fingerprint (`approve`), and the request itself
    /// when `found`.
    fn seed_found(shared: &Shared, approve: Option<&str>, found: bool, cx: &mut VisualTestContext) {
        let key = vec![7; 32];
        let state = ducktape_host::session::AccountState {
            account: Some(Some((9, "ada".into()))),
            approve: approve.map(str::to_owned),
            error: "no".into(),
            ..Default::default()
        };
        let request = found.then(|| noded_client::join::Request {
            network: "testkit".into(),
            key,
        });
        shared
            .account
            .update(cx, |account, cx| account.seed(state, None, request, cx));
    }

    fn approve_state(
        shared: &Shared,
        cx: &mut VisualTestContext,
    ) -> ducktape_host::session::AccountState {
        shared
            .account
            .read_with(cx, |account, _| account.get().clone())
    }

    /// Add a device over the Account tab, as its row opens it.
    fn show_approve(root: &Entity<Root>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| root.update(cx, |root, cx| root.show_approve(window, cx)));
        run_out(cx);
    }

    /// Switch network over the Account tab, as its row opens it.
    fn show_switch(root: &Entity<Root>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| root.update(cx, |root, cx| root.show_switch(window, cx)));
        run_out(cx);
    }

    /// A press in the middle of what `id` names, and its motion run out.
    fn click(id: &'static str, cx: &mut VisualTestContext) {
        let at = cx.debug_bounds(id).expect(id);
        cx.simulate_click(at.center(), gpui_kit::Modifiers::none());
        run_out(cx);
    }

    /// The sheet up, and whether it is going.
    fn sheet_up(root: &Entity<Root>, cx: &mut VisualTestContext) -> (Option<Sheet>, bool) {
        root.read_with(cx, |root, _| {
            let going = root.drawn_sheet.as_ref().is_some_and(|shown| shown.leaving);
            (root.sheet, going)
        })
    }

    /// A finger's pan on the sheet as gpui hands it on: every step where it
    /// began (10 under the panel's top edge), the first past the touch
    /// slop, each drawn and `gap` apart, then its release.
    fn pan(steps: &[f32], gap: std::time::Duration, cx: &mut VisualTestContext) {
        use gpui_kit::{ScrollDelta, ScrollWheelEvent, TouchPhase};
        let panel = cx.debug_bounds("sheet").expect("a sheet up");
        let at = point(panel.center().x, panel.top() + px(10.));
        let event = |step: f32, touch_phase| ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.), px(step))),
            modifiers: gpui_kit::Modifiers::none(),
            touch_phase,
        };
        for (at, &step) in steps.iter().enumerate() {
            let phase = match at {
                0 => TouchPhase::Started,
                _ => TouchPhase::Moved,
            };
            cx.simulate_event(event(step, phase));
            frames(cx);
            cx.executor().advance_clock(gap);
        }
        cx.simulate_event(event(0., TouchPhase::Ended));
        cx.run_until_parked();
    }

    /// The Account tab is one page: picked, it is the root on screen with
    /// nothing pushed over it, its sections drawn under the big title down
    /// to the version at the foot; Add a device only while this phone holds
    /// an account. Add a device opens its sheet over the tab with nothing
    /// the last one found, Switch network its own; Back closes each, back
    /// to the tab. A view asking for the reader's account (a header's
    /// avatar) picks the tab too, the program left open where it was.
    #[gpui_kit::test]
    fn the_account_tab_is_one_page_and_its_two_typing_rows_open_sheets(cx: &mut TestAppContext) {
        let (root, stack, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        let shown = |cx: &mut VisualTestContext| {
            root.read_with(cx, |root, cx| (root.sheet, root.page(cx), root.pages.len()))
        };
        let account = (None, Page::Root(Tab::Account), 0);
        click("tab/account", cx);
        assert_eq!(shown(cx), account, "the Account tab");
        for id in [
            "account/phone",
            "account/key-storage",
            "account/network",
            "account/switch",
            "appearance/system",
            "appearance/light",
            "appearance/dark",
            "account/debug",
            "account/version",
        ] {
            assert!(cx.debug_bounds(id).is_some(), "{id} not on the tab");
        }
        assert!(
            cx.debug_bounds("account/add-device").is_none(),
            "Add a device with no account to approve for"
        );
        seed_found(&shared, Some("A1F3 09C2"), false, cx);
        run_out(cx);
        for (row, sheet) in [
            ("account/add-device", Sheet::AddDevice),
            ("account/switch", Sheet::Switch),
        ] {
            click(row, cx);
            assert_eq!(
                shown(cx),
                (Some(sheet), Page::Root(Tab::Account), 0),
                "{row}"
            );
            if sheet == Sheet::AddDevice {
                let state = approve_state(&shared, cx);
                assert_eq!(
                    (state.approve, state.error.as_str()),
                    (None, ""),
                    "Add a device opened on the last one's find"
                );
            }
            root.update(cx, |root, cx| root.back(cx));
            run_out(cx);
            assert_eq!(shown(cx), account, "Back from {row}");
        }

        click("tab/home", cx);
        root.update(cx, |root, cx| _ = root.open(PROGRAM, cx));
        run_out(cx);
        let asked = SeatsEvent::Show(PROGRAM, 0, view_wire::methods::Place::Account);
        root.update(cx, |root, cx| root.seats_said(&asked, cx));
        run_out(cx);
        assert_eq!(shown(cx), account, "a view's avatar");
        let open = stack.read_with(cx, |stack, _| {
            (stack.top(), stack.instance(PROGRAM).is_some())
        });
        assert_eq!(open, (None, true), "the program was not left open");
    }

    /// Add a device asks for the code, Next and Cancel under it, until the
    /// host found the joining key; then it shows that key's fingerprint
    /// and Approve.
    #[gpui_kit::test]
    fn add_a_device_shows_the_fingerprint_once_the_host_found_the_key(cx: &mut TestAppContext) {
        let (root, _, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        let drawn = |id: &'static str, cx: &mut VisualTestContext| cx.debug_bounds(id).is_some();
        show_approve(&root, cx);
        assert!(drawn("approve/next", cx) && drawn("approve/cancel", cx));
        assert!(
            !drawn("approve/fingerprint", cx),
            "a fingerprint before a find"
        );
        seed_found(&shared, Some("A1F3 09C2"), true, cx);
        run_out(cx);
        assert!(drawn("approve/fingerprint", cx) && drawn("approve/confirm", cx));
        assert!(
            !drawn("approve/next", cx),
            "the code step under the fingerprint"
        );
        assert_eq!(sheet_up(&root, cx), (Some(Sheet::AddDevice), false));
    }

    /// Add a device closed by a press on the scrim, Back, a drag down,
    /// Cancel or a program opened over it forgets what it found: Approve
    /// after it asks nothing of the host.
    #[gpui_kit::test]
    fn add_a_device_closed_any_way_forgets_what_it_found(cx: &mut TestAppContext) {
        let (root, _, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        type Leave = fn(&Entity<Root>, &mut VisualTestContext);
        let ways: [(&str, Leave); 5] = [
            ("the scrim", |_, cx| {
                let panel = cx.debug_bounds("sheet").expect("the sheet");
                let over = point(panel.center().x, panel.top() - px(40.));
                cx.simulate_click(over, gpui_kit::Modifiers::none());
            }),
            ("Back", |root, cx| root.update(cx, |root, cx| root.back(cx))),
            ("a drag down", |_, cx| {
                pan(&[40., 300.], std::time::Duration::from_millis(16), cx)
            }),
            ("Cancel", |_, cx| click("approve/cancel", cx)),
            ("a program opened", |root, cx| {
                root.update(cx, |root, cx| _ = root.open(PROGRAM, cx))
            }),
        ];
        for (way, leave) in ways {
            show_approve(&root, cx);
            seed_found(&shared, Some("A1F3 09C2"), true, cx);
            run_out(cx);
            leave(&root, cx);
            run_out(cx);
            assert_eq!(
                sheet_up(&root, cx),
                (None, false),
                "{way} left the sheet up"
            );
            shared
                .account
                .update(cx, |account, cx| account.approve_confirm(cx));
            let state = approve_state(&shared, cx);
            assert_eq!(state.approve, None, "{way} kept the fingerprint");
            assert!(!state.busy, "{way} kept the request: Approve signed it");
        }
    }

    /// An approval landing closes Add a device, drawn as it was while it
    /// goes, and the host's notice says so; one landing while Switch
    /// network is up leaves it.
    #[gpui_kit::test]
    fn an_approval_closes_add_a_device_and_says_so(cx: &mut TestAppContext) {
        let (root, _, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        show_approve(&root, cx);
        seed_found(&shared, Some("A1F3 09C2"), true, cx);
        run_out(cx);
        shared
            .account
            .update(cx, |account, cx| account.approve_done(Ok(()), cx));
        frames(cx);
        assert_eq!(sheet_up(&root, cx), (None, true), "the approval left it up");
        assert!(
            cx.debug_bounds("approve/fingerprint").is_some(),
            "the sheet went without what it showed"
        );
        run_out(cx);
        assert_eq!(sheet_up(&root, cx), (None, false));
        let notice = shared.toast.read_with(cx, |toast, _| toast.get().clone());
        assert_eq!(notice, "Approved. The new device finishes on its own.");
        show_switch(&root, cx);
        shared
            .account
            .update(cx, |account, cx| account.approve_done(Ok(()), cx));
        run_out(cx);
        assert_eq!(
            sheet_up(&root, cx),
            (Some(Sheet::Switch), false),
            "an approval closed Switch network"
        );
    }

    /// What the person does: Account tab, Add a device, a code typed into
    /// its field (the keys in it as the sheet opens) and Next; the host is
    /// asked, and its refusal shows under the field in the sheet until the
    /// next keystroke; return asks again.
    #[gpui_kit::test]
    fn a_code_typed_and_next_asks_the_host_and_its_refusal_shows_under_the_field(
        cx: &mut TestAppContext,
    ) {
        use join::tests::until;
        const REFUSED: &str = "A code is 8 letters and digits, like KQ4M-9XPT.";
        let (root, _, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        let error = |cx: &mut VisualTestContext| approve_state(&shared, cx).error;
        seed_found(&shared, None, false, cx);
        click("tab/account", cx);
        click("account/add-device", cx);
        cx.simulate_input("KQ4M");
        let typed = root.read_with(cx, |root, cx| root.code.read(cx).value().to_string());
        assert_eq!(typed, "KQ4M", "the keys were not in the field");
        click("approve/next", cx);
        until(cx, |cx| !error(cx).is_empty());
        assert_eq!(error(cx), REFUSED);
        run_out(cx);
        assert!(
            cx.debug_bounds("approve/problem").is_some(),
            "the refusal was not under the field"
        );
        assert_eq!(sheet_up(&root, cx), (Some(Sheet::AddDevice), false));
        cx.simulate_input("9");
        assert_eq!(error(cx), "", "a keystroke left the refusal up");
        cx.simulate_keystrokes("enter");
        until(cx, |cx| !error(cx).is_empty());
        assert_eq!(error(cx), REFUSED, "return asked nothing");
    }

    /// A finger drags a sheet: held, it is drawn where the finger has it;
    /// let go short and slowly it springs back up from there; let go past
    /// half its height it goes on down from there and closes, and so does
    /// a short quick flick. A pan begun upward leaves it be; under Reduce
    /// Motion nothing follows the finger, and a release that goes closes
    /// it all the same.
    #[gpui_kit::test]
    fn a_sheet_dragged_down_past_half_closes_and_a_short_drag_springs_back(
        cx: &mut TestAppContext,
    ) {
        use std::time::Duration;
        let (root, _, cx) = phone(cx);
        let up = Sheet::Notifications(PROGRAM);
        let show = |cx: &mut VisualTestContext| {
            root.update(cx, |root, cx| root.show(up, cx));
            run_out(cx);
        };
        let tween = |cx: &mut VisualTestContext| {
            root.read_with(cx, |root, _| {
                root.drawn_sheet
                    .as_ref()
                    .map(|it| (it.tween.from, it.tween.to))
            })
        };
        let slow = Duration::from_millis(200);
        show(cx);
        let height = root.read_with(cx, |root, _| root.sheet_height.get());
        let height = f32::from(height.expect("the sheet laid out"));
        let top = cx.debug_bounds("sheet").expect("the sheet").top();
        // a short slow drag: under the finger while held, then back up
        pan(&[20.], slow, cx);
        assert_eq!(
            sheet_up(&root, cx),
            (Some(up), false),
            "a short drag closed it"
        );
        let (from, to) = tween(cx).expect("the sheet");
        assert_eq!(to, 1., "it did not spring back");
        assert!(
            (from - (1. - 20. / height)).abs() < 1e-3,
            "{from}: not from the finger"
        );
        run_out(cx);
        assert_eq!(cx.debug_bounds("sheet").map(|it| it.top()), Some(top));

        let held = |cx: &mut VisualTestContext| root.read_with(cx, |root, cx| root.sheet_held(cx));
        let at = cx.debug_bounds("sheet").expect("the sheet");
        let start = point(at.center().x, at.top() + px(10.));
        let step = |step: f32, touch_phase, cx: &mut VisualTestContext| {
            cx.simulate_event(gpui_kit::ScrollWheelEvent {
                position: start,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(step))),
                modifiers: gpui_kit::Modifiers::none(),
                touch_phase,
            });
            frames(cx);
        };
        step(30., gpui_kit::TouchPhase::Started, cx);
        step(30., gpui_kit::TouchPhase::Moved, cx);
        let moved = cx.debug_bounds("sheet").expect("the sheet").top();
        assert_eq!(moved, top + px(60.), "the sheet is not under the finger");
        assert!(held(cx).is_some());
        step(0., gpui_kit::TouchPhase::Cancelled, cx);
        assert_eq!(held(cx), None);
        assert_eq!(
            sheet_up(&root, cx),
            (Some(up), false),
            "a cancelled drag closed it"
        );
        run_out(cx);

        // past half: closed, going on down from where it was left
        pan(&[height * 0.3, height * 0.3], slow, cx);
        assert_eq!(sheet_up(&root, cx), (None, true), "past half kept it");
        let (from, to) = tween(cx).expect("the sheet going");
        assert_eq!(to, 0.);
        assert!((from - 0.4).abs() < 1e-3, "{from}: not from the finger");
        run_out(cx);
        assert_eq!(sheet_up(&root, cx), (None, false));

        // a short quick flick closes it too
        show(cx);
        pan(&[10., 10.], Duration::from_millis(16), cx);
        assert_eq!(sheet_up(&root, cx), (None, true), "a flick kept it");
        run_out(cx);

        // a pan begun upward is not a drag, even come back down past half
        show(cx);
        pan(&[-40., height * 0.9], slow, cx);
        assert_eq!(
            sheet_up(&root, cx),
            (Some(up), false),
            "an upward pan closed it"
        );

        cx.update(|_, cx| cx.set_reduce_motion(true));
        step(20., gpui_kit::TouchPhase::Started, cx);
        assert_eq!(held(cx), None, "the sheet followed under Reduce Motion");
        assert_eq!(cx.debug_bounds("sheet").map(|it| it.top()), Some(top));
        step(0., gpui_kit::TouchPhase::Cancelled, cx);
        pan(&[height * 0.6], slow, cx);
        frames(cx);
        let fade = root.read_with(cx, |root, _| {
            root.drawn_sheet.as_ref().map(|it| it.tween.motion)
        });
        assert_eq!(sheet_up(&root, cx).0, None, "a release past half kept it");
        assert_eq!(
            fade,
            Some(motion::FADE),
            "it did not fade under Reduce Motion"
        );
    }

    /// The join takes the sheet with it: a switch landing on a network
    /// with no account for this phone shows the join whole, and back on a
    /// network no sheet comes sliding off the stack.
    #[gpui_kit::test]
    fn the_join_takes_the_sheet_with_it(cx: &mut TestAppContext) {
        let (root, _, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        show_switch(&root, cx);
        let step = ducktape_host::session::AccountStep::Name;
        shared.screen.update(cx, |screen, cx| {
            screen.set(Screen::Account { step }, cx);
        });
        run_out(cx);
        assert_eq!(
            sheet_up(&root, cx),
            (None, false),
            "the sheet stayed over the join"
        );
        shared.screen.update(cx, |screen, cx| {
            screen.set(Screen::Desk, cx);
        });
        frames(cx);
        assert!(cx.debug_bounds("sheet").is_none(), "a sheet came back");
    }
    /// The Notifications tab lists the host's centre, newest first, a dot on
    /// the tab while one is unread; a row pressed is read and opens its
    /// program (no link: as a tapped system notice does); Mark all read
    /// reads the rest and the dot goes; Clear read empties the list.
    #[gpui_kit::test]
    fn the_notifications_tab_lists_the_centre_and_opens_a_notice(cx: &mut TestAppContext) {
        use ducktape_host::runtime::notify::Settings;
        let _seat = ducktape_host::backend::seat_serial();
        let (root, stack, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        ducktape_host::runtime::seat_for_test(PROGRAM, 320);
        shared.rail.update(cx, |rail, cx| {
            rail.read_off(Roster::listing(&[PROGRAM]), cx)
        });
        let dot = |cx: &mut VisualTestContext| cx.debug_bounds("tab/notifications/dot").is_some();
        run_out(cx);
        assert!(!dot(cx), "a dot with nothing logged");

        let settings = Settings {
            banners: true,
            in_front: false,
            burst: 3,
            views: Default::default(),
            unasked: None,
        };
        let center = shared
            .notifications
            .read_with(cx, |it, _| it.center().clone());
        for (title, at) in [("Older", 100), ("Newer", 200)] {
            let post = view_wire::methods::Notification {
                title: title.into(),
                body: "said".into(),
                ..Default::default()
            };
            let now = std::time::Instant::now();
            let _ = center
                .lock()
                .post(&settings, PROGRAM, "Test", post, now, at);
        }
        shared.notifications.update(cx, |it, cx| _ = it.refresh(cx));
        run_out(cx);
        assert!(dot(cx), "no dot with two unread");

        let click = |id: &'static str, cx: &mut VisualTestContext| {
            let at = cx.debug_bounds(id).expect(id);
            cx.simulate_click(at.center(), gpui_kit::Modifiers::none());
            run_out(cx);
        };
        click("tab/notifications", cx);
        let ids: Vec<u64> = center.lock().entries().map(|entry| entry.id).collect();
        let [newer, older] = ids[..] else {
            panic!("{ids:?}");
        };
        let row_top = |id: u64, cx: &mut VisualTestContext| {
            let row = format!("notifications/{id}").leak();
            cx.debug_bounds(row).expect("the row drawn").top()
        };
        assert!(row_top(newer, cx) < row_top(older, cx), "not newest first");

        click(format!("notifications/{older}").leak(), cx);
        let read = |cx: &mut VisualTestContext| {
            let entries = shared.notifications.read_with(cx, |it, _| it.entries());
            entries.iter().map(|entry| entry.read).collect::<Vec<_>>()
        };
        assert_eq!(read(cx), [false, true], "the row pressed was not read");
        assert_eq!(top(&stack, cx), Some(PROGRAM), "its program did not open");

        root.update(cx, |root, cx| root.back(cx));
        run_out(cx);
        assert!(dot(cx), "the dot went with one still unread");
        click("notifications/mark-all", cx);
        assert_eq!(read(cx), [true, true]);
        assert!(!dot(cx), "the dot stayed with nothing unread");
        click("notifications/clear-read", cx);
        assert_eq!(read(cx), [] as [bool; 0], "Clear read kept a read row");
    }

    /// The Notifications tab's name to a screen reader, as last drawn.
    fn notifications_tab_name(cx: &mut VisualTestContext) -> String {
        cx.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            let tree = window.a11y_tree().expect("an accessibility tree");
            let tab = tree.nodes.iter().find(|(_, node)| {
                node.role() == gpui_kit::accesskit::Role::Tab
                    && node
                        .label()
                        .is_some_and(|name| name.starts_with("Notifications"))
            });
            tab.and_then(|(_, node)| node.label())
                .unwrap_or_default()
                .to_owned()
        })
    }

    /// A notice the push server carried, as the OS hands it over (the
    /// carrier's own payload): it waits for its chain's log, then is one
    /// row in the Notifications tab with its program's words and the OS's
    /// time, however often the OS's list is read, and the tab says it is
    /// unread to a screen reader in the desktop bell's words; opened from
    /// the OS, its row is read. One that came while the app was in front on
    /// its chain is left to this phone's own run of the notifier; another
    /// chain's waits for that chain, once however often it is handed over.
    #[gpui_kit::test]
    fn a_pushed_notice_is_one_row_in_the_notifications_tab(cx: &mut TestAppContext) {
        use ducktape_host::session::SessionState;
        let _seat = ducktape_host::backend::seat_serial();
        let (root, _, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        ducktape_host::runtime::seat_for_test(PROGRAM, 320);
        shared.rail.update(cx, |rail, cx| {
            rail.read_off(Roster::listing(&[PROGRAM]), cx)
        });
        let shown = |chain: &str, title: &str, at: i64, front: bool, cx: &mut VisualTestContext| {
            // `Push::show`'s APNs body (crates/pushd)
            let payload = serde_json::json!({
                "aps": { "alert": { "title": title, "body": "said" }, "thread-id": PROGRAM },
                "kind": "notice", "chain": chain, "program": PROGRAM,
                "tag": format!("{PROGRAM}/room"), "title": title, "body": "said",
                "link": "", "until": 0,
            });
            // opened from the OS: the one at 101
            let opened = at == 101;
            let notice = crate::os::OsNotice::of(&payload.to_string(), at, front, opened);
            cx.update(|_, cx| os_notice(notice.expect("a notice"), cx));
            run_out(cx);
        };
        let rows = |cx: &mut VisualTestContext| {
            let entries = shared.notifications.read_with(cx, |it, _| it.entries());
            let rows = entries
                .iter()
                .map(|entry| (entry.title.clone(), entry.tag.clone(), entry.at, entry.read));
            rows.collect::<Vec<_>>()
        };
        shown("garage#1", "Hello", 100, false, cx);
        assert_eq!(rows(cx), [], "logged before its chain's log was open");

        shared.session.update(cx, |session, cx| {
            let state = SessionState {
                connected: true,
                chain: "garage#1".into(),
                ..Default::default()
            };
            session.seed(state, cx);
        });
        run_out(cx);
        let hello = ("Hello".to_owned(), "room".to_owned(), 100, false);
        assert_eq!(rows(cx), std::slice::from_ref(&hello));
        shown("garage#1", "Hello", 100, false, cx);
        assert_eq!(
            rows(cx),
            [hello],
            "the OS's list read again logged it again"
        );
        assert!(cx.debug_bounds("tab/notifications/dot").is_some());
        assert_eq!(notifications_tab_name(cx), "Notifications, 1 unread");

        shown("garage#1", "Hello", 101, false, cx);
        assert!(rows(cx)[0].3, "opened from the OS, and still unread");
        assert_eq!(notifications_tab_name(cx), "Notifications");

        shown("garage#1", "In front", 200, true, cx);
        shown("other#2", "Elsewhere", 200, false, cx);
        // the OS's list handed over again while it waits
        shown("other#2", "Elsewhere", 200, false, cx);
        let read = ("Hello".to_owned(), "room".to_owned(), 100, true);
        assert_eq!(rows(cx), [read], "logged one this phone's run decides");
        let waiting = cx.update(|_, cx| cx.global::<Phone>().notices.clone());
        assert_eq!(
            waiting.len(),
            1,
            "the other chain's did not wait, or waited twice"
        );
    }

    /// A pushed notice's row read or cleared in the Notifications tab takes
    /// the OS's copy out of its list, so the next read of the list (each
    /// return to the front) does not log it again: "Clear read" takes the
    /// read ones (read elsewhere: its program saw it), a row opened and
    /// "Mark all read" take theirs as they are read.
    #[gpui_kit::test]
    fn a_row_read_or_cleared_here_leaves_the_os_list_too(cx: &mut TestAppContext) {
        use ducktape_host::session::SessionState;
        let _seat = ducktape_host::backend::seat_serial();
        let (root, _, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        ducktape_host::runtime::seat_for_test(PROGRAM, 320);
        shared.rail.update(cx, |rail, cx| {
            rail.read_off(Roster::listing(&[PROGRAM]), cx)
        });
        shared.session.update(cx, |session, cx| {
            let state = SessionState {
                connected: true,
                chain: "garage#1".into(),
                ..Default::default()
            };
            session.seed(state, cx);
        });
        crate::os::WITHDRAWN.with(|withdrawn| withdrawn.borrow_mut().clear());
        // the OS's list: what it shows, by the push server's tag
        let list = [("room", 100), ("desk", 110), ("hall", 120)];
        let withdrawn = || crate::os::WITHDRAWN.with(|withdrawn| withdrawn.borrow().clone());
        let read_list = |cx: &mut VisualTestContext| {
            let withdrawn = withdrawn();
            for (tag, at) in list {
                let tag = format!("{PROGRAM}/{tag}");
                if withdrawn.contains(&tag) {
                    continue;
                }
                let payload = serde_json::json!({
                    "kind": "notice", "chain": "garage#1", "program": PROGRAM, "tag": tag,
                    "title": tag, "body": "said", "link": "", "until": 0,
                });
                let notice = crate::os::OsNotice::of(&payload.to_string(), at, false, false);
                cx.update(|_, cx| os_notice(notice.expect("a notice"), cx));
            }
            run_out(cx);
            let entries = shared.notifications.read_with(cx, |it, _| it.entries());
            entries
                .iter()
                .map(|entry| (entry.tag.clone(), entry.read))
                .collect::<Vec<_>>()
        };
        let click = |id: &'static str, cx: &mut VisualTestContext| {
            let at = cx.debug_bounds(id).expect(id);
            cx.simulate_click(at.center(), gpui_kit::Modifiers::none());
            run_out(cx);
        };
        let unread = |tag: &str| (tag.to_owned(), false);
        assert_eq!(
            read_list(cx),
            [unread("hall"), unread("desk"), unread("room")]
        );
        // its program saw "room" (`notify.seen`) where no withdraw reached this list
        let center = shared
            .notifications
            .read_with(cx, |it, _| it.center().clone());
        center.lock().read_tag(PROGRAM, "room");
        shared.notifications.update(cx, |it, cx| _ = it.refresh(cx));
        click("tab/notifications", cx);
        click("notifications/clear-read", cx);
        assert_eq!(
            read_list(cx),
            [unread("hall"), unread("desk")],
            "cleared, then back from the OS's list"
        );
        let entries = shared.notifications.read_with(cx, |it, _| it.entries());
        let hall = entries
            .iter()
            .find(|entry| entry.tag == "hall")
            .expect("hall")
            .id;
        click(format!("notifications/{hall}").leak(), cx);
        assert!(
            withdrawn().contains(&format!("{PROGRAM}/hall")),
            "opened, still in the OS's list"
        );
        root.update(cx, |root, cx| root.back(cx));
        run_out(cx);
        click("notifications/mark-all", cx);
        assert!(
            withdrawn().contains(&format!("{PROGRAM}/desk")),
            "read, still in the OS's list"
        );
        click("notifications/clear-read", cx);
        assert_eq!(
            read_list(cx),
            [],
            "read and cleared, then back from the OS's list"
        );
    }

    #[gpui_kit::test]
    fn a_cancelled_touch_on_a_tile_opens_nothing_and_a_tap_opens_it(cx: &mut TestAppContext) {
        let _seat = ducktape_host::backend::seat_serial();
        let (root, stack, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        ducktape_host::runtime::seat_for_test(PROGRAM, 320);
        shared.rail.update(cx, |rail, cx| {
            rail.read_off(Roster::listing(&[PROGRAM]), cx)
        });
        run_out(cx);

        const TILE: &str = "home/phone-root-test";
        let at = cx.debug_bounds(TILE).expect("the tile drawn").center();
        let touch = |id: u64, phase: gpui_kit::TouchPhase, cx: &mut VisualTestContext| {
            cx.simulate_event(gpui_kit::TouchEvent {
                id: gpui_kit::TouchId(id),
                phase,
                position: at,
                predicted_position: None,
                force: None,
            });
        };

        touch(1, gpui_kit::TouchPhase::Started, cx);
        assert_eq!(top(&stack, cx), None, "opened at touch-down");
        touch(1, gpui_kit::TouchPhase::Cancelled, cx);
        assert_eq!(
            top(&stack, cx),
            None,
            "a cancelled touch opened the program"
        );

        touch(2, gpui_kit::TouchPhase::Started, cx);
        touch(2, gpui_kit::TouchPhase::Ended, cx);
        cx.run_until_parked();
        assert_eq!(
            top(&stack, cx),
            Some(PROGRAM),
            "the tap did not open the program at lift"
        );
    }

    /// Home over a roster read: while the programs' bytes are read the
    /// caption counts them read and no tile stands; once every one is read
    /// (the host shows them all at once) every tile with a view comes in the
    /// same frame, dimmed and with no word, only the one a loader compiles
    /// ringed, the ring running as the clock does (held still under Reduce
    /// Motion); the caption counts the views as they are prepared and is the
    /// normal one once all are settled, and nothing redraws Home after.
    #[gpui_kit::test]
    fn home_shows_every_tile_at_once_once_read_and_counts_them_prepared(cx: &mut TestAppContext) {
        use ducktape_host::runtime::{
            seat_compiled_for_test, seat_empty_for_test, seat_listed_for_test,
            seat_reading_for_test,
        };
        let (root, _, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        let (alpha, beta, none) = ("phone-prep-alpha", "phone-prep-beta", "phone-prep-none");
        seat_reading_for_test(alpha, false);
        seat_reading_for_test(beta, false);
        seat_reading_for_test(none, true);
        shared.rail.update(cx, |rail, cx| {
            rail.read_off(Roster::listing(&[alpha, beta, none]), cx)
        });
        run_out(cx);
        let refresh = |cx: &mut VisualTestContext| {
            shared.rail.update(cx, |rail, cx| _ = rail.refresh(cx));
            run_out(cx);
        };
        let caption = |cx: &mut VisualTestContext| {
            root.read_with(cx, |root, cx| {
                root.run.caption(root.shared.rail.read(cx).rows())
            })
        };
        let shown = |what: &str, cx: &mut VisualTestContext| {
            [alpha, beta].map(|program| {
                let id = format!("home/{program}{what}").leak();
                cx.debug_bounds(id).is_some()
            })
        };
        assert_eq!(caption(cx).as_deref(), Some("Loading 2/3"));
        assert_eq!(shown("", cx), [false; 2], "a tile before all are read");
        seat_reading_for_test(alpha, true);
        refresh(cx);
        assert_eq!(caption(cx).as_deref(), Some("Loading 3/3"));
        assert_eq!(shown("", cx), [false; 2], "a tile before all are read");

        seat_listed_for_test(alpha, "Alpha", false);
        seat_listed_for_test(beta, "Beta", false);
        seat_empty_for_test(none);
        refresh(cx);
        assert_eq!(shown("", cx), [true; 2], "the tiles came apart");
        assert_eq!(caption(cx).as_deref(), Some("Preparing 1/2"));
        assert_eq!(shown("/ring", cx), [false; 2], "a waiting tile ringed");
        assert_eq!(
            shown("/note", cx),
            [false; 2],
            "a word under a tile on its way"
        );

        seat_listed_for_test(alpha, "Alpha", true);
        refresh(cx);
        assert_eq!(caption(cx).as_deref(), Some("Preparing 1/2 · Alpha"));
        assert_eq!(
            shown("/ring", cx),
            [true, false],
            "not the one compiling ringed"
        );
        const RING: &str = "home/phone-prep-alpha/ring";
        const RUN: &str = "home/phone-prep-alpha/ring/run";
        let tile = cx.debug_bounds("home/phone-prep-alpha").expect("the tile");
        let ring = cx.debug_bounds(RING).expect("the ring");
        assert_eq!(
            ring.size.width,
            gpui_kit::px(64.),
            "not just outside the glyph"
        );
        assert!(tile.contains(&ring.center()));
        // the clock on: the ring's part has moved round, and moves again
        let step = |cx: &mut VisualTestContext| {
            let before = cx.debug_bounds(RUN).expect("the ring's running part");
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(200));
            cx.run_until_parked();
            before != cx.debug_bounds(RUN).expect("the ring's running part")
        };
        assert!(step(cx) && step(cx), "the ring stood still");
        cx.update(|_, cx| cx.set_reduce_motion(true));
        cx.update(|window, _| window.refresh());
        run_out(cx);
        assert!(cx.debug_bounds(RING).is_some(), "no still ring");
        assert!(!step(cx), "the ring ran under Reduce Motion");
        cx.update(|_, cx| cx.set_reduce_motion(false));

        seat_compiled_for_test(alpha, "Alpha", "");
        seat_listed_for_test(beta, "Beta", true);
        refresh(cx);
        assert_eq!(caption(cx).as_deref(), Some("Preparing 2/2 · Beta"));
        assert_eq!(shown("/ring", cx), [false, true]);

        seat_compiled_for_test(beta, "Beta", "");
        refresh(cx);
        assert_eq!(caption(cx), None, "the count stayed once all settled");
        let renders = root.read_with(cx, |root, _| root.renders);
        run_out(cx);
        assert_eq!(frames(cx), 0, "a frame asked for with nothing preparing");
        let redrawn = root.read_with(cx, |root, _| root.renders) - renders;
        assert_eq!(redrawn, 0, "Home redrew with nothing preparing");
    }

    /// The Account tab's System / Light / Dark sets the appearance at a
    /// press; the palette (and the kit theme every view reads) follows the
    /// choice, not the system's, and System hands it back to the window's
    /// own.
    #[gpui_kit::test]
    fn the_palette_follows_the_appearance_chosen_not_the_system(cx: &mut TestAppContext) {
        let (root, _, cx) = phone(cx);
        let prefs = root.read_with(cx, |root, _| root.shared.prefs.clone());
        click("tab/account", cx);
        let system = cx.update(|window, _| {
            gpui_kit::component::ThemeMode::from(window.appearance()).is_dark()
        });
        let dark = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| {
                let theme = gpui_kit::component::Theme::global(cx).is_dark();
                (look::palette(cx).dark(), theme)
            })
        };
        for (choice, mode, chosen) in [
            ("appearance/dark", Appearance::Dark, true),
            ("appearance/light", Appearance::Light, false),
            ("appearance/system", Appearance::System, system),
        ] {
            click(choice, cx);
            let saved = prefs.read_with(cx, |prefs, _| prefs.get().appearance);
            assert_eq!(saved, mode, "{choice} did not set the pref");
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
        root.update(cx, |root, cx| root.go_to(Tab::Home, cx));
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

    /// Back on Switch network closes an alert over it first, then the
    /// sheet, back to the screen under it: never the address screen's own
    /// Back, which puts the app behind the others.
    #[gpui_kit::test]
    fn back_on_switch_network_closes_an_alert_first_then_the_sheet_and_never_leaves_the_app(
        cx: &mut TestAppContext,
    ) {
        let (root, _, cx) = phone(cx);
        show_switch(&root, cx);
        let sheet = |cx: &mut VisualTestContext| root.read_with(cx, |root, _| root.sheet);
        assert_eq!(sheet(cx), Some(Sheet::Switch));
        root.update(cx, |root, cx| root.alert("Another network", "x".into(), cx));
        root.update(cx, |root, cx| root.back(cx));
        let alert = root.read_with(cx, |root, _| root.alert.is_some());
        assert_eq!(
            (alert, sheet(cx)),
            (false, Some(Sheet::Switch)),
            "Back did not close the alert alone"
        );
        run_out(cx);
        root.update(cx, |root, cx| root.back(cx));
        assert_eq!(sheet(cx), None, "Back did not close Switch network");
        let page = root.read_with(cx, |root, cx| root.page(cx));
        assert_eq!(page, Page::Root(Tab::Home));
    }

    /// On a network, Continue on the node in hand (the field holds it, the
    /// keys in it, the sheet at its top) closes Switch network at once; a
    /// switch whose node does not answer says why under the field, not in a
    /// notice over it, and the sheet stays; one that lands closes it.
    #[gpui_kit::test]
    fn a_switch_that_stays_or_lands_closes_the_sheet_and_a_failed_one_says_why_under_the_field(
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
        let sheet = |cx: &mut VisualTestContext| root.read_with(cx, |root, _| root.sheet);
        let join = root.read_with(cx, |root, _| root.join.clone());
        // left scrolled down (the last time, or the join's own screen)
        let scrolled = point(px(0.), px(-50.));
        join.read_with(cx, |join, _| join.scroll.set_offset(scrolled));
        // read before a draw, which would clamp it to this short list
        let top = cx.update(|window, cx| {
            root.update(cx, |root, cx| root.show_switch(window, cx));
            join.read(cx).scroll.offset().y
        });
        assert_eq!(top, px(0.), "Switch network opened scrolled down");
        run_out(cx);
        let focused = cx.update(|window, cx| {
            join.read(cx)
                .node
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        });
        assert!(focused, "the keys were not in the field");
        let at = cx.debug_bounds("join/connect").expect("Continue drawn");
        cx.simulate_click(at.center(), gpui_kit::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            sheet(cx),
            None,
            "Continue on the node in hand kept the sheet"
        );
        show_switch(&root, cx);

        let (silent, _) =
            source(|_| Err("error sending request for url (http://127.0.0.1:9/v1/status)".into()));
        shared.session.update(cx, |session, cx| {
            session.connect_through("http://127.0.0.1:9".into(), silent, cx)
        });
        until(cx, |cx| {
            join.read_with(cx, |join, _| join.refused.is_some())
        });
        let said = join.read_with(cx, |join, _| join.refused.clone().unwrap_or_default());
        assert!(
            said.starts_with("Nothing answered at 127.0.0.1:9."),
            "{said}"
        );
        let notice = shared.toast.read_with(cx, |toast, _| toast.get().clone());
        assert_eq!(notice, "", "the failure was also said over the sheet");
        run_out(cx);
        assert_eq!(sheet(cx), Some(Sheet::Switch));
        assert!(
            cx.debug_bounds("join/connect").is_some(),
            "the failure took the field away"
        );

        let (answers, _) = source(|_| Ok(status(1)));
        shared.session.update(cx, |session, cx| {
            session.connect_through("http://127.0.0.1:2".into(), answers, cx)
        });
        until(cx, |cx| sheet(cx).is_none());
        let page = root.read_with(cx, |root, cx| root.page(cx));
        assert_eq!(page, Page::Root(Tab::Home), "the switch did not land here");
    }

    #[gpui_kit::test]
    fn a_views_link_window_opens_as_its_link_open_does(cx: &mut TestAppContext) {
        const LINKED: &str = "phone-link-window-target";
        let (root, stack, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        shared
            .rail
            .update(cx, |rail, cx| rail.read_off(Roster::listing(&[LINKED]), cx));
        run_out(cx);
        let link = format!("duck://{LINKED}/room/7");
        let asked = SeatsEvent::OpenWindow("phone-link-window-view", 0, link);
        root.update(cx, |root, cx| root.seats_said(&asked, cx));
        run_out(cx);
        assert_eq!(top(&stack, cx), Some(LINKED));
        let instance = stack.read_with(cx, |stack, _| stack.instance(LINKED));
        let Some(instance) = instance else {
            panic!("the link opened nothing");
        };
        assert_eq!(
            ducktape_host::runtime::take_route(LINKED, instance).as_deref(),
            Some("room/7")
        );
    }

    /// A tapped notice of another network in the list switches to that
    /// network and opens there once it lists its programs, at a cold start
    /// too (the link held until the network first reached lists its own).
    /// One whose network is not in the list says whose it is, as before,
    /// and reaches nothing.
    #[gpui_kit::test]
    fn another_listed_networks_link_switches_to_it_and_opens_there(cx: &mut TestAppContext) {
        use ducktape_host::backend::RecentEndpoint;
        use ducktape_host::session::SessionState;
        use ducktape_host::session::tests::{source, status};
        use join::tests::until;
        const HERE: &str = "http://127.0.0.1:1";
        const THERE: &str = "http://127.0.0.1:2";
        // the switch leaves the network in hand: the seat is one for the process
        let _seat = ducktape_host::backend::seat_serial();
        let (root, stack, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        let node = |url: &str, network: &str| RecentEndpoint {
            url: url.into(),
            network: network.into(),
            ..RecentEndpoint::default()
        };
        let on = |url: &str, network: &str, chain: &str, recent| SessionState {
            connected: true,
            connected_rpc: url.into(),
            network: network.into(),
            chain: chain.into(),
            recent_endpoints: recent,
            ..SessionState::default()
        };
        // there is testkit as `status` answers it; the phone was on it
        // before, and is on home now
        let there = ducklink::ChainId::of("testkit", &[0; 32])
            .unwrap()
            .to_string();
        let before = on(THERE, "testkit", &there, vec![node(THERE, "testkit")]);
        shared
            .session
            .update(cx, |session, cx| session.seed(before, cx));
        cx.run_until_parked();
        let recent = vec![node(HERE, "home"), node(THERE, "testkit")];
        let now = on(HERE, "home", "home#0a0a0a0a", recent);
        shared
            .session
            .update(cx, |session, cx| session.seed(now, cx));
        cx.run_until_parked();

        // the tap launched the app: held until home lists its programs
        let link = ducklink::mint(&there, PROGRAM, &["room"]).unwrap();
        root.update(cx, |root, cx| root.take_link(link.clone(), cx));
        cx.run_until_parked();
        let reaching = |cx: &mut VisualTestContext| {
            shared
                .session
                .read_with(cx, |session, _| session.get().reaching.clone())
        };
        assert_eq!(reaching(cx), "", "a switch before home listed its programs");
        ducktape_host::runtime::seat_for_test(PROGRAM, 320);
        shared.rail.update(cx, |rail, cx| {
            rail.read_off(Roster::listing(&[PROGRAM]), cx)
        });
        assert_eq!(
            reaching(cx),
            THERE,
            "the link did not switch to its network"
        );
        assert!(root.read_with(cx, |root, _| root.alert.is_none()));
        assert_eq!(top(&stack, cx), None, "the link opened on home");

        // testkit answers (in place of the node the switch asked)
        let (answers, _) = source(|_| Ok(status(1)));
        shared.session.update(cx, |session, cx| {
            session.connect_through(THERE.into(), answers, cx)
        });
        until(cx, |cx| {
            shared
                .session
                .read_with(cx, |session, _| session.get().chain == there)
        });
        cx.run_until_parked();
        assert_eq!(
            top(&stack, cx),
            None,
            "the link opened before testkit listed"
        );
        shared.rail.update(cx, |rail, cx| {
            rail.read_off(Roster::listing(&[PROGRAM]), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            top(&stack, cx),
            Some(PROGRAM),
            "the link did not open on testkit"
        );

        // a network not in the list: said, not reached
        let far = ducklink::mint("far#0c0c0c0c", PROGRAM, &[]).unwrap();
        root.update(cx, |root, cx| root.take_link(far, cx));
        cx.run_until_parked();
        let alert = root.read_with(cx, |root, _| root.alert.clone());
        assert_eq!(
            alert,
            Some((
                "Another network",
                "This link is for far, you're on testkit.".to_owned()
            ))
        );
        assert_eq!(
            reaching(cx),
            "",
            "a link of no listed network reached a node"
        );
    }

    /// A tapped notice's switch that does not land leaves nothing behind.
    /// One whose node is the one in hand (what the registrar heard of it
    /// is stale) starts no switch and says whose link it is at once; one a
    /// switch of the reader's own took over raises no "Another network"
    /// when that one lands; one a disconnect dropped opens no old link on
    /// the next network.
    #[gpui_kit::test]
    fn a_tapped_switch_that_does_not_land_leaves_nothing_behind(cx: &mut TestAppContext) {
        use ducktape_host::backend::{NodeStatus, RecentEndpoint};
        use ducktape_host::session::SessionState;
        use ducktape_host::session::tests::{source, status};
        use join::tests::until;
        const HERE: &str = "http://127.0.0.1:1";
        const THERE: &str = "http://127.0.0.1:2";
        const MINE: &str = "http://127.0.0.1:3";
        const FAR: &str = "http://127.0.0.1:4";
        let _seat = ducktape_host::backend::seat_serial();
        let (root, stack, cx) = phone(cx);
        let shared = root.read_with(cx, |root, _| root.shared.clone());
        let registrar = cx.update(|_, cx| cx.global::<Phone>().registrar.clone());
        let node = |url: &str, network: &str| RecentEndpoint {
            url: url.into(),
            network: network.into(),
            ..RecentEndpoint::default()
        };
        let chain_of = |network| {
            ducklink::ChainId::of(network, &[0; 32])
                .unwrap()
                .to_string()
        };
        let (there, far) = (chain_of("testkit"), chain_of("far"));
        let answering = |network: &'static str| {
            source(move |_| {
                Ok(NodeStatus {
                    network: network.into(),
                    ..status(1)
                })
            })
            .0
        };
        let list = |cx: &mut VisualTestContext| {
            shared.rail.update(cx, |rail, cx| {
                rail.read_off(Roster::listing(&[PROGRAM]), cx)
            });
            cx.run_until_parked();
        };
        let switching =
            |cx: &mut VisualTestContext| root.read_with(cx, |root, _| root.switching.is_some());
        let alert = |cx: &mut VisualTestContext| root.read_with(cx, |root, _| root.alert.clone());
        let tap = |link: &str, cx: &mut VisualTestContext| {
            root.update(cx, |root, cx| root.take_link(link.to_owned(), cx))
        };
        // on home, testkit listed and heard serving `there` before
        let before = SessionState {
            connected: true,
            connected_rpc: THERE.into(),
            network: "testkit".into(),
            chain: there.clone(),
            recent_endpoints: vec![node(THERE, "testkit")],
            ..SessionState::default()
        };
        shared
            .session
            .update(cx, |session, cx| session.seed(before, cx));
        cx.run_until_parked();
        let now = SessionState {
            connected: true,
            connected_rpc: HERE.into(),
            network: "home".into(),
            chain: "home#0a0a0a0a".into(),
            recent_endpoints: vec![node(HERE, "home"), node(THERE, "testkit")],
            ..SessionState::default()
        };
        shared
            .session
            .update(cx, |session, cx| session.seed(now, cx));
        cx.run_until_parked();
        ducktape_host::runtime::seat_for_test(PROGRAM, 320);
        list(cx);
        let to_there = ducklink::mint(&there, PROGRAM, &["room"]).unwrap();

        // the registrar heard the node in hand serve `there`: no switch
        registrar.update(cx, |this, _| this.heard(HERE, &there));
        tap(&to_there, cx);
        assert!(!switching(cx), "a switch the session never started waits");
        assert_eq!(
            alert(cx).map(|(title, _)| title),
            Some("Another network"),
            "the tap was not said"
        );
        root.update(cx, |root, _| root.alert = None);
        registrar.update(cx, |this, _| this.heard(HERE, "home#0a0a0a0a"));

        // a switch that lands opens the link there (the one in hand after)
        tap(&to_there, cx);
        assert!(switching(cx), "the tap did not switch");
        shared.session.update(cx, |session, cx| {
            session.connect_through(THERE.into(), answering("testkit"), cx)
        });
        until(cx, |cx| {
            shared
                .session
                .read_with(cx, |session, _| session.get().chain == there)
        });
        list(cx);
        assert_eq!(top(&stack, cx), Some(PROGRAM));

        // far listed too; the tap's switch to it taken over by the
        // reader's own, to another node of the chain in hand
        ducktape_host::backend::note_endpoint(node(FAR, "far"));
        let listed = SessionState {
            recent_endpoints: ducktape_host::backend::recent_endpoints(),
            ..shared
                .session
                .read_with(cx, |session, _| session.get().clone())
        };
        shared
            .session
            .update(cx, |session, cx| session.seed(listed, cx));
        registrar.update(cx, |this, _| this.heard(FAR, &far));
        let to_far = ducklink::mint(&far, PROGRAM, &[]).unwrap();
        tap(&to_far, cx);
        assert!(switching(cx), "the tap did not switch");
        shared.session.update(cx, |session, cx| {
            session.connect_through(MINE.into(), answering("testkit"), cx)
        });
        until(cx, |cx| {
            shared
                .session
                .read_with(cx, |session, _| session.get().connected_rpc == MINE)
        });
        assert_eq!(
            alert(cx),
            None,
            "the reader's own switch raised the tap's alert"
        );
        assert!(!switching(cx));

        // the tap's switch dropped by a disconnect: the next network
        // reached, far itself, opens no old link
        tap(&to_far, cx);
        assert!(switching(cx), "the tap did not switch");
        shared
            .session
            .update(cx, |session, cx| session.disconnect(cx));
        assert!(!switching(cx), "a disconnect left the tap's switch waiting");
        shared.session.update(cx, |session, cx| {
            session.connect_through(FAR.into(), answering("far"), cx)
        });
        until(cx, |cx| {
            shared
                .session
                .read_with(cx, |session, _| session.get().chain == far)
        });
        list(cx);
        assert_eq!(top(&stack, cx), None, "an old link opened");
        assert_eq!(alert(cx), None);
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
                notices: Vec::new(),
                registrar,
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
        view_page_of(key, depth, back, Vec::new())
    }

    /// [`view_page`] holding `children`, top to bottom.
    fn view_page_of(
        key: &str,
        depth: u32,
        back: bool,
        children: Vec<view_wire::Node>,
    ) -> view_wire::Node {
        let style = ducktape_host::render::test_style(div().size_full().style().clone());
        let column =
            ducktape_host::render::test_style(div().size_full().flex().flex_col().style().clone());
        let page = view_wire::Node::Container(view_wire::ContainerNode {
            id: Some(view_wire::ElementIdWire::Name(key.into())),
            style: column,
            interactivity: Some(Box::new(view_wire::Interactivity {
                page: Some(depth),
                ..Default::default()
            })),
            children,
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
        root.update(cx, |root, cx| root.drag(width / 4., 0., cx));
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

        root.update(cx, |root, cx| root.drag(width / 2., 0., cx));
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
        root.update(cx, |root, cx| root.drag(width / 4., 0., cx));
        assert_eq!(finger(cx), None, "the view's page followed under an alert");
        ducktape_host::contract::testing::touch_first_for_test(false);
    }

    /// A box `height` points tall across the page, its id `id` when it has
    /// one, scrolling sideways over `content` points of width when that is
    /// given.
    fn strip(id: Option<&str>, height: f32, content: Option<f32>) -> view_wire::Node {
        let mut style = div().w_full().h(px(height)).flex_none().style().clone();
        let mut children = Vec::new();
        if let Some(width) = content {
            style.overflow.x = Some(gpui_kit::Overflow::Scroll);
            let inner = div().w(px(width)).h(px(height)).flex_none().style().clone();
            children.push(view_wire::Node::Container(view_wire::ContainerNode {
                id: None,
                style: ducktape_host::render::test_style(inner),
                interactivity: None,
                children: Vec::new(),
            }));
        }
        view_wire::Node::Container(view_wire::ContainerNode {
            id: id.map(|id| view_wire::ElementIdWire::Name(id.into())),
            style: ducktape_host::render::test_style(style),
            interactivity: None,
            children,
        })
    }

    /// A swipe from anywhere begins over the plain part of a view's page
    /// with a page under it, never over a scroller with room to scroll
    /// sideways (one whose content fits takes no sideways drag): a swipe
    /// that starts there is the scroller's, and only the left edge's drags
    /// the page from there. None begins at Home, while a finger holds the
    /// page or it slides, or under an alert (one of what covers the screen).
    #[gpui_kit::test]
    fn a_swipe_from_anywhere_begins_over_plain_content_not_a_sideways_scroller(
        cx: &mut TestAppContext,
    ) {
        const SWIPED: &str = "phone-anywhere-test";
        let _seat = ducktape_host::backend::seat_serial();
        ducktape_host::contract::testing::touch_first_for_test(true);
        let frame = |root, busy| view_wire::Frame {
            root: Some(root),
            busy,
            ..Default::default()
        };
        let room = vec![
            strip(None, 200., None),
            strip(Some("wide"), 100., Some(2000.)),
            strip(Some("fits"), 100., Some(10.)),
        ];
        ducktape_host::runtime::seat_ticking_for_test(
            SWIPED,
            320,
            &[
                frame(view_page("rooms", 0, false), true),
                frame(view_page_of("room", 1, true, room), false),
                view_wire::Frame {
                    unchanged: true,
                    ..Default::default()
                },
            ],
        );
        let (root, _, cx) = phone(cx);
        run_out(cx);
        assert!(!swipe::published().open, "a swipe began at Home");
        root.update(cx, |root, cx| root.open(SWIPED, cx));
        run_out(cx);
        run_out(cx);

        let shown = swipe::published();
        assert!(shown.open, "no swipe begins over a page pushed in a view");
        let [wide] = shown.claimed[..] else {
            panic!("not one box takes a sideways drag: {:?}", shown.claimed);
        };
        assert_eq!(wide.size.height, px(100.));
        let rightward = point(px(300.), px(40.));
        assert!(
            !swipe::begins_here(wide.center(), rightward),
            "a swipe began over the sideways scroller"
        );
        let above = point(wide.center().x, wide.top() - px(50.));
        assert!(swipe::begins_here(above, rightward), "over plain content");
        let fits = point(wide.center().x, wide.bottom() + px(50.));
        assert!(
            swipe::begins_here(fits, rightward),
            "over a scroller that fits"
        );

        // the left edge's swipe, begun over the scroller, drags the page
        let tree = view_tree(&root, cx);
        let finger = |cx: &mut VisualTestContext| tree.read_with(cx, |tree, _| tree.page_finger());
        root.update(cx, |root, cx| root.drag(40., 0., cx));
        frames(cx);
        assert!(
            finger(cx).is_some(),
            "the edge's swipe did not drag the page"
        );
        assert!(!swipe::published().open, "a swipe began under a finger");
        root.update(cx, |root, cx| root.drag_cancelled(cx));
        frames(cx);
        assert!(!swipe::published().open, "a swipe began as the page slid");
        run_out(cx);
        assert!(swipe::published().open);

        root.update(cx, |root, cx| {
            root.alert("Alert", "Over the view".into(), cx)
        });
        run_out(cx);
        assert!(!swipe::published().open, "a swipe began under an alert");
        ducktape_host::contract::testing::touch_first_for_test(false);
    }

    /// The tabs' swipe under way: the tab it settles beside, its offset at
    /// the start and its speed there.
    fn settling(
        root: &Entity<Root>,
        cx: &mut VisualTestContext,
    ) -> Option<(Option<Tab>, f32, f32)> {
        root.read_with(cx, |root, _| match root.paging {
            Some(Paging::Settling { beside, tween }) => Some((beside, tween.from, tween.velocity)),
            _ => None,
        })
    }

    /// At Home's root a sideways swipe let go past the rule lands on Open,
    /// which slides the rest of the way in and then asks for no frame; the
    /// same swipe on a program pushed over Open goes back to Open, not to a
    /// tab, and one the other way there does nothing.
    #[gpui_kit::test]
    fn a_sideways_swipe_at_a_tab_root_lands_beside_and_on_a_pushed_page_goes_back(
        cx: &mut TestAppContext,
    ) {
        let (root, stack, cx) = phone(cx);
        run_out(cx);
        let shown = swipe::published();
        assert!(shown.tabs && !shown.open, "no tabs' swipe begins at Home");
        let width = root.read_with(cx, |root, _| root.width);
        let tab = |cx: &mut VisualTestContext| stack.read_with(cx, |stack, _| stack.tab());
        let swipe = |x: f32, cx: &mut VisualTestContext| {
            root.update(cx, |root, cx| root.drag(x, 0., cx));
            let_go(&root, cx);
            cx.run_until_parked();
        };

        swipe(-width * 0.75, cx);
        assert_eq!(tab(cx), Tab::Open, "the swipe did not land on Open");
        assert_eq!(settling(&root, cx), Some((Some(Tab::Home), 0.25, 0.)));
        assert!(frames(cx) > 0, "Open settling asked for no frame");
        run_out(cx);
        assert_eq!(settling(&root, cx), None, "Open settled for ever");
        assert_eq!(frames(cx), 0, "a frame asked for once Open was still");

        root.update(cx, |root, cx| _ = root.open(PROGRAM, cx));
        run_out(cx);
        assert!(
            !swipe::published().tabs,
            "the tabs' swipe began over a program"
        );
        root.update(cx, |root, cx| root.drag(-width * 0.75, 0., cx));
        assert_eq!(
            root.read_with(cx, |root, _| root.dragged),
            Some(0.),
            "dragged leftward"
        );
        let_go(&root, cx);
        run_out(cx);
        assert_eq!((tab(cx), top(&stack, cx)), (Tab::Open, Some(PROGRAM)));
        swipe(width * 0.75, cx);
        assert_eq!(
            (tab(cx), top(&stack, cx)),
            (Tab::Open, None),
            "the swipe on the program did not go back to Open"
        );
        run_out(cx);

        // a program pushed while the finger pages the tabs stays, let go
        // past the rule, and so does the tab under it
        root.update(cx, |root, cx| root.drag(-width * 0.75, 0., cx));
        root.update(cx, |root, cx| _ = root.open(PROGRAM, cx));
        let_go(&root, cx);
        run_out(cx);
        assert_eq!(
            (tab(cx), top(&stack, cx)),
            (Tab::Open, Some(PROGRAM)),
            "the let go picked a tab over the program"
        );
    }

    /// The tab beside slides in next to the one on screen under the finger,
    /// the bar staying still under both; let go short (or ended by the OS),
    /// the tab on screen slides back from the finger at its speed; a short
    /// fast flick goes
    /// on; past the bar's end the screen resists and settles back from
    /// rest; a Back under the finger, a tab picked in the bar or a view's
    /// way to a tab ends the tabs' swipe at once. Nothing pages under an
    /// alert or Reduce Motion.
    #[gpui_kit::test]
    fn the_tabs_follow_the_finger_and_settle_as_a_pop_does(cx: &mut TestAppContext) {
        let (root, stack, cx) = phone(cx);
        let width = root.read_with(cx, |root, _| root.width);
        let tab = |cx: &mut VisualTestContext| stack.read_with(cx, |stack, _| stack.tab());
        let drawn = |id: &'static str, cx: &mut VisualTestContext| {
            cx.update(|window, cx| window.render_frame(cx));
            cx.debug_bounds(id)
        };
        let pick = |tab: Tab, cx: &mut VisualTestContext| {
            root.update(cx, |root, cx| root.pick(tab, cx));
            run_out(cx);
        };
        pick(Tab::Account, cx);
        let (row, bar) = (drawn("account/switch", cx), drawn("tab/account", cx));
        let (row, bar) = (row.expect("Account drawn"), bar.expect("the bar drawn"));
        pick(Tab::Notifications, cx);
        assert_eq!(
            drawn("account/switch", cx),
            None,
            "Account drawn under Notifications"
        );

        root.update(cx, |root, cx| root.drag(-width / 4., 0., cx));
        let beside = drawn("account/switch", cx).expect("Account not drawn beside");
        let shift = f32::from(beside.left() - row.left());
        assert!(
            (shift - width * 0.75).abs() < 1.,
            "Account {shift} in, not {}",
            width * 0.75
        );
        assert_eq!(
            drawn("tab/account", cx),
            Some(bar),
            "the bar moved with the tabs"
        );
        root.update(cx, |root, cx| root.drag(-width / 4., width / 2., cx));
        let_go(&root, cx);
        assert_eq!(
            tab(cx),
            Tab::Notifications,
            "let go short and moving back, it went on"
        );
        assert_eq!(settling(&root, cx), Some((Some(Tab::Account), -0.25, 0.5)));
        run_out(cx);

        // ended by UIKit past the rule: back into place all the same
        root.update(cx, |root, cx| root.drag(-width * 0.75, 0., cx));
        root.update(cx, |root, cx| root.drag_cancelled(cx));
        assert_eq!(tab(cx), Tab::Notifications, "a cancelled swipe went on");
        run_out(cx);

        // a width and a half a second, let go an eighth in
        root.update(cx, |root, cx| root.drag(-width / 8., -width * 1.5, cx));
        let_go(&root, cx);
        assert_eq!(tab(cx), Tab::Account, "a fast short flick did not go on");
        assert_eq!(
            settling(&root, cx),
            Some((Some(Tab::Notifications), 0.875, -1.5))
        );
        run_out(cx);

        // past Account, the bar's end: Account resists, and nothing comes in
        root.update(cx, |root, cx| root.drag(-width / 2., -width, cx));
        let resisted = swipe::resisted(-width / 2., width);
        let drawn_at = drawn("account/switch", cx).expect("Account drawn").left();
        let moved = f32::from(drawn_at - row.left());
        assert!(
            (moved - resisted).abs() < 1.,
            "Account {moved} in, not {resisted}"
        );
        assert!(
            resisted > -width / 2.,
            "Account did not resist past the end"
        );
        let_go(&root, cx);
        let back = Some((None, resisted / width, 0.));
        assert_eq!((tab(cx), settling(&root, cx)), (Tab::Account, back));
        run_out(cx);

        pick(Tab::Open, cx);
        root.update(cx, |root, cx| root.drag(width * 0.75, 0., cx));
        root.update(cx, |root, cx| root.back(cx));
        assert_eq!(settling(&root, cx), None, "a Back under the finger");
        assert_eq!(tab(cx), Tab::Home);
        run_out(cx);

        root.update(cx, |root, cx| root.drag(-width * 0.75, 0., cx));
        let_go(&root, cx);
        assert!(settling(&root, cx).is_some(), "Open is not settling");
        root.update(cx, |root, cx| root.pick(Tab::Account, cx));
        assert_eq!(
            settling(&root, cx),
            None,
            "a tab picked as the tabs settled"
        );
        root.update(cx, |root, cx| root.drag(width * 0.75, 0., cx));
        let_go(&root, cx);
        assert!(
            settling(&root, cx).is_some(),
            "Notifications is not settling"
        );
        root.update(cx, |root, cx| root.go_to(Tab::Home, cx));
        assert_eq!(
            settling(&root, cx),
            None,
            "a way to Home as the tabs settled"
        );
        run_out(cx);

        root.update(cx, |root, cx| {
            root.alert("Alert", "Over the tab".into(), cx)
        });
        run_out(cx);
        assert!(!swipe::published().tabs, "a swipe began under an alert");
        root.update(cx, |root, cx| root.drag(-width / 4., 0., cx));
        assert!(
            root.read_with(cx, |root, _| root.paging.is_none()),
            "paged under an alert"
        );
        root.update(cx, |root, cx| root.back(cx));
        run_out(cx);
        cx.update(|_, cx| cx.set_reduce_motion(true));
        run_out(cx);
        assert!(
            !swipe::published().tabs,
            "a swipe began under Reduce Motion"
        );
        root.update(cx, |root, cx| root.drag(-width / 4., 0., cx));
        assert!(
            root.read_with(cx, |root, _| root.paging.is_none()),
            "paged under Reduce Motion"
        );
        cx.update(|_, cx| cx.set_reduce_motion(false));
    }

    /// Flicks in a row, each let go before the last one's tab has settled,
    /// each catch that tab where it is and go on: Home to Account. A tab
    /// settling back from the bar's end is caught where it is drawn.
    #[gpui_kit::test]
    fn quick_flicks_catch_the_settling_tab_and_each_lands(cx: &mut TestAppContext) {
        let (root, stack, cx) = phone(cx);
        run_out(cx);
        let width = root.read_with(cx, |root, _| root.width);
        let tab = |cx: &mut VisualTestContext| stack.read_with(cx, |stack, _| stack.tab());
        let settled_at = |cx: &mut VisualTestContext| {
            root.read_with(cx, |root, cx| match root.paging {
                Some(Paging::Settling { tween, .. }) if !tween.done(motion::now(cx)) => {
                    tween.at(motion::now(cx))
                }
                _ => panic!("no tab settling"),
            })
        };
        let flick = |cx: &mut VisualTestContext| {
            root.update(cx, |root, cx| root.drag(-width / 8., -width * 1.5, cx));
            let_go(&root, cx);
        };
        flick(cx);
        assert_eq!(tab(cx), Tab::Open);
        for next in [Tab::Notifications, Tab::Account] {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(250));
            frames(cx);
            let at = settled_at(cx);
            assert!(at > 0., "the tab is not on its way in: {at}");
            assert!(
                swipe::published().tabs,
                "no swipe begins as the tab settles"
            );
            root.update(cx, |root, cx| root.drag(0., 0., cx));
            let caught = root.read_with(cx, |root, _| root.along());
            assert_eq!(caught, Some(at), "not caught where the tab was");
            flick(cx);
            assert_eq!(tab(cx), next, "the flick did not land");
        }
        run_out(cx);

        // past Account's end, caught on its way back
        root.update(cx, |root, cx| root.drag(-width / 2., 0., cx));
        let_go(&root, cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(100));
        frames(cx);
        let at = settled_at(cx);
        root.update(cx, |root, cx| root.drag(0., 0., cx));
        let held = root.read_with(cx, |root, cx| root.held_at(root.along().unwrap(), cx).0);
        assert!((held - at).abs() < 1e-4, "caught at {held}, drawn at {at}");
        run_out(cx);
    }

    /// A swipe from Home landing on Open lets the keys go from Home's
    /// filter, as a tab picked in the bar does.
    #[gpui_kit::test]
    fn a_swipe_landing_off_home_lets_its_filter_go(cx: &mut TestAppContext) {
        let (root, _, cx) = phone(cx);
        run_out(cx);
        cx.update(|window, cx| root.update(cx, |root, cx| root.filter(true, window, cx)));
        run_out(cx);
        let focused = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let root = root.read(cx);
                root.search.read(cx).focus_handle(cx).is_focused(window)
            })
        };
        assert!(focused(cx), "the filter did not take the keys");
        let width = root.read_with(cx, |root, _| root.width);
        root.update(cx, |root, cx| root.drag(-width * 0.75, 0., cx));
        let_go(&root, cx);
        run_out(cx);
        assert!(!focused(cx), "Home's filter kept the keys on Open");
    }

    /// No swipe from anywhere begins while a program slides in; once it is
    /// still over Home, one does. Let go, a swipe is judged where and how
    /// fast it went, and the rest of the move starts at the finger's speed:
    /// short and moving back, the program slides back the other way; short
    /// and fast, it pops.
    #[gpui_kit::test]
    fn a_release_keeps_the_fingers_speed(cx: &mut TestAppContext) {
        let (root, stack, cx) = phone(cx);
        root.update(cx, |root, cx| root.open(PROGRAM, cx));
        frames(cx);
        assert!(
            !swipe::published().open,
            "a swipe began as the program slid in"
        );
        run_out(cx);
        assert!(swipe::published().open, "no swipe begins over a program");
        let width = root.read_with(cx, |root, _| root.width);
        let moving = |cx: &mut VisualTestContext| {
            root.read_with(cx, |root, _| {
                root.moving
                    .map(|it| (it.pop, it.tween.from, it.tween.velocity))
            })
        };

        root.update(cx, |root, cx| root.drag(width / 4., -width, cx));
        let_go(&root, cx);
        cx.run_until_parked();
        assert_eq!(moving(cx), Some((false, 0.75, 1.)));
        assert_eq!(
            top(&stack, cx),
            Some(PROGRAM),
            "let go moving back, it popped"
        );
        run_out(cx);

        // a width and a half a second, let go an eighth in
        root.update(cx, |root, cx| root.drag(width / 8., width * 1.5, cx));
        let_go(&root, cx);
        cx.run_until_parked();
        assert_eq!(top(&stack, cx), None, "a fast short flick did not pop");
        assert_eq!(moving(cx), Some((true, 0.125, 1.5)));
        run_out(cx);
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
        root.update(cx, |root, cx| root.drag(width / 2., 0., cx));
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
