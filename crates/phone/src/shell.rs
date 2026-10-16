//! The phone's one window: the host started once per process
//! ([`launch`]), the window opened over it ([`open_window`]: once on iOS,
//! once per Activity on Android), what the host hands on (Back, a changed
//! inset) acted on, and in the gate build the AX door.
//!
//! The window draws the [`stack`]: a tab root (Home, Open, Account) under the
//! tab bar, or a program pushed over it under its capsule. The safe area
//! pads every edge but the bottom, which the tab bar (or, over a program,
//! the body) takes.

mod home;
mod look;
pub mod stack;

use ducktape_host::contract::{Boot, OsServices, Signal};
use ducktape_host::session::{Session, Shared};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Context, Global, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, WeakEntity, Window, WindowOptions, div,
    px,
};
use stack::{Pushed, Stack, Tab};
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

/// What the shell keeps between [`launch`] and every [`open_window`]: the
/// host's entities and the window on screen.
struct Phone {
    shared: Shared,
    window: Option<(AnyWindowHandle, WeakEntity<Root>)>,
}

impl Global for Phone {}

/// Starts the host on `os`, once per process: the app's faces and theme,
/// the host's stores and entities, and a connect to the node this phone
/// used last.
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
    let runner = crate::os::runner(os.dirs());
    let boot = Boot {
        os,
        runner,
        // banners are not posted yet (`os::NoBanners`): no click lands
        open: |_| {},
    };
    let (host, inbound) = ducktape_host::start::start(boot, cx);
    crate::os::install_inbound(inbound);
    if let Some(target) = Session::boot_target() {
        host.shared
            .session
            .update(cx, |session, cx| session.connect(target, cx));
    }
    let signals = host.signals;
    cx.set_global(Phone {
        shared: host.shared,
        window: None,
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(signal) = signals.recv().await {
            cx.update(|cx| handed_on(signal, cx));
        }
    })
    .detach();
}

/// Opens the phone's window over the host [`launch`] started.
pub fn open_window(cx: &mut App) {
    let Some(shared) = cx.try_global::<Phone>().map(|phone| phone.shared.clone()) else {
        tracing::error!(target: "ducktape::phone", "a window was asked for before the host started");
        return;
    };
    let opened = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        cx.new(|cx| Root::new(shared, window, cx))
    });
    match opened {
        Ok((handle, root)) => {
            cx.global_mut::<Phone>().window = Some((handle, root.downgrade()));
            #[cfg(feature = "ax-door")]
            open_door(cx);
        }
        Err(error) => {
            tracing::error!(target: "ducktape::phone", %error, "the phone's window did not open");
        }
    }
}

/// What the host hands on rather than consumes: Back walks the stack (and
/// past its last screen, the OS's own); a changed inset redraws.
fn handed_on(signal: Signal, cx: &mut App) {
    let Some(root) = cx
        .try_global::<Phone>()
        .and_then(|phone| phone.window.as_ref())
        .and_then(|(_, root)| root.upgrade())
    else {
        return;
    };
    match signal {
        Signal::Back => root.update(cx, |root, cx| {
            if root.stack.back() {
                cx.notify();
            } else {
                crate::os::back_at_root();
            }
        }),
        Signal::InsetsChanged => root.update(cx, |_, cx| cx.notify()),
        // no seat is warm yet to drop; the host consumes the rest
        Signal::MemoryWarning | Signal::Lifecycle(_) | Signal::BannerClick(_) => {}
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

/// The window's root: the stack over the host's entities.
pub struct Root {
    shared: Shared,
    stack: Stack,
    _subscriptions: Vec<Subscription>,
}

impl Root {
    fn new(shared: Shared, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![
            cx.observe(&shared.rail, |_, _, cx| cx.notify()),
            cx.observe(&shared.session, |_, _, cx| cx.notify()),
            cx.observe_window_appearance(window, |_, window, cx| {
                gpui_kit::component::Theme::sync_system_appearance(Some(window), cx);
                cx.notify();
            }),
        ];
        subscriptions.extend(crate::os::follow(window, cx));
        gpui_kit::component::Theme::sync_system_appearance(Some(window), cx);
        Self {
            shared,
            stack: Stack::default(),
            _subscriptions: subscriptions,
        }
    }

    fn pick(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.stack.pick(tab);
        cx.notify();
    }

    fn open(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.stack.push(Pushed::Program(program));
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.stack.close();
        cx.notify();
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edges = ducktape_host::contract::os().insets(window).effective();
        let palette = look::palette(window);
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.background())
            .pt(edges.top)
            .pl(edges.left)
            .pr(edges.right);
        match self.stack.top() {
            Some(Pushed::Program(program)) => root
                .child(home::program(program, &self.shared, palette, cx))
                .child(div().h(edges.bottom).flex_none()),
            None => {
                let body = match self.stack.tab() {
                    Tab::Home => home::home(&self.shared, palette, cx).into_any_element(),
                    Tab::Open => home::open(palette).into_any_element(),
                    Tab::Account => home::account(&self.shared, palette, cx).into_any_element(),
                };
                root.child(body).child(home::tab_bar(
                    self.stack.tab(),
                    edges.bottom.max(px(0.)),
                    palette,
                    cx,
                ))
            }
        }
    }
}
