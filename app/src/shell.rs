//! The native chrome, and nothing a network decides: a window, a screen to
//! reach a node and unlock a key, a rail of whatever programs that node
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

mod screens;
mod sign_in;
mod theme;

#[cfg(not(target_os = "macos"))]
use theme::EMOJI_FACE;
use theme::{BUNDLED_FACES, RAIL_WIDTH, configure_native_theme, hsla_of};
pub(crate) use theme::{fallback_chain, mono_family, with_family};

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
}

impl Desktop {
    fn ax_windows(&self, _cx: &gpui_kit::App) -> Vec<(String, gpui_kit::AnyWindowHandle)> {
        self.windows
            .iter()
            .enumerate()
            .map(|(nth, (_, handle))| {
                let name = match nth {
                    0 => "console".to_owned(),
                    nth => format!("console{}", nth + 1),
                };
                (name, *handle)
            })
            .collect()
    }

    fn dispatch(&mut self, message: Message, cx: &mut Context<Self>) {
        let runtime = crate::module_view::runtime();
        let _runtime = runtime.enter();
        let appearance = self.state.appearance;
        let task = self.state.update(message);
        if appearance != self.state.appearance {
            self.sync_appearance(cx);
        }
        self.tray.sync(&self.state);
        self.start(task, cx).detach();
        self.subscriptions(cx);
        cx.notify();
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
        let runtime = crate::module_view::runtime();
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
        let runtime = crate::module_view::runtime();
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
            Command::Open { key, kind, reply } => self.open_window(key, kind, reply, cx),
            Command::Close(key) => self.close_window(key, cx),
            Command::Raise(key) => self.raise_window(key, cx),
            Command::OpenLink(url) => match url.starts_with("duck://") {
                true => self.dispatch(Message::OpenLink(url), cx),
                false => cx.open_url(&url),
            },
            Command::Quit => self.quit(cx),
        }
    }

    fn open_window(
        &mut self,
        key: WindowKey,
        kind: WindowKind,
        reply: oneshot::Sender<WindowKey>,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::*;
        let crate::shell::WindowKind::Console = kind;
        let size = size(px(1280.0), px(800.0));
        let model = cx.entity();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
            titlebar: Some(TitlebarOptions {
                title: (!cfg!(target_os = "macos")).then(|| "Ducktape".into()),
                appears_transparent: cfg!(target_os = "macos"),
                traffic_light_position: Some(point(px(12.), px(12.))),
            }),
            window_min_size: Some(gpui_kit::size(px(720.), px(480.))),
            is_resizable: true,
            app_id: Some("dev.ducktape.app".into()),
            kind: gpui_kit::WindowKind::Normal,
            icon: image::RgbaImage::from_raw(
                128,
                128,
                include_bytes!("../assets/icon.rgba").to_vec(),
            )
            .map(std::sync::Arc::new),
            ..Default::default()
        };
        cx.defer(move |cx| {
            let mut opened_view = None;
            let window_model = model.clone();
            let opened = cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| {
                    cx.on_release(DesktopWindow::released).detach();
                    let observer = cx.observe(&window_model, |_, _, cx| cx.notify());
                    let activation = cx.observe_window_activation(
                        window,
                        move |this: &mut DesktopWindow, window, cx| {
                            let message = match window.is_window_active() {
                                true => Message::WindowFocused(key),
                                false => Message::WindowUnfocused(key),
                            };
                            let model = this.model.clone();
                            cx.defer(move |cx| {
                                model.update(cx, |model, cx| model.dispatch(message, cx))
                            });
                        },
                    );
                    let focus = cx.focus_handle();
                    focus.focus(window, cx);
                    let keystrokes = DesktopWindow::intercept_global_keys(window, cx);
                    DesktopWindow {
                        model: window_model,
                        seat: None,
                        route: None,
                        inputs: HashMap::new(),
                        focus,
                        _activation: activation,
                        _observer: observer,
                        _keystrokes: keystrokes,
                        _focus_lost: cx.on_focus_lost(window, |_, window, cx| window.blur(cx)),
                    }
                });
                opened_view = Some(view.downgrade());
                let closing = view.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    let _ = closing.update(cx, |this, cx| {
                        this.observe_window(view_wire::events::Window::CloseRequested, cx)
                    });
                    release_window_input(window, cx);
                    true
                });
                cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
            });
            match opened {
                Ok(handle) => {
                    model.update(cx, |model, _| {
                        model.windows.insert(key, handle.into());
                        if let Some(view) = opened_view {
                            model.views.insert(key, view);
                        }
                    });
                    let _ = reply.send(key);
                }
                Err(error) => {
                    tracing::error!(target: "ducktape::app", reason = "native_window_open_failed", %error, "window could not be opened");
                    model.update(cx, |model, cx| {
                        model.state.error = format!("The window could not be opened: {error}");
                        cx.notify();
                    });
                }
            }
        });
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
    /// The program whose view this window draws, and its presenter.
    seat: Option<(&'static str, Entity<crate::module_view::NativeModuleView>)>,
    route: Option<gpui_kit::Subscription>,
    inputs: HashMap<&'static str, NativeInput>,
    focus: gpui_kit::FocusHandle,
    _activation: gpui_kit::Subscription,
    _observer: gpui_kit::Subscription,
    _keystrokes: gpui_kit::Subscription,
    _focus_lost: gpui_kit::Subscription,
}

struct NativeInput {
    state: Entity<gpui_kit::component::input::InputState>,
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
                    cx,
                );
            });
        })
    }

    /// ⌘Q quits and ⌘W closes; any other command chord goes to the seated
    /// view if it claimed it.
    fn global_key(&mut self, key: KeyPress, in_guest_editor: bool, cx: &mut Context<Self>) {
        let command = crate::backend::command_held(key.modifiers);
        if command && key.key == "q" {
            self.model
                .update(cx, |model, cx| model.dispatch(Message::TrayQuit, cx));
            cx.stop_propagation();
            return;
        }
        if !in_guest_editor && self.deliver_chord(&key, cx) {
            cx.stop_propagation();
        }
    }

    fn deliver_chord(&mut self, key: &KeyPress, cx: &mut Context<Self>) -> bool {
        let Some(chord) = crate::module_view::chord_of(&key.key, key.modifiers) else {
            return false;
        };
        let Some((_, view)) = self.seat.clone() else {
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
        self.route = None;
        if let Some((module, view)) = self.seat.take() {
            let intents = view.update(cx, |view, _| view.hide());
            for intent in intents {
                self.model.update(cx, |model, cx| {
                    model.dispatch(Message::ViewEvent(module, intent), cx)
                });
            }
        }
    }

    fn observe_window(&mut self, event: view_wire::events::Window, cx: &mut gpui_kit::App) {
        let Some((module, view)) = self.seat.clone() else {
            return;
        };
        let intents = view.update(cx, |view, cx| view.observe_final_window_event(event, cx));
        for intent in intents {
            self.model.update(cx, |model, cx| {
                model.dispatch(Message::ViewEvent(module, intent), cx)
            });
        }
    }
}

impl Render for DesktopWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::InteractiveElement as _;
        use gpui_kit::component::ActiveTheme as _;
        let content = match self.model.read(cx).state.screen {
            Screen::Connect => self.connect(window, cx),
            Screen::Console => {
                let state = self.model.read(cx).state.clone_facts();
                match (
                    !state.phrase.is_empty(),
                    state.signer_key.is_empty() && !state.browsing,
                ) {
                    (true, _) => self.phrase(&state, cx),
                    (_, true) => self.unlock(&state, window, cx),
                    _ => self.console(window, cx),
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

// ---------- launch ----------

pub(crate) fn run() {
    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    let (url_sender, mut urls) = mpsc::unbounded::<Vec<String>>();
    application.on_open_urls(move |urls| {
        let _ = url_sender.unbounded_send(urls);
    });
    application.run(move |cx| {
        gpui_kit::init(cx);
        let fonts: Vec<std::borrow::Cow<'static, [u8]>> = BUNDLED_FACES
            .iter()
            .copied()
            .map(std::borrow::Cow::Borrowed)
            .collect();
        // CoreGraphics cannot load Noto's CBDT color font; macOS supplies emoji.
        #[cfg(not(target_os = "macos"))]
        let fonts = {
            let mut fonts = fonts;
            fonts.push(std::borrow::Cow::Borrowed(EMOJI_FACE));
            fonts
        };
        if let Err(error) = cx.text_system().add_fonts(fonts) {
            tracing::error!(target: "ducktape::app", reason = "font_registration_failed", %error, "bundled desktop fonts could not be registered");
        }
        configure_native_theme(cx);
        let mut commands = commands();
        let (state, initial) = Ducktape::boot();
        let (mut tray, mut tray_events) = crate::tray::init(cx);
        tray.sync(&state);
        let desktop = cx.new(|_| Desktop {
            state,
            tray,
            windows: BTreeMap::new(),
            views: BTreeMap::new(),
            streams: HashMap::new(),
        });
        desktop.update(cx, |desktop, cx| desktop.sync_appearance(cx));
        let url_desktop = desktop.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(urls) = urls.next().await {
                let result = url_desktop.update(cx, |desktop, cx| {
                    for url in urls {
                        desktop.dispatch(Message::OpenLink(url), cx);
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
        let tray_desktop = desktop.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(row) = tray_events.next().await {
                let Some(message) = crate::tray::message(row) else {
                    continue;
                };
                if tray_desktop
                    .update(cx, |desktop, cx| desktop.dispatch(message, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let weak = desktop.downgrade();
        cx.on_window_closed(move |cx, id| {
            let weak = weak.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |desktop, cx| {
                    let key = desktop
                        .windows
                        .iter()
                        .find_map(|(key, handle)| (handle.window_id() == id).then_some(*key));
                    let Some(key) = key else {
                        return;
                    };
                    desktop.windows.remove(&key);
                    desktop.views.remove(&key);
                    desktop.dispatch(Message::WindowWasClosed(key), cx);
                });
            });
        })
        .detach();
        desktop.update(cx, |desktop, cx| {
            // the first window is the console; it draws the connect screen
            // until a node answers
            let (key, opened) = open(WindowKind::Console);
            desktop.state.console_win = Some(key);
            desktop.start(opened.map(Message::ConsoleOpened), cx).detach();
            desktop.start(initial, cx).detach();
            desktop.subscriptions(cx);
        });
        if let Some(calls) = crate::ax_door::open() {
            let door_desktop = desktop.downgrade();
            cx.spawn(async move |cx: &mut AsyncApp| {
                let windows = move |cx: &gpui_kit::App| {
                    door_desktop
                        .upgrade()
                        .map(|desktop| desktop.read(cx).ax_windows(cx))
                        .unwrap_or_default()
                };
                crate::ax_door::serve(calls, windows, cx).await;
            })
            .detach();
        }
        let command_desktop = desktop.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(pending) = commands.next().await {
                let _ = command_desktop.update(cx, |desktop, cx| desktop.execute(pending.command, cx));
                let _ = pending.completed.send(());
            }
        })
        .detach();
        let mut desktop = Some(desktop);
        cx.on_app_quit(move |_| {
            drop(desktop.take());
            async {}
        })
        .detach();
    });
}
