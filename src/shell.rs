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
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use view_wire::Task;

use crate::{AppMessage as Message, Ducktape, Screen};

#[cfg(debug_assertions)]
mod fixtures;
#[cfg(debug_assertions)]
pub(crate) use fixtures::render_tree_fixture;
mod desk;
mod figure;
mod launch;
mod launcher;
mod layout;
mod panes;
#[cfg(test)]
#[path = "shell/panes_tests.rs"]
mod panes_tests;
mod screens;
#[cfg(test)]
#[path = "shell/screens_tests.rs"]
mod screens_tests;
mod windows;

pub(crate) use launch::run;
mod settings;
mod sign_in;
mod switcher;
mod theme;

#[cfg(not(target_os = "macos"))]
use theme::EMOJI_FACE;
use theme::{BUNDLED_FACES, NARROW_WINDOW_WIDTH, configure_native_theme, hsla_of};
pub(crate) use theme::{fallback_chain, refine_fallbacks};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct WindowKey(u64);

impl WindowKey {
    pub(crate) fn unique() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowKind {
    Console,
    View {
        module: &'static str,
    },
    /// The app's settings, floating over the desk.
    Settings,
}

#[derive(Clone, Debug)]
pub(crate) struct KeyPress {
    pub(crate) key: String,
    pub(crate) modifiers: gpui_kit::Modifiers,
}

pub(crate) enum Command {
    Open {
        key: WindowKey,
        kind: WindowKind,
        reply: oneshot::Sender<WindowKey>,
    },
    Close(WindowKey),
    Raise(WindowKey),
    OpenLink(String),
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

#[allow(dead_code)]
pub(crate) fn close<M: 'static>(key: WindowKey) -> Task<M> {
    effect(Command::Close(key))
}

pub(crate) fn raise<M: 'static>(key: WindowKey) -> Task<M> {
    effect(Command::Raise(key))
}

pub(crate) fn quit<M: 'static>() -> Task<M> {
    effect(Command::Quit)
}

/// A link a view or an editor pressed, handed to the app off any thread.
pub(crate) fn open_link(url: String) {
    let (completed, _dropped) = oneshot::channel();
    let pending = PendingCommand {
        command: Command::OpenLink(url),
        completed,
    };
    let sent = sender()
        .lock()
        .expect("native shell commands")
        .as_ref()
        .is_some_and(|sender| sender.unbounded_send(pending).is_ok());
    if !sent {
        tracing::error!(target: "ducktape::app", reason = "native_shell_closed", "a pressed link could not be delivered");
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
    desk_bounds: Option<gpui_kit::Bounds<gpui_kit::Pixels>>,
}

impl Desktop {
    fn ax_windows(&self, cx: &gpui_kit::App) -> Vec<(String, gpui_kit::AnyWindowHandle)> {
        let settings = |key: &WindowKey| {
            self.views
                .get(key)
                .and_then(|view| view.upgrade())
                .is_some_and(|view| view.read(cx).kind == WindowKind::Settings)
        };
        let mut nth = 0;
        self.windows
            .iter()
            .map(|(key, handle)| {
                if settings(key) {
                    return ("settings".to_owned(), *handle);
                }
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
        let active = self.state.active;
        let launcher = self.state.in_launcher();
        let task = self.state.update(message);
        if launcher != self.state.in_launcher() {
            self.swap_console(cx);
        }
        if appearance != self.state.appearance {
            self.sync_appearance(cx);
        }
        if active != self.state.active {
            if let Some(module) = self.state.active {
                let views: Vec<_> = self.views.values().cloned().collect();
                cx.defer(move |cx| {
                    for view in views {
                        let _ = view.update(cx, |view, cx| {
                            if view.kind == WindowKind::Console {
                                view.layout.select(module);
                                view.initialized = true;
                                cx.notify();
                            }
                        });
                    }
                });
            } else {
                let views: Vec<_> = self.views.values().cloned().collect();
                cx.defer(move |cx| {
                    for view in views {
                        let _ = view.update(cx, |view, cx| view.unseat(cx));
                    }
                });
            }
        }
        self.tray.sync(&self.state);
        self.start(task, cx).detach();
        self.subscriptions(cx);
        cx.notify();
    }

    /// The launcher and the desk are two windows, the way a game client
    /// signs in before its main window opens: crossing from one to the
    /// other opens the next (centred, or where the desk last was) and
    /// closes the last.
    fn swap_console(&mut self, cx: &mut Context<Self>) {
        let Some(old) = self.state.console_win else {
            return;
        };
        if self.state.in_launcher()
            && let Some(handle) = self.windows.get(&old)
        {
            self.desk_bounds = handle.update(cx, |_, window, _| window.bounds()).ok();
        }
        let key = WindowKey::unique();
        self.state.console_win = Some(key);
        let at = match self.state.in_launcher() {
            true => None,
            false => self.desk_bounds,
        };
        let (reply, _) = oneshot::channel();
        self.open_window(key, WindowKind::Console, reply, None, at, cx);
        self.close_window(old, cx);
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
            Command::Close(key) => self.close_window(key, cx),
            Command::Raise(key) => self.raise_window(key, cx),
            Command::OpenLink(url) => match url.starts_with("duck://") {
                true => self.dispatch(Message::OpenLink(url), cx),
                false => cx.open_url(&url),
            },
            Command::Quit => self.quit(cx),
        }
    }

    fn close_window(&mut self, key: WindowKey, cx: &mut Context<Self>) {
        let Some(handle) = self.windows.remove(&key) else {
            return;
        };
        if let Some(view) = self.views.remove(&key) {
            let _ = view.update(cx, |this, cx| {
                this.observe_window(view_wire::events::Window::CloseRequested, cx)
            });
        }
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                release_window_input(window, cx);
                window.remove_window();
            });
        });
        self.dispatch(Message::WindowWasClosed(key), cx);
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

// ---------- the window ----------

pub(crate) struct DesktopWindow {
    model: Entity<Desktop>,
    key: WindowKey,
    kind: WindowKind,
    layout: layout::Layout,
    mounted: BTreeMap<u64, panes::MountedPane>,
    initialized: bool,
    resize: Option<(usize, f32, f32, f32)>,
    measured_widths: std::rc::Rc<std::cell::RefCell<Vec<f32>>>,
    inputs: HashMap<&'static str, NativeInput>,
    /// ⌘K's field took focus when it opened; it is not taken again while
    /// Spotlight stays open.
    spotlight_focused: bool,
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

    /// ⌘Q quits and ⌘W closes; any other command chord goes to the seated
    /// view if it claimed it.
    fn global_key(
        &mut self,
        key: KeyPress,
        in_guest_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = crate::backend::command_held(key.modifiers);
        if command && key.key == "q" {
            self.model
                .update(cx, |model, cx| model.dispatch(Message::TrayQuit, cx));
            cx.stop_propagation();
            return;
        }
        if command && key.key == "w" {
            self.close_by_key(window, cx);
            cx.stop_propagation();
            return;
        }
        if command && key.key == "k" && self.kind == WindowKind::Console && self.on_desk(cx) {
            let message = match self.model.read(cx).state.spotlight {
                true => Message::CloseSpotlight,
                false => Message::OpenSpotlight,
            };
            self.model
                .update(cx, |model, cx| model.dispatch(message, cx));
            cx.stop_propagation();
            return;
        }
        if key.key == "escape" && self.model.read(cx).state.popover.is_some() {
            self.model
                .update(cx, |model, cx| model.dispatch(Message::ClosePopover, cx));
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

    /// ⌘W closes a window, never the app. A pop-out closes as its pane's ×
    /// does. The console closes too where the status item reopens it
    /// (macOS); elsewhere there is no tray to bring it back from, and the
    /// last window closing would quit — so it minimizes instead.
    fn close_by_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match (self.kind, cfg!(target_os = "macos")) {
            (WindowKind::Console, false) => window.minimize_window(),
            // deferred: releasing input draws the window, and this view is
            // mid-update
            _ => {
                let handle = window.window_handle();
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, cx| {
                        release_window_input(window, cx);
                        window.remove_window();
                    });
                });
            }
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
        use gpui_kit::component::ActiveTheme as _;
        let content = match self.kind {
            WindowKind::Settings => self.settings(window, cx),
            WindowKind::View { .. } => self.console(window, cx),
            WindowKind::Console => {
                let state = self.model.read(cx).state.clone_facts();
                match self.model.read(cx).state.screen {
                    Screen::Connect => self.connect(window, cx),
                    Screen::Console => match (
                        !state.phrase.is_empty(),
                        state.signer_key.is_empty() && !state.browsing,
                        state.restoring,
                    ) {
                        (true, _, _) => self.phrase(&state, window, cx),
                        (_, true, true) => self.restore(&state, window, cx),
                        (_, true, false) => self.unlock(&state, window, cx),
                        _ if state.account_step && !state.signer_key.is_empty() => {
                            self.account_step(&state, window, cx)
                        }
                        _ => self.console(window, cx),
                    },
                }
            }
        };
        let mut root = gpui_kit::div();
        root.text_style().font_fallbacks = Some(fallback_chain());
        root.id("desktop-root")
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
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
