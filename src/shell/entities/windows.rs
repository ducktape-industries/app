//! The app's OS windows: every one open (its handle, its root view and its
//! own entities: `Desk`, `Front`, `Overlays`, `Spotlight`, `DotSlot`),
//! which is the console, which is in front, and the program in front
//! (`active`: the one an untouched console desk opens). Its methods open,
//! raise and close windows, move a pane between two desks (pop-out, pop-in),
//! open links and programs on the console's desk, and resize the console
//! as the launcher gives way to the desk and back. Notifies when the list,
//! the front or the active program moved; a window's own entities notify
//! on their own.
//!
//! What it hears: the session leaving its network (every desk cleared),
//! a node answering (the console comes forward), a device approved (its
//! dialog closes), a new account (Help opens), the screen crossing
//! (`swap_console`), the appearance chosen (`sync_appearance`), and each
//! window's activation (`front`, a hold let go, the notification centre
//! told). A link a banner's click or the passkey page asks for off the
//! window thread comes over the `Posted` channel (`posted`).
use super::{
    AccountEvent, Entities, Front, Overlay, Overlays, Screen, SessionEvent, Shared, Slice,
    Spotlight, WindowEntities,
};
use crate::runtime::notify::{CenterHandle, Entry};
use crate::runtime::{Link, WindowKey};
use crate::shell::layers::{self, WindowRoot};
use crate::shell::windows::{release_window_input, remove};
use crate::shell::{WindowKind, theme, windows};
use crate::ui::layout::{self, Layout};
use futures::StreamExt as _;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Pixels, Subscription,
    WeakEntity, Window, WindowBounds, WindowId, px, size,
};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// Timed as `reducer.desk`, the name qa's hang rule reads (docs/perf.md).
fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.desk")
}

pub(crate) struct Windows {
    shared: Shared,
    center: CenterHandle,
    handles: BTreeMap<WindowKey, AnyWindowHandle>,
    /// Each OS window's root view (not a program's view).
    views: BTreeMap<WindowKey, WeakEntity<WindowRoot>>,
    /// Each window's own entities, made as the window is asked for and
    /// dropped as it closes.
    by_window: BTreeMap<WindowKey, WindowEntities>,
    /// The main window, once opened; the launcher and the desk both live
    /// in it.
    console: Option<WindowKey>,
    /// The OS window in front, when it is one of ours.
    front: Option<WindowKey>,
    /// The program in front: the focused pane's, wherever it is, or the
    /// one just opened. An untouched console desk opens it.
    active: Option<&'static str>,
    /// Where the desk window was when it last gave way to the launcher:
    /// it comes back there.
    desk_bounds: Option<WindowBounds>,
    /// The console shows the launcher (else the desk), as last seen.
    launcher: bool,
    /// The appearance the theme was last synced to.
    appearance: crate::backend::Appearance,
    subscriptions: Vec<Subscription>,
}

impl Windows {
    /// No windows yet. `shared` is every app-wide entity but this one.
    pub(crate) fn new(shared: Shared, cx: &mut Context<Self>) -> Self {
        let center = shared.notifications.read(cx).center().clone();
        let launcher = *shared.screen.read(cx).get() != Screen::Desk;
        let appearance = shared.prefs.read(cx).get().appearance;
        let subscriptions = vec![
            cx.subscribe(
                &shared.session,
                |this, _, event: &SessionEvent, cx| match event {
                    SessionEvent::LeftNetwork => this.left_network(cx),
                    // the console comes forward, or opens, as the tray's Open
                    SessionEvent::Connected => this.raise_console(cx),
                    SessionEvent::Toast(_) => {}
                },
            ),
            cx.subscribe(
                &shared.account,
                |this, _, event: &AccountEvent, cx| match event {
                    // a new account starts on Help, greeted
                    AccountEvent::Welcome => this.open_help(cx),
                    // its dialog closes (`Account` is the app's, the dialog
                    // the console's)
                    AccountEvent::Approved => {
                        if let Some(own) = this.console.and_then(|key| this.by_window.get(&key)) {
                            own.overlays
                                .update(cx, |overlays, cx| overlays.close(Overlay::Approve, cx));
                        }
                    }
                    AccountEvent::Toast(_) => {}
                },
            ),
            // the launcher and the desk are one window: crossing from one to
            // the other takes the other's size
            cx.observe(&shared.screen, |this, screen, cx| {
                let now = *screen.read(cx).get() != Screen::Desk;
                if now == this.launcher {
                    return;
                }
                this.launcher = now;
                if !now {
                    crate::perf::mark("desk");
                }
                this.swap_console(cx);
            }),
            cx.observe(&shared.prefs, |this, prefs, cx| {
                let appearance = prefs.read(cx).get().appearance;
                if appearance != this.appearance {
                    this.appearance = appearance;
                    this.sync_appearance(cx);
                }
            }),
        ];
        Self {
            shared,
            center,
            handles: BTreeMap::new(),
            views: BTreeMap::new(),
            by_window: BTreeMap::new(),
            console: None,
            front: None,
            active: None,
            desk_bounds: None,
            launcher,
            appearance,
            subscriptions,
        }
    }

    /// Every entity, this one included: what a window's root is made over.
    pub(crate) fn entities(&self, cx: &Context<Self>) -> Entities {
        Entities {
            shared: self.shared.clone(),
            windows: cx.entity(),
        }
    }

    pub(crate) fn console(&self) -> Option<WindowKey> {
        self.console
    }

    pub(crate) fn active(&self) -> Option<&'static str> {
        self.active
    }

    /// Window `key`'s own entities; none for a window that is not ours.
    pub(crate) fn own(&self, key: WindowKey) -> Option<&WindowEntities> {
        self.by_window.get(&key)
    }

    /// Every window's own entities, by key.
    pub(crate) fn by_window(&self) -> &BTreeMap<WindowKey, WindowEntities> {
        &self.by_window
    }

    pub(crate) fn handles(&self) -> &BTreeMap<WindowKey, AnyWindowHandle> {
        &self.handles
    }

    /// The console's own entities, once it is open.
    fn console_own(&self) -> Option<&WindowEntities> {
        self.console.and_then(|key| self.by_window.get(&key))
    }

    /// The windows as the AX door names them: the first "console", the
    /// rest "console2", "console3", …
    pub(crate) fn served(&self) -> Vec<crate::ax::Served> {
        self.handles
            .iter()
            .enumerate()
            .map(|(nth, (key, handle))| {
                let name = match nth {
                    0 => "console".to_owned(),
                    nth => format!("console{}", nth + 1),
                };
                (name, *key, *handle)
            })
            .collect()
    }

    // ---------- opening, raising, closing ----------

    /// A window of `kind`, opened at `at` (`None`: the display's centre).
    /// Its key and its own entities exist now; the OS window follows
    /// (deferred: the caller is most often mid-update).
    pub(crate) fn open(
        &mut self,
        kind: WindowKind,
        at: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) -> WindowKey {
        let _timed = timed();
        let key = WindowKey::unique();
        self.make_own(key, cx);
        if kind == WindowKind::Console {
            self.console = Some(key);
        }
        self.open_window(key, kind, at, cx);
        cx.notify();
        key
    }

    /// Window `key`'s own entities, and what follows them here: the
    /// program in front and the notification centre follow its desk;
    /// anything opening over the desk lets go of the keyboard's hold, and
    /// "Add a device…" closing, however it did, forgets what it found.
    fn make_own(&mut self, key: WindowKey, cx: &mut Context<Self>) {
        let desk = cx.new(|_| Slice::new(Layout::default()));
        let session = self.shared.session.clone();
        let own = WindowEntities {
            front: Front::of_desk(&desk, cx),
            overlays: cx.new(|cx| Overlays::new(&session, cx)),
            spotlight: cx.new(|_| Slice::new(Spotlight::default())),
            dot: cx.new(|_| Slice::new(None)),
            desk,
        };
        self.subscriptions
            .push(cx.observe(&own.desk, move |this, desk, cx| {
                // the window in front is the active program, however it got
                // there (a frame or a fill moves no front: `shown` stands)
                if let Some(module) = desk.read(cx).get().shown()
                    && this.active != Some(module)
                {
                    this.active = Some(module);
                    cx.notify();
                }
                this.set_front(key, cx);
            }));
        let (desk, account) = (own.desk.clone(), self.shared.account.clone());
        let mut was = None;
        self.subscriptions
            .push(cx.observe(&own.overlays, move |_, overlays, cx| {
                let open = *overlays.read(cx).get();
                let before = std::mem::replace(&mut was, open);
                if before == Some(Overlay::Approve) && open != before {
                    account.update(cx, |account, cx| account.approve_closed(cx));
                }
                if open.is_some() && desk.read(cx).get().held.is_some() {
                    desk.update(cx, |desk, cx| desk.release(true, cx));
                }
            }));
        self.by_window.insert(key, own);
    }

    /// Opens the OS window for `key` (deferred: the caller is mid-update)
    /// and remembers its handle and view. On failure a pane on its way
    /// there goes back to the desk and the window is forgotten.
    fn open_window(
        &mut self,
        key: WindowKey,
        kind: WindowKind,
        at: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::{TitlebarOptions, WindowOptions, point};
        let title = match kind {
            WindowKind::Console => "Ducktape".to_owned(),
            WindowKind::View { module } => {
                crate::shell::panes::label(self.shared.rail.read(cx).rows(), module)
            }
        };
        // the launcher is a small window of a fixed size; the desk grows
        let launcher = kind == WindowKind::Console && self.launcher;
        let extent = match kind {
            WindowKind::Console if launcher => {
                size(px(layers::LAUNCHER_SIZE.0), px(layers::LAUNCHER_SIZE.1))
            }
            WindowKind::Console | WindowKind::View { .. } => {
                size(px(windows::WINDOW_SIZE.0), px(windows::WINDOW_SIZE.1))
            }
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(
                at.unwrap_or_else(|| Bounds::centered(None, extent, cx)),
            )),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                appears_transparent: cfg!(target_os = "macos"),
                // the buttons are 14px tall: centred in the 36px bar
                traffic_light_position: Some(point(px(11.), px(11.))),
            }),
            // one window serves the launcher and the desk, so it resizes;
            // a pop-out goes down to its view's minimum, as it stands when
            // the window opens (there is no setting it later)
            window_min_size: Some(match kind {
                WindowKind::Console => size(px(720.), px(480.)),
                WindowKind::View { module } => {
                    size(px(windows::popout_min(module)), px(windows::POPOUT_MIN))
                }
            }),
            app_id: Some("dev.ducktape.app".into()),
            kind: gpui_kit::WindowKind::Normal,
            icon: image::RgbaImage::from_raw(
                128,
                128,
                include_bytes!("../../../assets/icon.rgba").to_vec(),
            )
            .map(std::sync::Arc::new),
            ..Default::default()
        };
        let entities = self.entities(cx);
        let this = cx.entity();
        cx.defer(move |cx| {
            let mut opened_view = None;
            let (root_entities, windows) = (entities.clone(), this.clone());
            let opened = cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| WindowRoot::new(root_entities, key, kind, window, cx));
                opened_view = Some(view.downgrade());
                windows.update(cx, |this, cx| this.watch_activation(key, window, cx));
                window.on_window_should_close(cx, move |window, cx| {
                    release_window_input(window, cx);
                    true
                });
                cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
            });
            match opened {
                Ok(handle) => {
                    crate::perf::mark("window");
                    this.update(cx, |this, cx| {
                        this.handles.insert(key, handle.into());
                        if let Some(view) = opened_view {
                            this.views.insert(key, view);
                        }
                        // `Seats` hears the list move: a popped-out pane's
                        // seat moves into its window now
                        cx.notify();
                    });
                }
                Err(error) => {
                    tracing::error!(target: "ducktape::app", reason = "native_window_open_failed", %error, "window could not be opened");
                    this.update(cx, |this, cx| {
                        let error = format!("The window could not be opened: {error}");
                        entities
                            .session
                            .update(cx, |session, cx| session.note_error(error, cx));
                        // a pane on its way to this window goes back to the desk
                        this.pop_in(key, cx);
                        this.closed(key, cx);
                    });
                }
            }
        });
    }

    /// Window `key` gains or loses the front: the front moves, the keys
    /// leaving a window end a hold on one of its panes, and the
    /// notification centre hears which window is in front.
    fn watch_activation(&mut self, key: WindowKey, window: &mut Window, cx: &mut Context<Self>) {
        self.subscriptions.push(
            cx.observe_window_activation(window, move |this, window, cx| {
                this.activation(key, window.is_window_active(), cx)
            }),
        );
        self.activation(key, window.is_window_active(), cx);
    }

    fn activation(&mut self, key: WindowKey, active: bool, cx: &mut Context<Self>) {
        let front = match active {
            true => Some(key),
            false => self.front.filter(|front| *front != key),
        };
        if front != self.front {
            self.front = front;
            cx.notify();
        }
        if !active && let Some(own) = self.by_window.get(&key) {
            own.desk.update(cx, |desk, cx| desk.drop_hold(cx));
        }
        self.set_front(key, cx);
    }

    /// Tells the notification centre whether window `key` is in front and
    /// which view is focused in it: that view's banners stay away (unless
    /// asked for).
    fn set_front(&self, key: WindowKey, cx: &App) {
        let Some(own) = self.by_window.get(&key) else {
            return;
        };
        let layout = own.desk.read(cx).get();
        let focused = layout
            .panes
            .get(layout.focused)
            .map_or(layout::EMPTY, |pane| pane.module);
        self.center
            .lock()
            .set_front(key, self.front == Some(key), focused);
    }

    /// Window `key` to the front. A frame follows even when it is already
    /// active (the switch is timed to it).
    pub(crate) fn raise(&mut self, key: WindowKey, cx: &mut Context<Self>) {
        let _timed = timed();
        if let Some(view) = self.views.get(&key) {
            let _ = view.update(cx, |view, cx| {
                view.start_switch();
                cx.notify()
            });
        }
        if let Some(handle) = self.handles.get(&key) {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
    }

    /// The console brought forward, or opened if there is none (the tray's
    /// Open; a node answering).
    pub(crate) fn raise_console(&mut self, cx: &mut Context<Self>) {
        match self.console {
            Some(key) => self.raise(key, cx),
            None => drop(self.open(WindowKind::Console, None, cx)),
        }
    }

    /// The OS window `key` off the screen (`closed` follows, from the
    /// platform's word that it went).
    fn remove(&self, key: WindowKey, cx: &mut App) {
        if let Some(handle) = self.handles.get(&key) {
            remove(*handle, cx);
        }
    }

    /// The platform says window `id` is gone: it is forgotten, its own
    /// entities with it.
    pub(crate) fn closed_id(&mut self, id: WindowId, cx: &mut Context<Self>) {
        let key = self
            .handles
            .iter()
            .find_map(|(key, handle)| (handle.window_id() == id).then_some(*key));
        if let Some(key) = key {
            self.closed(key, cx);
        }
    }

    fn closed(&mut self, key: WindowKey, cx: &mut Context<Self>) {
        let _timed = timed();
        self.handles.remove(&key);
        self.views.remove(&key);
        self.by_window.remove(&key);
        if self.console == Some(key) {
            self.console = None;
        }
        if self.front == Some(key) {
            self.front = None;
        }
        cx.notify();
    }

    /// Every window off the screen, its input let go first, then the app.
    pub(crate) fn quit(&self, cx: &mut Context<Self>) {
        let windows = self.handles.values().copied().collect::<Vec<_>>();
        cx.defer(move |cx| {
            for handle in windows {
                let _ = handle.update(cx, |_, window, cx| release_window_input(window, cx));
            }
            cx.quit();
        });
    }

    // ---------- panes across windows ----------

    /// Pane `index` of window `key` closed. A pop-out is its one pane: it
    /// goes with it.
    pub(crate) fn close_pane(&mut self, key: WindowKey, index: usize, cx: &mut Context<Self>) {
        let _timed = timed();
        let Some(own) = self.by_window.get(&key) else {
            return;
        };
        own.desk.update(cx, |desk, cx| _ = desk.close(index, cx));
        if self.console != Some(key) {
            self.remove(key, cx);
        }
    }

    /// Pane `index` of window `from` into a window of its own, opened `at`
    /// (where it sat). An empty or Help window has no view to carry out.
    pub(crate) fn pop_out(
        &mut self,
        from: WindowKey,
        index: usize,
        at: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let _timed = timed();
        let Some(own) = self.by_window.get(&from) else {
            return;
        };
        if !own
            .desk
            .read(cx)
            .get()
            .panes
            .get(index)
            .is_some_and(|pane| pane.is_view())
        {
            return;
        }
        let Some(pane) = own.desk.update(cx, |desk, cx| desk.close(index, cx)) else {
            return;
        };
        let kind = WindowKind::View {
            module: pane.module,
        };
        let key = self.open(kind, at, cx);
        self.by_window[&key]
            .desk
            .update(cx, |desk, cx| desk.popin(pane, cx));
    }

    /// Pop-out `key`'s pane back onto the console's desk; its window goes.
    pub(crate) fn pop_in(&mut self, key: WindowKey, cx: &mut Context<Self>) {
        let _timed = timed();
        let Some(console) = self.console.filter(|console| *console != key) else {
            return;
        };
        let (Some(own), Some(console)) = (self.by_window.get(&key), self.by_window.get(&console))
        else {
            return;
        };
        if let Some(pane) = own.desk.update(cx, |desk, cx| desk.close(0, cx)) {
            console.desk.update(cx, |desk, cx| desk.popin(pane, cx));
        }
        self.remove(key, cx);
    }

    // ---------- the console's desk ----------

    /// Show a program on the desk (Spotlight, a menu), as the bar's click
    /// does: into an empty focused window, else the window it is in, else
    /// one of its own.
    pub(crate) fn select_view(&mut self, module: &'static str, cx: &mut Context<Self>) {
        let _timed = timed();
        self.open_seat(module, None, cx);
    }

    /// Help on the desk: into the focused window if it is empty, else
    /// where it already is, else a window of its own. (Whether it greets a
    /// new account is `Account.welcome`.)
    pub(crate) fn open_help(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if let Some(own) = self.console_own() {
            own.desk.update(cx, |desk, cx| desk.open(layout::HELP, cx));
        }
    }

    /// Help asked for (⌘/, a menu): it greets a new account only until
    /// then (`Account.welcome`).
    pub(crate) fn help_asked(&mut self, cx: &mut Context<Self>) {
        self.shared
            .account
            .update(cx, |account, cx| account.help_asked(cx));
        self.open_help(cx);
    }

    /// A link, read against the chain in hand:
    /// `duck://<chain>/<program>/<tail>` on this chain, or the short
    /// `duck://<view>/<route>`: the seat opens and its view is handed the
    /// route. The window coming forward is the answer; a notice only says
    /// what there is nothing to see of.
    pub(crate) fn open_link(&mut self, link: &str, cx: &mut Context<Self>) {
        let _timed = timed();
        let parsed = self.shared.rail.read(cx).parse_link(link);
        match parsed {
            Link::View { module, route } => self.open_seat(module, route, cx),
            Link::Chain(parsed) if !self.shared.rail.read(cx).lists(&parsed.program) => {
                self.notice(format!("No view here opens {} links.", parsed.program), cx);
            }
            Link::Chain(parsed) => {
                let module = crate::runtime::intern(&parsed.program);
                let route = parsed.tail.join("/");
                let chain = self.shared.session.read(cx).get().chain.clone();
                if parsed.chain.to_string() != chain {
                    // another chain's page is not this chain's to show
                    self.open_seat(module, None, cx);
                    self.notice(
                        format!(
                            "That link is to {}, not this network; opened {module}.",
                            parsed.chain.label
                        ),
                        cx,
                    );
                } else if route.is_empty() || crate::runtime::valid_route(&route) {
                    self.open_seat(module, (!route.is_empty()).then_some(route), cx);
                } else {
                    self.open_seat(module, None, cx);
                    self.notice(format!("{module} can't open that part of the link."), cx);
                }
            }
            Link::Web(url) => cx.open_url(&url),
            Link::Unknown => self.notice("This link is not one this app opens.".into(), cx),
        }
    }

    /// A notification centre row picked: its link opened, else its view's
    /// seat brought forward.
    pub(crate) fn open_notice(&mut self, entry: &Entry, cx: &mut Context<Self>) {
        if !entry.link.is_empty() {
            self.open_link(&entry.link, cx);
        } else if self.shared.rail.read(cx).lists(&entry.module) {
            self.open_seat(crate::runtime::intern(&entry.module), None, cx);
        }
    }

    /// `module` beside the view in front on the console's desk, not in
    /// place of it, handed `route` first.
    fn open_seat(&mut self, module: &'static str, route: Option<String>, cx: &mut Context<Self>) {
        if let Some(route) = route {
            crate::runtime::route_to(module, route);
        }
        if self.active != Some(module) {
            self.active = Some(module);
            cx.notify();
        }
        if let Some(own) = self.console_own() {
            own.desk.update(cx, |desk, cx| desk.open(module, cx));
        }
    }

    fn notice(&self, said: String, cx: &mut App) {
        self.shared
            .toast
            .update(cx, |toast, cx| toast.show(said, cx));
    }

    /// The network in hand was left: every window's panes go, each desk
    /// keeping its measure; what is open over each closes; the active
    /// program goes with them.
    fn left_network(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.active.take().is_some() {
            cx.notify();
        }
        for own in self.by_window.values() {
            own.desk.update(cx, |desk, cx| desk.clear(cx));
            own.overlays.update(cx, |overlays, cx| {
                if let Some(open) = *overlays.get() {
                    overlays.close(open, cx);
                }
            });
        }
    }

    /// A link or a page asked for off the window thread (a banner's click,
    /// the passkey page).
    pub(crate) fn posted(&mut self, posted: Posted, cx: &mut Context<Self>) {
        match posted {
            Posted::Link(url) => {
                // the banner's row was read as it was clicked
                self.shared
                    .notifications
                    .update(cx, |notifications, cx| _ = notifications.refresh(cx));
                self.open_link(&url, cx);
            }
            Posted::Url(url) => cx.open_url(&url),
        }
    }

    // ---------- the console's shape ----------

    /// The appearance chosen, onto the app's theme; the OS's word back
    /// into `Prefs`.
    pub(crate) fn sync_appearance(&mut self, cx: &mut Context<Self>) {
        use gpui_kit::component::{Theme, ThemeMode};
        let _timed = timed();
        match self.shared.prefs.read(cx).get().appearance {
            crate::backend::Appearance::Light => Theme::change(ThemeMode::Light, None, cx),
            crate::backend::Appearance::Dark => Theme::change(ThemeMode::Dark, None, cx),
            crate::backend::Appearance::System => Theme::sync_system_appearance(None, cx),
        }
        let dark = Theme::global(cx).is_dark();
        self.shared
            .prefs
            .update(cx, |prefs, cx| prefs.set_system_dark(dark, cx));
        theme::configure_native_theme(cx);
    }

    /// The launcher and the desk are one window: crossing from one to
    /// the other resizes it in place rather than closing it and opening
    /// another. The launcher comes up centred on the window's display; the
    /// desk comes back where it was, at its size, maximized or fullscreen
    /// again if it had been.
    fn swap_console(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.console else {
            return;
        };
        let (Some(handle), Some(view)) = (
            self.handles.get(&key).copied(),
            self.views.get(&key).cloned(),
        ) else {
            return;
        };
        let launcher = self.launcher;
        let desk = self.desk_bounds;
        // deferred: the crossing is often heard from inside this very
        // window's update (a click on Lock), where it can't be updated again
        cx.spawn(async move |this, cx| {
            // the frames the window reports, `None` while still full size
            let (heard, frames) = mpsc::unbounded();
            // a full-size window ignores (or the window manager overrides) a
            // new frame, so leave that state first
            let left = handle
                .update(cx, |_, window, cx| {
                    let bounds = window.window_bounds().get_bounds();
                    let left = if window.is_fullscreen() {
                        window.toggle_fullscreen();
                        WindowBounds::Fullscreen(bounds)
                    } else if window.is_maximized() {
                        // macOS "maximized" is only a size a new frame replaces
                        if !cfg!(target_os = "macos") {
                            window.zoom_window();
                        }
                        WindowBounds::Maximized(bounds)
                    } else {
                        return (WindowBounds::Windowed(bounds), None);
                    };
                    let observing = view
                        .update(cx, |_, cx| {
                            cx.observe_window_bounds(window, move |_, window, _| {
                                let frame = (!window.is_fullscreen() && !window.is_maximized())
                                    .then(|| window.bounds());
                                let _ = heard.unbounded_send(frame);
                            })
                        })
                        .ok();
                    (left, observing)
                })
                .ok();
            // held until the wait is over
            let (mut left, _observing) = left.unzip();
            let full = matches!(left, Some(WindowBounds::Fullscreen(_)))
                || matches!(left, Some(WindowBounds::Maximized(_))) && !cfg!(target_os = "macos");
            if full {
                // the window manager (or macOS's animation) puts back the
                // frame the window had before it went full size: wait for it,
                // and keep it as the frame the desk goes full size from again
                let restored = match left_full_size(frames, cx).await {
                    Some(restored) => Some(restored),
                    // it never said: take the window as it is
                    None => handle
                        .update(cx, |_, window, _| {
                            (!window.is_fullscreen() && !window.is_maximized())
                                .then(|| window.bounds())
                        })
                        .ok()
                        .flatten(),
                };
                if let Some(restored) = restored {
                    left = left.map(|left| match left {
                        WindowBounds::Fullscreen(_) => WindowBounds::Fullscreen(restored),
                        _ => WindowBounds::Maximized(restored),
                    });
                }
            }
            let _ = handle.update(cx, |_, window, cx| {
                let display = window.display(cx).map(|display| display.visible_bounds());
                let centred = |extent: gpui_kit::Size<Pixels>| match display {
                    Some(display) => windows::centered(extent, display),
                    None => Bounds::new(window.bounds().origin, extent),
                };
                let to = match (launcher, desk) {
                    (true, _) => centred(size(
                        px(layers::LAUNCHER_SIZE.0),
                        px(layers::LAUNCHER_SIZE.1),
                    )),
                    (false, Some(desk)) => desk.get_bounds(),
                    (false, None) => {
                        centred(size(px(windows::WINDOW_SIZE.0), px(windows::WINDOW_SIZE.1)))
                    }
                };
                window.set_bounds(to);
                match (launcher, desk) {
                    (false, Some(WindowBounds::Fullscreen(_))) => window.toggle_fullscreen(),
                    (false, Some(WindowBounds::Maximized(_))) if !cfg!(target_os = "macos") => {
                        window.zoom_window()
                    }
                    _ => {}
                }
            });
            if launcher {
                let _ = this.update(cx, |this, _| this.desk_bounds = left);
            }
        })
        .detach();
    }

    // ---------- tests' seams ----------

    /// The program in front, as a test seeds it.
    #[cfg(test)]
    pub(in crate::shell) fn set_active(&mut self, module: Option<&'static str>) {
        self.active = module;
    }

    /// Each window's root view.
    #[cfg(test)]
    pub(in crate::shell) fn views(&self) -> &BTreeMap<WindowKey, WeakEntity<WindowRoot>> {
        &self.views
    }
}

/// How long a window leaving full size has to report its frame.
const LEAVE_FULL_SIZE: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a frame has to stand before it is the one: a window manager
/// can send the state and the frame as separate events, in either order.
const FRAME_SETTLES: std::time::Duration = std::time::Duration::from_millis(100);

/// The frame a window leaving full size went back to, from the frames it
/// reports (`None` while still full size): the last one once no other
/// follows for a moment, or whatever it last said when time runs out.
async fn left_full_size(
    mut frames: UnboundedReceiver<Option<Bounds<Pixels>>>,
    cx: &AsyncApp,
) -> Option<Bounds<Pixels>> {
    let clock = cx.background_executor();
    let give_up = clock.now() + LEAVE_FULL_SIZE;
    let mut last = None;
    loop {
        let left = give_up.saturating_duration_since(clock.now());
        let wait = if last.is_some() {
            FRAME_SETTLES.min(left)
        } else {
            left
        };
        let timer = std::pin::pin!(clock.timer(wait));
        match futures::future::select(frames.next(), timer).await {
            futures::future::Either::Left((Some(frame), _)) => last = frame.or(last),
            _ => return last,
        }
    }
}

// ---------- links and pages asked for off the window thread ----------

/// Something asked for from a thread with no `&mut App`: a banner's click
/// (`runtime::notify`) or the passkey ceremony's page (`backend::auth_page`).
pub(crate) enum Posted {
    /// A link, for `Windows::open_link`.
    Link(String),
    /// A web page, for the system browser.
    Url(String),
}

fn poster() -> &'static Mutex<Option<UnboundedSender<Posted>>> {
    static POSTER: OnceLock<Mutex<Option<UnboundedSender<Posted>>>> = OnceLock::new();
    POSTER.get_or_init(Mutex::default)
}

/// Installs the hooks the runtime and the backend post through, once per
/// process, and hands back what they post for `Windows::posted`.
pub(crate) fn posted() -> UnboundedReceiver<Posted> {
    let (send, receive) = mpsc::unbounded();
    let mut current = poster().lock().expect("the shell's posts");
    assert!(current.is_none(), "one native shell per process");
    // runtime::notify and backend::auth_page cannot depend on shell: they
    // call these hooks instead
    crate::runtime::notify::on_open_link(|url| post(Posted::Link(url)));
    crate::backend::auth_page::on_open_url(|url| post(Posted::Url(url)));
    *current = Some(send);
    receive
}

fn post(posted: Posted) {
    let sent = poster()
        .lock()
        .expect("the shell's posts")
        .as_ref()
        .is_some_and(|sender| sender.unbounded_send(posted).is_ok());
    if !sent {
        tracing::error!(target: "ducktape::app", reason = "native_shell_closed", "a link could not be delivered");
    }
}

#[cfg(test)]
mod swap_tests {
    use super::*;
    use gpui_kit::{point, px, size};

    #[gpui_kit::test]
    async fn a_window_leaving_full_size_is_taken_at_the_frame_it_settles_on(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let frame = |x: f32| Bounds::new(point(px(x), px(0.)), size(px(800.), px(600.)));
        let (heard, frames) = mpsc::unbounded();
        let wait = cx.spawn(async move |cx| left_full_size(frames, &cx).await);
        // the state first, still at full size, then two frames
        for reported in [None, Some(frame(1.)), Some(frame(2.))] {
            heard.unbounded_send(reported).unwrap();
        }
        cx.run_until_parked();
        cx.executor().advance_clock(FRAME_SETTLES);
        assert_eq!(wait.await, Some(frame(2.)));

        // a window that never says: given up on, not waited for forever
        let (_heard, frames) = mpsc::unbounded();
        let wait = cx.spawn(async move |cx| left_full_size(frames, &cx).await);
        cx.run_until_parked();
        cx.executor().advance_clock(LEAVE_FULL_SIZE);
        assert_eq!(wait.await, None);
    }
}
