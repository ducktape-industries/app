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

    /// The seat draws `module`'s view; a change of program re-seats.
    fn seat(
        &mut self,
        module: &'static str,
        cx: &mut Context<Self>,
    ) -> Entity<crate::module_view::NativeModuleView> {
        if let Some((seated, view)) = &self.seat
            && *seated == module
        {
            return view.clone();
        }
        self.unseat(cx);
        let view = cx.new(|_| crate::module_view::NativeModuleView::new(module));
        let model = self.model.clone();
        self.route = Some(cx.subscribe(&view, move |_, _, event, cx| {
            let event = event.clone();
            model.update(cx, |model, cx| {
                model.dispatch(Message::ViewEvent(module, event), cx)
            });
        }));
        self.seat = Some((module, view.clone()));
        view
    }

    /// A native text field; Enter dispatches `on_enter`, every change
    /// dispatches `on_change` with the text.
    #[allow(clippy::too_many_arguments, reason = "one call site per field")]
    fn input(
        &mut self,
        key: &'static str,
        placeholder: &'static str,
        masked: bool,
        initial: &str,
        on_change: fn(String) -> Message,
        on_enter: fn() -> Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::input::{Input, InputContentType, InputEvent, InputState};
        if !self.inputs.contains_key(key) {
            let state = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(masked)
            });
            state.update(cx, |state, cx| {
                state.set_value(initial.to_owned(), window, cx)
            });
            let model = self.model.clone();
            let subscription = cx.subscribe_in(&state, window, move |_, input, event, _, cx| {
                match event {
                    InputEvent::PressEnter { .. } => {
                        model.update(cx, |model, cx| model.dispatch(on_enter(), cx));
                    }
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        model.update(cx, |model, cx| model.dispatch(on_change(text), cx));
                    }
                    _ => {}
                }
                cx.notify();
            });
            self.inputs.insert(
                key,
                NativeInput {
                    state,
                    _subscription: subscription,
                },
            );
        }
        use gpui_kit::{Focusable as _, StatefulInteractiveElement as _};
        let state = &self.inputs[key].state;
        let input = Input::new(state).id(key);
        let input = match masked {
            true => input.content_type(InputContentType::Password),
            false => input,
        };
        let field = crate::a11y::text_field(
            gpui_kit::SharedString::from(format!("{key}/field")),
            &state.read(cx).focus_handle(cx),
            {
                let state = state.clone();
                move |value, window, cx| {
                    state.update(cx, |state, cx| state.replace_all(value, window, cx))
                }
            },
            input.role(gpui_kit::component::RoleOverride::Presentational),
        )
        .aria_label(placeholder);
        match masked {
            true => field.role(gpui_kit::Role::PasswordInput),
            false => field.role(gpui_kit::Role::TextInput),
        }
        .into_any_element()
    }

    fn action(
        &self,
        key: impl Into<gpui_kit::ElementId>,
        label: impl Into<gpui_kit::SharedString>,
        message: fn() -> Message,
        disabled: bool,
    ) -> gpui_kit::component::button::Button {
        use gpui_kit::component::Disableable as _;
        let model = self.model.clone();
        let button = gpui_kit::component::button::Button::new(key)
            .label(label)
            .disabled(disabled)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                model.update(cx, |model, cx| model.dispatch(message(), cx))
            });
        crate::a11y::disabled(button, disabled)
    }

    fn toast(&self, cx: &gpui_kit::App) -> Option<gpui_kit::Stateful<gpui_kit::Div>> {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let toast = self.model.read(cx).state.toast.clone();
        if toast.is_empty() {
            return None;
        }
        let theme = gpui_kit::component::Theme::global(cx);
        Some(
            div()
                .id("toast")
                .role(Role::Status)
                .absolute()
                .bottom_4()
                .right_4()
                .max_w(px(420.))
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_2p5()
                .rounded(px(design::radius::CARD as f32))
                .border_1()
                .border_color(theme.color_tokens().border)
                .bg(theme.popover)
                .shadow_md()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.5))
                        .child(Text::new("toast-message".into(), toast.into())),
                )
                .child(
                    self.action("toast-dismiss", "Dismiss", || Message::DismissToast, false)
                        .ghost()
                        .h_7(),
                ),
        )
    }

    /// Reaching a node: a URL, the ones used before, and why the last try
    /// did not land.
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let state = self.model.read(cx).state.clone_facts();
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let field = self.input(
            "endpoint",
            "http://127.0.0.1:8844",
            false,
            &state.endpoint,
            Message::EndpointTyped,
            || Message::ConnectSubmit,
            window,
            cx,
        );
        let recent = state.recent_endpoints.iter().map(|endpoint| {
            let model = self.model.clone();
            let target = endpoint.clone();
            div()
                .id(SharedString::from(format!("recent/{endpoint}")))
                .control(Role::Button, SharedString::from(endpoint.clone()))
                .cursor_pointer()
                .px_2()
                .py_1()
                .rounded(px(design::radius::CONTROL as f32))
                .text_size(px(12.5))
                .text_color(colors.muted_foreground)
                .hover(|style| style.text_color(colors.foreground))
                .on_click(move |_, _, cx| {
                    let target = target.clone();
                    model.update(cx, |model, cx| {
                        model.dispatch(Message::ConnectTo(target), cx)
                    })
                })
                .child(endpoint.clone())
        });
        let note = match (!state.error.is_empty(), !state.endpoint_error.is_empty()) {
            (true, _) => Some(state.error.clone()),
            (_, true) => Some(state.endpoint_error.clone()),
            _ => None,
        };
        div()
            .id("connect")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(460.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Connect to a node"),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(colors.muted_foreground)
                            .child("The node serves everything you will see: its programs, their views, your account."),
                    )
                    .child(field)
                    .child(
                        div().flex().gap_2().child(
                            self.action("connect", "Connect", || Message::ConnectSubmit, state.connecting)
                                .primary(),
                        )
                        .child(div().flex_1().text_size(px(12.5)).text_color(colors.muted_foreground).child(state.status.clone())),
                    )
                    .children(note.map(|note| {
                        div()
                            .id("connect-error")
                            .role(Role::Alert)
                            .text_size(px(12.5))
                            .text_color(hsla_of(design::palette(state.dark).danger))
                            .child(note)
                    }))
                    .when(!state.recent_endpoints.is_empty(), |card| {
                        card.child(
                            div()
                                .mt_2()
                                .text_size(px(11.))
                                .text_color(colors.muted_foreground)
                                .child("Recent"),
                        )
                        .children(recent)
                    }),
            )
            .into_any_element()
    }

    /// Inside a node: the rail of its programs on the left, the open one's
    /// view on the right, and the key's lock at the foot.
    fn console(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let state = self.model.read(cx).state.clone_facts();
        let (sidebar_bg, colors) = {
            let theme = gpui_kit::component::Theme::global(cx);
            (theme.sidebar, theme.color_tokens())
        };
        let palette = design::palette(state.dark);
        let ink_fg = hsla_of(palette.sidebar_foreground);
        let ink_muted = hsla_of(palette.sidebar_muted);
        let ink_raised = hsla_of(palette.sidebar_raised);
        let ink_border = hsla_of(palette.sidebar_border);
        let accent = hsla_of(palette.accent);
        let faint = hsla_of(palette.faint);
        let rail = crate::module_view::rail();
        if rail.iter().any(|row| row.note == Some("Loading")) {
            window.request_animation_frame();
        }
        let active = state
            .active
            .or_else(|| rail.iter().find(|row| !row.empty).map(|row| row.module));
        let rows = rail.iter().filter(|row| !row.empty).map(|row| {
            let module = row.module;
            let model = self.model.clone();
            let selected = active == Some(module);
            let badge = state.badges.get(module).copied().unwrap_or(0);
            let label = match row.note {
                Some(note) => format!("{} · {note}", row.label),
                None => row.label.clone(),
            };
            div()
                .id(SharedString::from(format!("rail/{module}")))
                .control(Role::Tab, SharedString::from(label.clone()))
                .aria_selected(selected)
                .focusable()
                .tab_stop(true)
                .flex()
                .items_center()
                .gap_2()
                .h(px(28.))
                .px_2()
                .mb_0p5()
                .rounded(px(design::radius::CONTROL as f32))
                .cursor_pointer()
                .text_size(px(13.))
                .font_weight(match selected {
                    true => FontWeight::MEDIUM,
                    false => FontWeight::NORMAL,
                })
                .text_color(match selected {
                    true => ink_fg,
                    false => ink_muted,
                })
                .when(selected, |row| row.bg(ink_raised))
                .hover(move |style| style.bg(ink_raised).text_color(ink_fg))
                .on_click(move |_, _, cx| {
                    model.update(cx, |model, cx| {
                        model.dispatch(Message::SelectView(module), cx)
                    })
                })
                .child(div().flex_1().min_w_0().truncate().child(label))
                .when(badge > 0, |row| {
                    row.child(
                        div()
                            .px_1p5()
                            .rounded_full()
                            .bg(accent)
                            .text_size(px(10.))
                            .text_color(hsla_of(palette.background))
                            .child(badge.to_string()),
                    )
                })
        });
        let rows: Vec<_> = rows.collect();
        let unlocked = !state.signer_key.is_empty();
        let foot = match unlocked {
            true => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(ink_muted)
                        .truncate()
                        .child(format!(
                            "key {}…",
                            &state.signer_key[..state.signer_key.len().min(12)]
                        )),
                )
                .child(
                    self.action("lock", "Lock", || Message::Lock, false)
                        .ghost()
                        .w_full(),
                ),
            false => {
                let password = self.input(
                    "password",
                    "Password",
                    true,
                    "",
                    Message::PasswordTyped,
                    || Message::UnlockSubmit,
                    window,
                    cx,
                );
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(ink_muted)
                            .child("Sign in to write"),
                    )
                    .child(password)
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                self.action(
                                    "unlock",
                                    "Unlock",
                                    || Message::UnlockSubmit,
                                    state.unlock_busy,
                                )
                                .primary()
                                .flex_1(),
                            )
                            .child(
                                self.action(
                                    "create-wallet",
                                    "New key",
                                    || Message::CreateWalletSubmit,
                                    state.unlock_busy,
                                )
                                .ghost(),
                            ),
                    )
                    .when(!state.unlock_error.is_empty(), |foot| {
                        foot.child(
                            div()
                                .id("unlock-error")
                                .role(Role::Alert)
                                .text_size(px(11.))
                                .text_color(hsla_of(design::palette(state.dark).danger))
                                .child(state.unlock_error.clone()),
                        )
                    })
            }
        };
        let sidebar = div()
            .id("rail")
            .w(px(RAIL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(sidebar_bg)
            .border_r_1()
            .border_color(ink_border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .pt(px(if cfg!(target_os = "macos") { 44. } else { 12. }))
                    .pb_2()
                    .child(
                        div()
                            .size(px(6.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(if state.connected { accent } else { faint }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ink_fg)
                                    .truncate()
                                    .child(state.network.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(ink_muted)
                                    .truncate()
                                    .child(state.status.clone()),
                            ),
                    ),
            )
            .child(
                div()
                    .id("rail-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .role(Role::TabList)
                    .children(rows)
                    .when(rail.is_empty(), |list| {
                        list.child(
                            div()
                                .px_2()
                                .text_size(px(12.))
                                .text_color(ink_muted)
                                .child("No programs listed yet"),
                        )
                    }),
            )
            .child(
                div()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(ink_border)
                    // THE HOST'S OWN WORD ON WHAT IS RECORDING: a seated view
                    // draws inside its seat and can neither paint here nor
                    // decline to be listed.
                    .when_some(crate::module_view::capturing(), |rail, recording| {
                        rail.child(
                            div()
                                .id("capture-indicator")
                                .role(Role::Status)
                                .mb_1()
                                .px_1p5()
                                .py_0p5()
                                .rounded_full()
                                .bg(hsla_of(palette.danger))
                                .text_size(px(10.))
                                .text_color(hsla_of(palette.background))
                                .child(recording),
                        )
                    })
                    .child(foot)
                    .child(
                        self.action("disconnect", "Switch node", || Message::Disconnect, false)
                            .ghost()
                            .w_full()
                            .mt_1(),
                    ),
            );
        let seat = match active {
            Some(module) => {
                let view = self.seat(module, cx);
                let props = self.model.read(cx).state.view_props();
                view.update(cx, |view, cx| view.set_props(props, cx));
                view.into_any_element()
            }
            None => {
                self.unseat(cx);
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(colors.muted_foreground)
                    .child("This network runs no program with a view.")
                    .into_any_element()
            }
        };
        div()
            .id("console")
            .size_full()
            .flex()
            .child(sidebar)
            .child(div().id("seat").flex_1().min_w_0().h_full().child(seat))
            .children(self.toast(cx))
            .into_any_element()
    }
}

impl Render for DesktopWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::InteractiveElement as _;
        use gpui_kit::component::ActiveTheme as _;
        let content = match self.model.read(cx).state.screen {
            Screen::Connect => self.connect(window, cx),
            Screen::Console => self.console(window, cx),
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

/// The facts a draw reads, copied out so the model lock is not held while
/// elements are built.
#[derive(Clone)]
pub(crate) struct Facts {
    pub(crate) dark: bool,
    pub(crate) endpoint: String,
    pub(crate) endpoint_error: String,
    pub(crate) recent_endpoints: Vec<String>,
    pub(crate) connected: bool,
    pub(crate) connecting: bool,
    pub(crate) network: String,
    pub(crate) status: String,
    pub(crate) error: String,
    pub(crate) signer_key: String,
    pub(crate) unlock_error: String,
    pub(crate) unlock_busy: bool,
    pub(crate) active: Option<&'static str>,
    pub(crate) badges: BTreeMap<&'static str, i64>,
}

impl Ducktape {
    pub(crate) fn clone_facts(&self) -> Facts {
        Facts {
            dark: self.dark(),
            endpoint: self.endpoint.clone(),
            endpoint_error: self.endpoint_error.clone(),
            recent_endpoints: self.recent_endpoints.clone(),
            connected: self.connected,
            connecting: self.connecting,
            network: self.network.clone(),
            status: self.status.clone(),
            error: self.error.clone(),
            signer_key: self.signer_key.clone(),
            unlock_error: self.unlock_error.clone(),
            unlock_busy: self.unlock_busy,
            active: self.active,
            badges: self.badges.clone(),
        }
    }
}

// ---------- theme and fonts ----------

fn configure_native_theme(cx: &mut gpui_kit::App) {
    use gpui_kit::component::{Theme, ThemeRegistry};
    let registry = ThemeRegistry::global_mut(cx);
    let registered = registry.themes().contains_key(design::LIGHT_THEME);
    if !registered {
        registry
            .load_themes_from_str(&design::kit_theme_json())
            .expect("the product theme parses");
    }
    let light = with_syntax_colors(
        &registry.themes()[design::LIGHT_THEME],
        registry.default_light_theme(),
        &design::LIGHT,
    );
    let dark = with_syntax_colors(
        &registry.themes()[design::DARK_THEME],
        registry.default_dark_theme(),
        &design::DARK,
    );
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    let mode = theme.mode;
    Theme::change(mode, None, cx);
}

fn with_syntax_colors(
    product: &std::rc::Rc<gpui_kit::component::ThemeConfig>,
    defaults: &std::rc::Rc<gpui_kit::component::ThemeConfig>,
    palette: &design::Palette,
) -> std::rc::Rc<gpui_kit::component::ThemeConfig> {
    let mut theme = (**product).clone();
    let mut style = defaults.highlight.clone().unwrap_or_default();
    style.editor_background = Some(hsla_of(palette.background));
    theme.highlight = Some(style);
    std::rc::Rc::new(theme)
}

fn hsla_of(color: design::Color) -> gpui_kit::Hsla {
    let [r, g, b, a] = color;
    gpui_kit::Rgba { r, g, b, a }.into()
}

const RAIL_WIDTH: f32 = 200.;

const BUNDLED_FACES: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Italic.ttf"),
    include_bytes!("../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../assets/fonts/Inter-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/Pretendard-Regular.otf"),
    include_bytes!("../assets/fonts/Pretendard-Bold.otf"),
    include_bytes!("../assets/fonts/D2Coding-Regular.ttf"),
    include_bytes!("../assets/fonts/D2Coding-Bold.ttf"),
];

#[cfg(not(target_os = "macos"))]
const EMOJI_FACE: &[u8] = include_bytes!("../assets/fonts/NotoColorEmoji.ttf");

const FALLBACK_FAMILIES: &[&str] = &[
    "Apple Color Emoji",
    "Noto Color Emoji",
    "Apple SD Gothic Neo",
    "Hiragino Sans",
    "PingFang SC",
    "Noto Sans",
    "DejaVu Sans",
    "Apple Symbols",
];

pub(crate) fn fallback_chain() -> gpui_kit::FontFallbacks {
    static CHAIN: std::sync::LazyLock<gpui_kit::FontFallbacks> =
        std::sync::LazyLock::new(|| chain_led_by(design::fonts::FAMILY_UI_HANGUL));
    CHAIN.clone()
}

pub(crate) fn mono_fallback_chain() -> gpui_kit::FontFallbacks {
    static CHAIN: std::sync::LazyLock<gpui_kit::FontFallbacks> =
        std::sync::LazyLock::new(|| chain_led_by(design::fonts::FAMILY_MONO_HANGUL));
    CHAIN.clone()
}

fn chain_led_by(hangul: &str) -> gpui_kit::FontFallbacks {
    gpui_kit::FontFallbacks::from_fonts(
        std::iter::once(hangul.to_string())
            .chain(FALLBACK_FAMILIES.iter().map(|name| name.to_string()))
            .collect(),
    )
}

pub(crate) fn with_family<E: gpui_kit::Styled>(
    mut element: E,
    family: impl Into<gpui_kit::SharedString>,
) -> E {
    let family = family.into();
    let is_code_face = family == design::fonts::FAMILY_MONO;
    let style = element.text_style();
    style.font_fallbacks = Some(if is_code_face {
        mono_fallback_chain()
    } else {
        fallback_chain()
    });
    style.font_family = Some(family);
    element
}

pub(crate) fn mono_family<E: gpui_kit::Styled>(element: E) -> E {
    with_family(element, design::fonts::FAMILY_MONO)
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
