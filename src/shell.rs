//! The native shell: every OS window the app opens, and everything drawn
//! in one that is not a program's view. Nothing a network decides is here.
//!
//! The shell's state is its entities (`entities`): app-wide ones
//! (`Session`, `Account`, `Rail`, `Windows`, …) and one set per window,
//! each written by its own methods. Each OS window's root is a
//! `layers::WindowRoot`, a thin gpui view laying the window's layers out
//! as siblings: the launcher (connect, key, account, recovery screens;
//! `layers::LauncherLayer`) until sign-in, then the desk: the menu bar
//! (`layers::Chrome`), the panes floating under it (`layers::PaneLayer`),
//! the open dialog (⌘K, Settings, "Add a device…": `layers::OverlayLayer`),
//! the node's breath (`layers::StatusDot`) and the footer
//! (`layers::ToastView`). `Desktop` is what is left of the reducer: the
//! wall clock's beat and the tray (s12 deletes it).

use crate::ui::task::Task;
use futures::StreamExt as _;
use gpui_kit::{Context, Subscription};
use std::collections::HashMap;

use crate::ui::layout::{self, PaneMessage};
use crate::{AppMessage as Message, Ducktape};

#[cfg(debug_assertions)]
mod fixtures;
#[cfg(debug_assertions)]
pub(crate) use fixtures::render_tree_fixture;
pub(crate) mod entities;
mod figure;
mod help;
pub(crate) use help::chords;
pub(crate) use layers::Kept;
mod ink;
mod keys;
mod launch;
mod layers;
pub(in crate::shell) use layers::WindowRoot;
mod pane_drag;
mod pane_hold;
#[cfg(test)]
mod pane_hold_tests;
mod panes;
#[cfg(test)]
pub(in crate::shell) mod panes_tests;
mod screens;
#[cfg(test)]
mod screens_tests;
mod status_bar;
mod windows;

pub(crate) use launch::run;
mod spin;
mod theme;

use crate::fonts::BUNDLED_FACES;
#[cfg(not(target_os = "macos"))]
use crate::fonts::EMOJI_FACE;
use theme::configure_native_theme;

pub(crate) use crate::runtime::WindowKey;

/// What an OS window is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowKind {
    /// The main window: the launcher, then the desk with its menu bar.
    Console,
    /// A pop-out: one pane that left the desk, in a window of its own.
    View { module: &'static str },
}

/// How the platform writes a command chord: "⌘K" on a Mac, "Ctrl K"
/// elsewhere. A key led by ⇧ is with Shift ("⌘⇧M", "Ctrl Shift M"), and ↩
/// is Enter where the key has no such glyph.
pub(crate) fn chord_label(key: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let (shift, key) = match key.strip_prefix('⇧') {
        Some(key) => (true, key),
        None => (false, key),
    };
    match (mac, shift) {
        (true, false) => format!("⌘{key}"),
        (true, true) => format!("⌘⇧{key}"),
        (false, false) => format!("Ctrl {key}"),
        (false, true) => format!("Ctrl Shift {}", key.replace('↩', "Enter")),
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

/// What is left of the reducer: the wall clock's beat (`dispatch`, gated
/// so a beat that moves nothing draws nothing) and the tray, kept in step
/// with the session, the chain and the appearance. Not a window:
/// `WindowRoot` is. s12 deletes it.
struct Desktop {
    state: Ducktape,
    tray: crate::tray::Tray,
    /// The model's subscriptions (`Ducktape::subscriptions`), by recipe
    /// key: each a task feeding its stream's messages into `dispatch`.
    streams: HashMap<u64, gpui_kit::Task<()>>,
    entities: entities::Entities,
    /// What a clock's beat can move on screen, as the windows were last
    /// told to draw it (`Ducktape::beat_face`).
    drawn: crate::BeatFace,
    _subscriptions: [Subscription; 3],
}

impl Desktop {
    fn new(
        state: Ducktape,
        tray: crate::tray::Tray,
        entities: entities::Entities,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&entities.session, |desktop, _, cx| desktop.sync_tray(cx)),
            cx.observe(&entities.chain, |desktop, _, cx| desktop.sync_tray(cx)),
            cx.observe(&entities.prefs, |desktop, _, cx| desktop.sync_tray(cx)),
        ];
        Self {
            drawn: state.beat_face(),
            state,
            tray,
            streams: HashMap::new(),
            entities,
            _subscriptions: subscriptions,
        }
    }

    fn dispatch(&mut self, message: Message, cx: &mut Context<Self>) {
        let _timed = crate::perf::time(crate::perf::Key::Shell, "dispatch");
        let runtime = crate::runtime::handle();
        let _runtime = runtime.enter();
        let beat = message.is_beat();
        let task = self.state.handle(message);
        self.start(task, cx).detach();
        self.subscriptions(cx);
        // a clock's beat that finds nothing moved on screen draws no frame
        let face = self.state.beat_face();
        if !beat || face != self.drawn {
            cx.notify();
        }
        self.drawn = face;
    }

    /// The status item follows the session, the chain and the appearance.
    fn sync_tray(&mut self, cx: &Context<Self>) {
        let snapshot = crate::tray::Snapshot::of(
            self.entities.session.read(cx).get(),
            self.entities.chain.read(cx),
            self.entities.prefs.read(cx).get().appearance,
        );
        self.tray.sync(snapshot);
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
        // the ticks run on the wall clock and wake from the kernel's
        // thread: under gpui's test scheduler a wake from another thread
        // is nondeterminism, and fails whichever test outlasts a tick
        if cfg!(test) {
            return;
        }
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
