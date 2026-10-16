//! The phone's one window: the host started once per process ([`launch`]),
//! the window opened over it ([`open_window`]: once on iOS, once per
//! Activity on Android), what the host hands on acted on (Back, a changed
//! inset, memory pressure, the consent queue, a link), and in the gate
//! build the AX door.
//!
//! The window draws the [`stack`]: a tab root (Home, Open) under the tab
//! bar, or the program on top, its view seated full screen under the
//! capsule; a program's page (its capabilities, from the capsule's About)
//! over either; a sheet over all (the account menu, the capsule's menu,
//! the consent sheet). Each program opened keeps one seat, placed with the
//! safe body and in front only while it is on top and the window is: a
//! program left stays warm where it was (wasm-mobile SPEC §7.1).

mod about;
mod home;
mod look;
mod running;
mod sheet;
pub mod stack;

use ducktape_host::contract::{Boot, OsServices, RunnerConfig, Signal, SoftKeyboard};
use ducktape_host::runner::WasmtimeRunner;
use ducktape_host::runtime::notify::{Opened as Banner, Permission};
use ducktape_host::runtime::{Back, consent};
use ducktape_host::session::{Opening, Place, SeatsEvent, Session, SessionEvent, Shared};
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
use std::collections::BTreeMap;
use std::sync::Arc;

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
    // ponytail: iOS runs wasmtime too until the JSC runner lands (lane J
    // part 2): the simulator JITs; a device refuses at `prepare`, and the
    // seat says so in its standin.
    let runner = Arc::new(WasmtimeRunner::new(RunnerConfig {
        // an arm64 guest's trap under wasmtime's signal handlers kills an
        // Android process (SPEC C3)
        signals_based_traps: !cfg!(target_os = "android"),
        cache_dir: Some(os.dirs().cache.join("view-code")),
    }));
    let boot = Boot {
        os,
        runner,
        open: banner_opened,
    };
    let (host, inbound) = ducktape_host::start::start(boot, cx);
    // `DUCKTAPE_PERF=1` in the launch's environment turns the counters on
    // (iOS: `SIMCTL_CHILD_DUCKTAPE_PERF=1 xcrun simctl launch`; Android has
    // no launch environment, so `MainActivity` makes one of the
    // `-e DUCKTAPE_PERF 1` extra); the sampler reads the OS facts, which
    // `start` installed
    ducktape_host::perf::start();
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
            let stack = cx.global::<Phone>().stack.clone();
            stack.update(cx, |stack, cx| {
                stack.trim();
                cx.notify();
            });
        }
        Signal::Back => {
            if let Some(root) = root(cx) {
                root.update(cx, |root, cx| root.back(cx));
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

/// The window's root: the stack over the host's entities, the seats of the
/// programs open, and the sheet over them.
pub struct Root {
    shared: Shared,
    stack: Entity<Stack>,
    sheet: Option<Sheet>,
    /// The program whose page is over the stack.
    about: Option<&'static str>,
    /// The title each seat's view gave its window, by its instance.
    titles: BTreeMap<u64, String>,
    /// What `Seats` was last told: each open program's place, and the seat
    /// whose view may move the keys.
    placed: (BTreeMap<u64, Place>, Option<u64>),
    /// A link that came before the network listed its programs.
    waiting: Option<String>,
    /// Home has listed the network's programs: every one's code was read
    /// once. Until then Home lists none (a program with no view would show
    /// for a moment); after, one still on its way is left out alone.
    known: bool,
    /// Home's filter.
    search: Entity<InputState>,
    /// Each placed seat, observed: a tree, a standin or a fault that moved
    /// draws the window again.
    watched: BTreeMap<u64, Subscription>,
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
        Self {
            shared,
            stack,
            sheet: None,
            about: None,
            titles: BTreeMap::new(),
            placed: Default::default(),
            waiting: None,
            known: false,
            search,
            watched: BTreeMap::new(),
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

    /// `program` on top, in its seat; the seat's instance.
    fn open(&mut self, program: &'static str, cx: &mut Context<Self>) -> u64 {
        if !matches!(self.sheet, Some(Sheet::Consent(..))) {
            self.sheet = None;
        }
        self.about = None;
        self.stack.update(cx, |stack, cx| {
            let instance = stack.push(program);
            cx.notify();
            instance
        })
    }

    /// `program`'s page over the stack.
    fn show_about(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.sheet = None;
        self.about = Some(program);
        cx.notify();
    }

    fn close_about(&mut self, cx: &mut Context<Self>) {
        self.about = None;
        cx.notify();
    }

    /// `program`'s own link (`duck://<chain>/<program>`) on the clipboard.
    fn copy_link(&mut self, program: &'static str, cx: &mut Context<Self>) {
        let chain = self.shared.session.read(cx).get().chain.clone();
        if let Some(link) = ducklink::mint(&chain, program, &[]) {
            cx.write_to_clipboard(ClipboardItem::new_string(link));
            self.notice("Copied".into(), cx);
        }
        if !matches!(self.sheet, Some(Sheet::Consent(..))) {
            self.sheet = None;
        }
        cx.notify();
    }

    /// The capsule's ×: Home, the program left where it is.
    fn go_home(&mut self, cx: &mut Context<Self>) {
        let now = Self::now(cx);
        self.stack.update(cx, |stack, cx| {
            stack.home(now);
            cx.notify();
        });
    }

    /// Open's ×: the program's seat goes; it stays on Home.
    fn close_program(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.stack.update(cx, |stack, cx| {
            stack.close(program);
            cx.notify();
        });
    }

    /// `sheet` over the screen, unless the consent sheet is up: it stays
    /// until it is answered.
    fn show(&mut self, sheet: Sheet, cx: &mut Context<Self>) {
        if !matches!(self.sheet, Some(Sheet::Consent(..))) {
            self.sheet = Some(sheet);
            cx.notify();
        }
    }

    fn account_sheet(&mut self, cx: &mut Context<Self>) {
        self.show(Sheet::Account, cx);
    }

    fn capsule_menu(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.show(Sheet::Capsule(program), cx);
    }

    /// The sheet goes (the scrim, Back): the consent sheet's ask is
    /// refused, as Cancel refuses it.
    fn close_sheet(&mut self, cx: &mut Context<Self>) {
        match self.sheet.take() {
            Some(Sheet::Consent(ask, _)) => {
                consent::answer(ask, false);
                self.consent_moved(cx);
            }
            Some(_) => cx.notify(),
            None => {}
        }
    }

    /// The consent queue moved: its front shows in the consent sheet, over
    /// any other sheet; none, and the consent sheet goes.
    fn consent_moved(&mut self, cx: &mut Context<Self>) {
        match consent::front() {
            Some((ask, _)) => {
                if !matches!(self.sheet, Some(Sheet::Consent(shown, _)) if shown == ask) {
                    self.sheet = Some(Sheet::Consent(ask, Self::now(cx)));
                }
            }
            None => {
                if let Some(Sheet::Consent(..)) = self.sheet {
                    self.sheet = None;
                }
            }
        }
        cx.notify();
    }

    /// Back (Android's button, iOS's edge): a sheet closes first, then a
    /// program's page; else the view on top goes back inside itself when its last frame's root asks
    /// to hear it; else the stack pops; at Home, the OS's own (SPEC §7.6).
    fn back(&mut self, cx: &mut Context<Self>) {
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
        let popped = self.stack.update(cx, |stack, cx| {
            let popped = stack.back(now);
            cx.notify();
            popped
        });
        if !popped {
            crate::os::back_at_root();
        }
    }

    /// The seat of the program on top.
    fn top_seat(&self, cx: &App) -> Option<Entity<ducktape_host::runtime::Seat>> {
        let stack = self.stack.read(cx);
        let instance = stack.instance(stack.top()?)?;
        self.shared.seats.read(cx).seat(instance)
    }

    /// The network in hand was left: nothing stays open, and a link kept
    /// for it is dropped.
    fn left_network(&mut self, cx: &mut Context<Self>) {
        self.stack.update(cx, |stack, cx| {
            stack.clear();
            cx.notify();
        });
        if !matches!(self.sheet, Some(Sheet::Consent(..))) {
            self.sheet = None;
        }
        self.titles.clear();
        self.about = None;
        self.waiting = None;
        self.known = false;
        cx.notify();
    }

    /// The rail moved: Home redraws, and a link kept until the network
    /// listed its programs opens now.
    fn rail_moved(&mut self, cx: &mut Context<Self>) {
        if !self.known {
            let rows = self.shared.rail.read(cx).rows();
            self.known = !rows.is_empty() && rows.iter().all(|row| !row.pending);
        }
        if self.known
            && let Some(link) = self.waiting.take()
        {
            self.open_link(&link, cx);
        }
        cx.notify();
    }

    /// A link from the OS (`duck://`, or the Expo app's `ducktape://`):
    /// opened once the network lists its programs.
    fn take_link(&mut self, link: String, cx: &mut Context<Self>) {
        let link = match link.strip_prefix("ducktape://") {
            Some(rest) => format!("duck://{rest}"),
            None => link,
        };
        if !self.known {
            self.waiting = Some(link);
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
            Opening::Elsewhere { chain, .. } => {
                self.notice(
                    format!("This link is for {chain}, you're on {network}."),
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
            // the desktop widens a window to a view that came; a phone
            // never does
            SeatsEvent::Seated(_) => {}
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
    /// view's while no sheet is up: told to `Seats` after this draw when
    /// it moved.
    fn place(&mut self, window: &Window, body: (f32, f32), cx: &mut Context<Self>) {
        let handle = window.window_handle();
        let active = window.is_window_active();
        let stack = self.stack.read(cx);
        // a program's page covers the program on top
        let top = stack
            .top()
            .and_then(|program| stack.instance(program))
            .filter(|_| self.about.is_none());
        let wanted: BTreeMap<u64, Place> = stack
            .open()
            .iter()
            .map(|opened| {
                let in_front = active && top == Some(opened.instance);
                let place = (handle, Some(body), in_front);
                (opened.instance, (opened.program, Some(place)))
            })
            .collect();
        let keys = top.filter(|_| self.sheet.is_none());
        if (&wanted, keys) == (&self.placed.0, self.placed.1) {
            return;
        }
        self.placed = (wanted.clone(), keys);
        self.titles
            .retain(|instance, _| wanted.contains_key(instance));
        let seats = self.shared.seats.clone();
        cx.defer(move |cx| {
            seats.update(cx, |seats, cx| seats.place(wanted.clone(), cx));
            for instance in wanted.keys() {
                if let Some(seat) = seats.read(cx).seat(*instance) {
                    let free = keys == Some(*instance);
                    seat.update(cx, |seat, cx| seat.set_keys_free(free, cx));
                }
            }
        });
    }

    /// The sheet up, drawn: the frame over the window with its content.
    fn sheet(
        &mut self,
        sheet: Sheet,
        bottom: gpui_kit::Pixels,
        room: gpui_kit::Pixels,
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
            Sheet::Consent(ask, opened) => {
                let (_, words) = consent::front().filter(|(front, _)| *front == ask)?;
                let title = words.title;
                let this = cx.weak_entity();
                let answer = move |yes: bool, cx: &mut App| {
                    consent::answer(ask, yes);
                    // the window may have closed: the next one shows the
                    // queue's front
                    this.update(cx, |root, cx| root.consent_moved(cx))
                        .unwrap_or_default();
                };
                let approve = answer.clone();
                // Approve takes no press in the sheet's first moment: a
                // second press the view asked for lands before anyone read it
                let approved = move |cx: &mut App| {
                    if Self::now(cx).saturating_duration_since(opened) >= consent::SETTLE {
                        approve(true, cx);
                    }
                };
                let body = consent::sheet(words, palette.dark(), approved, answer);
                (title, body.into_any_element())
            }
        };
        Some(sheet::frame(name, content, bottom, room, palette, cx))
    }

    /// The account menu (the shell boards' 40): who is reading, and the
    /// account's own program (the `programs` view), opened by name.
    fn account_menu(&mut self, palette: look::Palette, cx: &mut Context<Self>) -> AnyElement {
        let network = self.shared.session.read(cx).get().network.clone();
        let (name, caption) = match home::me(&self.shared, cx) {
            Some((number, name)) => (name, format!("account {number} · {network}")),
            None => (String::new(), network),
        };
        let head = sheet::head(palette).child(
            div()
                .flex()
                .items_center()
                .gap(px(look::gap::LG))
                .child(home::avatar(&name, 40., palette))
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

    /// The notice up (Expo `Toasts`): the foreground's block over the tab
    /// bar's line, the words in the background's colour, announced.
    fn toast(
        &self,
        bottom: gpui_kit::Pixels,
        palette: look::Palette,
        cx: &App,
    ) -> Option<AnyElement> {
        let said = self.shared.toast.read(cx).get().clone();
        if said.is_empty() {
            return None;
        }
        let words = ducktape_host::a11y::live(
            div().id("toast").role(gpui_kit::Role::Status),
            gpui_kit::accesskit::Live::Polite,
            said.clone(),
        )
        .bg(palette.foreground())
        .px(px(look::gap::BLOCK))
        .py(px(look::gap::SM))
        .child(look::text(look::SMALL, FontWeight::NORMAL, palette.background()).child(said));
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(bottom)
                .flex()
                .justify_center()
                .child(words)
                .into_any_element(),
        )
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
        self.place(window, body, cx);
        let top = self.stack.read(cx).top();
        let mut root = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(palette.background())
            .text_color(palette.foreground());
        let toast_bottom;
        let about = self
            .about
            .and_then(|program| home::row_of(&self.shared, program, cx));
        if let Some(row) = about {
            let listing = ducktape_host::runtime::listing(row.program);
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .pt(edges.top)
                    .pl(edges.left)
                    .pr(edges.right)
                    .pb(edges.bottom)
                    .child(about::page(&row, listing, palette, cx)),
            );
            toast_bottom = edges.bottom + px(look::gap::LG);
        } else {
            match top {
                Some(program) => {
                    let instance = self.stack.read(cx).instance(program);
                    let seat =
                        instance.and_then(|instance| self.shared.seats.read(cx).seat(instance));
                    let row = home::row_of(&self.shared, program, cx);
                    let body = running::body(seat.as_ref(), row.as_ref(), palette, cx);
                    root = root
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .pt(edges.top)
                                .pl(edges.left)
                                .pr(edges.right)
                                .pb(edges.bottom)
                                .child(body),
                        )
                        .child(running::capsule(
                            program,
                            edges.top,
                            edges.right,
                            palette,
                            cx,
                        ));
                    toast_bottom = edges.bottom + px(look::gap::LG);
                }
                None => {
                    let tab = self.stack.read(cx).tab();
                    let screen = match tab {
                        Tab::Home => {
                            let searching =
                                self.search.read(cx).focus_handle(cx).is_focused(window);
                            home::home(
                                &self.shared,
                                &self.search,
                                searching,
                                self.known,
                                palette,
                                cx,
                            )
                        }
                        Tab::Open => {
                            let opened = self.stack.read(cx).open().to_vec();
                            home::open(&self.shared, &opened, &self.titles, palette, cx)
                        }
                    };
                    root = root.child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .pt(edges.top)
                            .pl(edges.left)
                            .pr(edges.right)
                            .child(screen)
                            .child(home::tab_bar(tab, edges.bottom, palette, cx)),
                    );
                    toast_bottom = edges.bottom + px(look::TAB_BAR + look::gap::LG);
                }
            }
        }
        if let Some(toast) = self.toast(toast_bottom, palette, cx) {
            root = root.child(toast);
        }
        if let Some(sheet) = self.sheet {
            let room = viewport.height - edges.top;
            match self.sheet(sheet, edges.bottom, room, palette, cx) {
                Some(sheet) => root = root.child(sheet),
                // what it was over is gone (an ask answered elsewhere, a
                // program no longer listed)
                None => self.sheet = None,
            }
        }
        root
    }
}
