//! The native chrome, and nothing a network decides: a window, a screen to
//! reach a node and unlock a key, a menu bar of whatever programs that node
//! runs, and one seat that draws the open program's view.

use crate::a11y::Control as _;
use futures::{
    StreamExt as _,
    channel::{mpsc, oneshot},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AppContext as _, AsyncApp, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use view_wire::Task;

use crate::{AppMessage as Message, Ducktape, Screen, Stage};

#[cfg(debug_assertions)]
mod fixtures;
#[cfg(debug_assertions)]
pub(crate) use fixtures::render_tree_fixture;
mod approve;
mod desk;
mod figure;
mod ink;
mod launch;
mod launcher;
mod layout;
mod menubar;
mod menus;
mod notifications;
mod panes;
#[cfg(test)]
mod panes_tests;
mod screens;
#[cfg(test)]
mod screens_tests;
mod spotlight;
mod windows;

pub(crate) use launch::run;
mod settings;
mod sign_in;
mod spin;
mod theme;

#[cfg(not(target_os = "macos"))]
use crate::fonts::EMOJI_FACE;
use crate::fonts::{BUNDLED_FACES, fallback_chain};
use theme::{NARROW_WINDOW_WIDTH, configure_native_theme};

pub(crate) use crate::runtime::WindowKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowKind {
    Console,
    View { module: &'static str },
}

#[derive(Clone, Debug)]
pub(crate) struct KeyPress {
    pub(crate) key: String,
    pub(crate) modifiers: gpui_kit::Modifiers,
}

/// How the platform writes a command chord: "⌘K" on a Mac, "Ctrl K"
/// elsewhere.
pub(crate) fn chord_label(key: &str) -> String {
    match cfg!(target_os = "macos") {
        true => format!("⌘{key}"),
        false => format!("Ctrl {key}"),
    }
}

pub(crate) enum Command {
    Open {
        key: WindowKey,
        kind: WindowKind,
        reply: oneshot::Sender<WindowKey>,
    },
    Raise(WindowKey),
    /// A link pressed off the window thread: the reducer reads it.
    OpenLink(String),
    /// A web page, for the system browser.
    OpenUrl(String),
    Quit,
}

pub(crate) struct PendingCommand {
    pub command: Command,
    pub completed: oneshot::Sender<()>,
}

fn sender() -> &'static Mutex<Option<mpsc::UnboundedSender<PendingCommand>>> {
    static SENDER: OnceLock<Mutex<Option<mpsc::UnboundedSender<PendingCommand>>>> = OnceLock::new();
    SENDER.get_or_init(Mutex::default)
}

pub(crate) fn commands() -> mpsc::UnboundedReceiver<PendingCommand> {
    let (send, receive) = mpsc::unbounded();
    let mut current = sender().lock().expect("native shell commands");
    assert!(current.is_none(), "one native shell per process");
    // the layers below hand these up without naming the shell
    crate::runtime::notify::on_open_link(open_link);
    crate::backend::passkey::on_open_url(open_url_now);
    *current = Some(send);
    receive
}

async fn send(command: Command) {
    let (completed, received) = oneshot::channel();
    let pending = PendingCommand { command, completed };
    let sent = sender()
        .lock()
        .expect("native shell commands")
        .as_ref()
        .is_some_and(|sender| sender.unbounded_send(pending).is_ok());
    if !sent {
        tracing::error!(target: "ducktape::app", reason = "native_shell_closed", "native window command could not be delivered");
        return;
    }
    let _ = received.await;
}

pub(crate) fn open(kind: WindowKind) -> (WindowKey, Task<WindowKey>) {
    let key = WindowKey::unique();
    let task = Task::stream(
        futures::stream::once(async move {
            let (reply, receive) = oneshot::channel();
            send(Command::Open { key, kind, reply }).await;
            receive.await.ok()
        })
        .filter_map(std::future::ready),
    );
    (key, task)
}

fn effect<M: 'static>(command: Command) -> Task<M> {
    Task::future(async move {
        send(command).await;
    })
    .discard()
}

pub(crate) fn raise<M: 'static>(key: WindowKey) -> Task<M> {
    effect(Command::Raise(key))
}

pub(crate) fn quit<M: 'static>() -> Task<M> {
    effect(Command::Quit)
}

/// A web page, in the system browser.
pub(crate) fn open_url<M: 'static>(url: String) -> Task<M> {
    effect(Command::OpenUrl(url))
}

/// A link pressed off the window thread (a banner's click), handed to the
/// reducer as `Message::OpenLink`.
fn open_link(url: String) {
    post(Command::OpenLink(url));
}

/// A web page for the system browser, asked for off the window thread
/// (the passkey ceremony's page).
fn open_url_now(url: String) {
    post(Command::OpenUrl(url));
}

/// A command sent without waiting for it to be done.
fn post(command: Command) {
    let (completed, _dropped) = oneshot::channel();
    let pending = PendingCommand { command, completed };
    let sent = sender()
        .lock()
        .expect("native shell commands")
        .as_ref()
        .is_some_and(|sender| sender.unbounded_send(pending).is_ok());
    if !sent {
        tracing::error!(target: "ducktape::app", reason = "native_shell_closed", "a link could not be delivered");
    }
}

/// A stream that yields every `period`.
pub(crate) fn every(period: std::time::Duration) -> impl futures::Stream<Item = ()> {
    futures::stream::unfold((), move |()| async move {
        tokio::time::sleep(period).await;
        Some(((), ()))
    })
}

// ---------- the desktop actor ----------

struct Desktop {
    state: Ducktape,
    tray: crate::tray::Tray,
    windows: BTreeMap<WindowKey, gpui_kit::AnyWindowHandle>,
    views: BTreeMap<WindowKey, gpui_kit::WeakEntity<DesktopWindow>>,
    streams: HashMap<u64, gpui_kit::Task<()>>,
    /// Where the desk window was when it last gave way to the launcher:
    /// it comes back there.
    desk_bounds: Option<gpui_kit::WindowBounds>,
}

impl Desktop {
    fn ax_windows(&self) -> Vec<(String, gpui_kit::AnyWindowHandle)> {
        let mut nth = 0;
        self.windows
            .values()
            .map(|handle| {
                nth += 1;
                let name = match nth {
                    1 => "console".to_owned(),
                    nth => format!("console{nth}"),
                };
                (name, *handle)
            })
            .collect()
    }

    fn dispatch(&mut self, message: Message, cx: &mut Context<Self>) {
        let runtime = crate::runtime::handle();
        let _runtime = runtime.enter();
        let appearance = self.state.appearance;
        let launcher = self.state.in_launcher();
        let task = self.state.update(message);
        if launcher != self.state.in_launcher() {
            self.swap_console(cx);
        }
        if appearance != self.state.appearance {
            self.sync_appearance(cx);
        }
        if let Some(request) = self.state.seat_request.take() {
            let views: Vec<_> = self.views.values().cloned().collect();
            cx.defer(move |cx| {
                for view in views {
                    let _ = view.update(cx, |view, cx| view.seat(request, cx));
                }
            });
        }
        self.tray.sync(&self.state);
        self.start(task, cx).detach();
        self.subscriptions(cx);
        cx.notify();
    }

    /// The launcher and the desk are one window: crossing from one to
    /// the other resizes it in place rather than closing it and opening
    /// another. The launcher comes up centred on the window's display; the
    /// desk comes back where it was, at its size, maximized or fullscreen
    /// again if it had been.
    fn swap_console(&mut self, cx: &mut Context<Self>) {
        use gpui_kit::WindowBounds;
        let Some(handle) = self
            .state
            .console_win
            .and_then(|key| self.windows.get(&key).copied())
        else {
            return;
        };
        let launcher = self.state.in_launcher();
        let desk = self.desk_bounds;
        // deferred: the crossing is often dispatched from inside this very
        // window's update (a click on Lock), where it can't be updated again
        cx.spawn(async move |desktop, cx| {
            // a full-size window ignores (or the window manager overrides) a
            // new frame, so leave that state first
            let mut left = handle
                .update(cx, |_, window, _| {
                    let bounds = window.window_bounds().get_bounds();
                    if window.is_fullscreen() {
                        window.toggle_fullscreen();
                        WindowBounds::Fullscreen(bounds)
                    } else if window.is_maximized() {
                        // macOS "maximized" is only a size a new frame replaces
                        if !cfg!(target_os = "macos") {
                            window.zoom_window();
                        }
                        WindowBounds::Maximized(bounds)
                    } else {
                        WindowBounds::Windowed(bounds)
                    }
                })
                .ok();
            let full = matches!(left, Some(WindowBounds::Fullscreen(_)))
                || matches!(left, Some(WindowBounds::Maximized(_))) && !cfg!(target_os = "macos");
            if full {
                // the window manager (or macOS's animation) puts back the
                // frame the window had before it went full size: wait for it,
                // and keep it as the frame the desk goes full size from again
                // ponytail: polled, a window-bounds observer if this ever shows
                let mut last = None;
                for _ in 0..60 {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(50))
                        .await;
                    let now = handle
                        .update(cx, |_, window, _| {
                            (!window.is_fullscreen() && !window.is_maximized())
                                .then(|| window.bounds())
                        })
                        .ok()
                        .flatten();
                    if now.is_some() && now == last {
                        break;
                    }
                    last = now;
                }
                if let Some(restored) = last {
                    left = left.map(|left| match left {
                        WindowBounds::Fullscreen(_) => WindowBounds::Fullscreen(restored),
                        _ => WindowBounds::Maximized(restored),
                    });
                }
            }
            let _ = handle.update(cx, |_, window, cx| {
                let display = window.display(cx).map(|display| display.visible_bounds());
                let centred = |extent: gpui_kit::Size<gpui_kit::Pixels>| match display {
                    Some(display) => windows::centered(extent, display),
                    None => gpui_kit::Bounds::new(window.bounds().origin, extent),
                };
                let to = match (launcher, desk) {
                    (true, _) => centred(gpui_kit::size(
                        gpui_kit::px(launcher::LAUNCHER_SIZE.0),
                        gpui_kit::px(launcher::LAUNCHER_SIZE.1),
                    )),
                    (false, Some(desk)) => desk.get_bounds(),
                    (false, None) => centred(gpui_kit::size(
                        gpui_kit::px(windows::WINDOW_SIZE.0),
                        gpui_kit::px(windows::WINDOW_SIZE.1),
                    )),
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
                let _ = desktop.update(cx, |this, _| this.desk_bounds = left);
            }
        })
        .detach();
    }

    fn sync_appearance(&mut self, cx: &mut Context<Self>) {
        use gpui_kit::component::{Theme, ThemeMode};
        match self.state.appearance {
            crate::Appearance::Light => Theme::change(ThemeMode::Light, None, cx),
            crate::Appearance::Dark => Theme::change(ThemeMode::Dark, None, cx),
            crate::Appearance::System => Theme::sync_system_appearance(None, cx),
        }
        self.state.system_dark = Theme::global(cx).is_dark();
        configure_native_theme(cx);
    }

    fn start(&self, task: Task<Message>, cx: &mut Context<Self>) -> gpui_kit::Task<()> {
        let mut stream = task.into_stream();
        let runtime = crate::runtime::handle();
        cx.spawn(async move |desktop, cx| {
            loop {
                let message = futures::future::poll_fn(|context| {
                    let _runtime = runtime.enter();
                    stream.poll_next_unpin(context)
                })
                .await;
                let Some(message) = message else {
                    break;
                };
                if desktop
                    .update(cx, |this, cx| this.dispatch(message, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    fn subscriptions(&mut self, cx: &mut Context<Self>) {
        let runtime = crate::runtime::handle();
        let _runtime = runtime.enter();
        let recipes = self.state.subscriptions().into_recipes();
        self.streams
            .retain(|key, _| recipes.iter().any(|recipe| recipe.key == *key));
        for recipe in recipes {
            if self.streams.contains_key(&recipe.key) {
                continue;
            }
            let stream = (recipe.start)();
            let task = self.start(Task::stream(stream), cx);
            self.streams.insert(recipe.key, task);
        }
    }

    fn execute(&mut self, command: Command, cx: &mut Context<Self>) {
        match command {
            Command::Open { key, kind, reply } => {
                self.open_window(key, kind, reply, None, None, cx)
            }
            Command::Raise(key) => self.raise_window(key, cx),
            Command::OpenLink(link) => self.dispatch(Message::OpenLink(link), cx),
            Command::OpenUrl(url) => cx.open_url(&url),
            Command::Quit => self.quit(cx),
        }
    }

    fn raise_window(&mut self, key: WindowKey, cx: &mut Context<Self>) {
        if let Some(handle) = self.windows.get(&key) {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
    }

    fn quit(&mut self, cx: &mut Context<Self>) {
        let windows = self.windows.values().copied().collect::<Vec<_>>();
        cx.defer(move |cx| {
            for handle in windows {
                let _ = handle.update(cx, |_, window, cx| release_window_input(window, cx));
            }
            cx.quit();
        });
    }
}

fn release_window_input(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    window.blur(cx);
    window.draw(cx).clear(cx);
}

/// Takes a window off the screen, its input let go first. Deferred:
/// releasing input draws the window, and the caller is most often in the
/// middle of updating it. `on_window_closed` (launch.rs) then forgets it.
fn remove(window: gpui_kit::AnyWindowHandle, cx: &mut gpui_kit::App) {
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, cx| {
            release_window_input(window, cx);
            window.remove_window();
        });
    });
}

// ---------- the window ----------

pub(crate) struct DesktopWindow {
    model: Entity<Desktop>,
    key: WindowKey,
    kind: WindowKind,
    layout: layout::Layout,
    mounted: BTreeMap<u64, panes::MountedPane>,
    initialized: bool,
    drag: Option<panes::Drag>,
    inputs: HashMap<&'static str, NativeInput>,
    /// ⌘K's field took focus when it opened; it is not taken again while
    /// Spotlight stays open.
    spotlight_focused: bool,
    /// Where the bar's menu buttons were last painted: each menu hangs
    /// under its own.
    bar_buttons: HashMap<crate::Overlay, gpui_kit::Bounds<gpui_kit::Pixels>>,
    focus: gpui_kit::FocusHandle,
    _activation: gpui_kit::Subscription,
    _observer: gpui_kit::Subscription,
    _keystrokes: gpui_kit::Subscription,
    _focus_lost: gpui_kit::Subscription,
}

struct NativeInput {
    state: Entity<gpui_kit::component::input::InputState>,
    /// A digest of the model text the field last agreed with — what it
    /// sent on its last change, or what the model last pushed into it.
    mirrored: std::rc::Rc<std::cell::Cell<u64>>,
    _subscription: gpui_kit::Subscription,
}

impl DesktopWindow {
    fn intercept_global_keys(
        window: &gpui_kit::Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Subscription {
        let window_id = window.window_handle().window_id();
        let view = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != window_id {
                return;
            }
            let in_guest_editor = event
                .context_stack
                .iter()
                .any(|context| context.contains(crate::editor::wire::GUEST_EDITOR_CONTEXT));
            let _ = view.update(cx, |view, cx| {
                view.global_key(
                    KeyPress {
                        key: event.keystroke.key.clone(),
                        modifiers: event.keystroke.modifiers,
                    },
                    in_guest_editor,
                    window,
                    cx,
                );
            });
        })
    }

    /// ⌘Q quits, then ⌘W closes (`command_w_pane`: the focused desk
    /// window, else the app's); the desk's own keys (`desk_key`) come next;
    /// any other command chord goes to the seated view if it claimed it.
    fn global_key(
        &mut self,
        key: KeyPress,
        in_guest_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = crate::runtime::command_held(key.modifiers);
        if command && key.key == "q" {
            self.model
                .update(cx, |model, cx| model.dispatch(Message::TrayQuit, cx));
            cx.stop_propagation();
            return;
        }
        if command && key.key == "w" {
            match self.command_w_pane(cx) {
                Some(index) => self.pane_message(panes::PaneMessage::Close(index), window, cx),
                None => self.close_by_key(window, cx),
            }
            cx.stop_propagation();
            return;
        }
        if self.desk_keys(cx) && self.desk_key(&key, in_guest_editor, window, cx) {
            cx.stop_propagation();
            return;
        }
        if command && key.key == "k" && self.kind == WindowKind::Console && self.on_desk(cx) {
            let message = match self.model.read(cx).state.overlay {
                Some(crate::Overlay::Spotlight) => Message::CloseOverlay(crate::Overlay::Spotlight),
                _ => Message::OpenSpotlight,
            };
            self.model
                .update(cx, |model, cx| model.dispatch(message, cx));
            cx.stop_propagation();
            return;
        }
        // Escape closes whatever is open over the desk
        let overlay = self.model.read(cx).state.overlay;
        if key.key == "escape"
            && self.kind == WindowKind::Console
            && let Some(overlay) = overlay
        {
            self.model.update(cx, |model, cx| {
                model.dispatch(Message::CloseOverlay(overlay), cx)
            });
            cx.stop_propagation();
            return;
        }
        if !in_guest_editor && self.deliver_chord(&key, cx) {
            cx.stop_propagation();
        }
    }

    /// On the desk: connected, and past the key and account steps.
    fn on_desk(&self, cx: &gpui_kit::App) -> bool {
        !self.model.read(cx).state.in_launcher()
    }

    /// The desk's own keys reach its windows: the console, on the desk,
    /// with no overlay (Spotlight, a menu, Settings) keeping its keys.
    fn desk_keys(&self, cx: &gpui_kit::App) -> bool {
        self.kind == WindowKind::Console
            && self.on_desk(cx)
            && self.model.read(cx).state.overlay.is_none()
    }

    /// What ⌘W closes: the focused desk window, when the desk's keys reach
    /// it and it has one; `None` is the app's window (`close_by_key`).
    fn command_w_pane(&self, cx: &gpui_kit::App) -> Option<usize> {
        (self.desk_keys(cx) && !self.layout.panes.is_empty()).then_some(self.layout.focused)
    }

    /// ⌘W closes a window, never the app. A pop-out closes as its pane's ×
    /// does. The console closes too where the status item reopens it
    /// (macOS); elsewhere there is no tray to bring it back from, and the
    /// last window closing would quit — so it minimizes instead.
    fn close_by_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match (self.kind, cfg!(target_os = "macos")) {
            (WindowKind::Console, false) => window.minimize_window(),
            _ => remove(window.window_handle(), cx),
        }
    }

    /// The window's focused element vanished — most often because the
    /// screen it was on gave way to another (Connect → sign-in, sign-in →
    /// recovery phrase, phrase → console, a dialog opening or closing
    /// mid-form). Refocus the window's own root (the same handle a fresh
    /// window starts on) so a keyboard-only reader's next Tab still lands
    /// on the new screen's first control, instead of the window going
    /// silently blurred with no dispatch path for Tab, Enter or Escape to
    /// reach at all.
    fn focus_lost(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
    }

    fn deliver_chord(&mut self, key: &KeyPress, cx: &mut Context<Self>) -> bool {
        let Some(chord) = crate::runtime::chord_of(&key.key, key.modifiers) else {
            return false;
        };
        let Some(view) = self.focused_view() else {
            return false;
        };
        let landed = view.update(cx, |view, cx| view.chord(&chord, cx));
        if landed {
            cx.notify();
        }
        landed
    }

    fn released(&mut self, cx: &mut gpui_kit::App) {
        self.unseat(cx);
        self.observe_window(view_wire::events::Window::Closed, cx);
    }

    /// The model's ask of the desk: the console's windows follow it.
    fn seat(&mut self, request: crate::SeatRequest, cx: &mut Context<Self>) {
        use crate::SeatRequest;
        match request {
            SeatRequest::Unseat => return self.unseat(cx),
            _ if self.kind != WindowKind::Console => return,
            SeatRequest::Select(module) => self.layout.select(module),
            // a link opens beside the view it was in, not in place of it
            SeatRequest::Open(module) => self.layout.open(module),
        };
        self.initialized = true;
        cx.notify();
    }

    fn unseat(&mut self, cx: &mut gpui_kit::App) {
        let mounted = std::mem::take(&mut self.mounted);
        self.layout = layout::Layout::default();
        self.initialized = false;
        for (_, pane) in mounted {
            self.hide_pane(pane, cx);
        }
    }

    fn observe_window(&mut self, event: view_wire::events::Window, cx: &mut gpui_kit::App) {
        for pane in self.mounted.values() {
            let intents = pane.view.update(cx, |view, cx| {
                view.observe_final_window_event(event.clone(), cx)
            });
            for intent in intents {
                self.model.update(cx, |model, cx| {
                    model.dispatch(Message::ViewEvent(pane.module, intent), cx)
                });
            }
        }
    }
}

impl Render for DesktopWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::InteractiveElement as _;
        let content = match self.kind {
            WindowKind::View { .. } => self.console(window, cx),
            WindowKind::Console => {
                let state = self.model.read(cx).state.clone_facts();
                match self.model.read(cx).state.stage() {
                    Stage::Connect => self.connect(window, cx),
                    Stage::Phrase => self.phrase(&state, window, cx),
                    Stage::Unlock => self.unlock(&state, window, cx),
                    Stage::Recover => self.recover(&state, window, cx),
                    Stage::Account => self.account_step(&state, window, cx),
                    Stage::Desk => self.console(window, cx),
                }
            }
        };
        let ink = ink::Ink::of(self.model.read(cx).state.dark());
        let mut root = gpui_kit::div();
        root.text_style().font_fallbacks = Some(fallback_chain());
        root.text_style().font_family = Some(theme::FAMILY_UI.into());
        root.id("desktop-root")
            .size_full()
            .bg(ink.bg)
            .text_color(ink.ink)
            .track_focus(&self.focus)
            .on_modifiers_changed(cx.listener(
                |this, event: &gpui_kit::ModifiersChangedEvent, _, cx| {
                    this.model.update(cx, |model, cx| {
                        model.dispatch(Message::ModifierStateChanged(event.modifiers), cx)
                    });
                },
            ))
            .child(content)
    }
}
