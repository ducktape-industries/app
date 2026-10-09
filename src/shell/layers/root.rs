//! The root view of one OS window: thin, uncached, laying the window's
//! layers out as siblings. On the desk: the bar (`Chrome`, console only),
//! the panes (`PaneLayer`), the dialog open over them (`OverlayLayer`),
//! the node's breath (`StatusDot`) and the footer (`ToastView`). Before the
//! desk the console draws its launcher screen (`LauncherLayer`, uncached)
//! and the footer.
//! Every cached layer under it hits while nothing it observes moved (P6):
//! a pulse of the dot draws the dot, this root and the uncached pane layer
//! and pane views under it, and no cached layer redraws.
//!
//! The root holds what is the window's own and not a layer's: its focus
//! (the keys' fallback), the key context the keymap reads, the actions a
//! window answers (`keys.rs`), the pane messages the layers send through
//! it (`panes.rs`, `pane_hold.rs`), and the switch timer.

use super::super::entities::{Desk, Entities, Observed, Overlays, Prefs, Screen, Slice};
use super::super::{WindowKey, WindowKind, ink, layout, theme};
use super::{BAR, Chrome, LauncherLayer, OverlayLayer, PaneLayer, StatusDot, ToastView};
use crate::render::deferred::HOST_BAND;
use gpui_kit::{
    AnyView, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _,
    Render, StyleRefinement, Styled as _, Subscription, Window, deferred, div, px,
};

/// The gpui view at the root of one OS window.
pub(in crate::shell) struct WindowRoot {
    /// The app's entities: every layer draws from them and every control
    /// calls into them.
    pub(in crate::shell) app: Entities,
    pub(in crate::shell) key: WindowKey,
    pub(in crate::shell) kind: WindowKind,
    /// This window's panes (`Windows::own`).
    pub(in crate::shell) desk: Entity<Desk>,
    /// Which screen the console shows: the launcher's, or the desk.
    screen: Observed<Slice<Screen>>,
    /// What is open over the desk: the key context says so.
    pub(in crate::shell) overlays: Observed<Overlays>,
    prefs: Observed<Slice<Prefs>>,
    /// The menu bar and its menus (the console only).
    chrome: Option<Entity<Chrome>>,
    /// The panes, drawn: one `PaneView` per pane, the keys' handoff between
    /// them, the pointer's and the keyboard's hold on one.
    pub(in crate::shell) panes: Entity<PaneLayer>,
    /// Spotlight, Settings and "Add a device…" over the desk, and where the
    /// keys go as anything opens or closes over it (the console only).
    pub(in crate::shell) overlay_layer: Option<Entity<OverlayLayer>>,
    /// The launcher screens (the console only).
    launcher: Option<Entity<LauncherLayer>>,
    toast: Entity<ToastView>,
    /// The node's breath over the bar's well (the console only).
    dot: Option<Entity<StatusDot>>,
    /// The handle the box a bar menu's card hangs from holds the keys by
    /// (`Chrome`'s; the console only).
    menu: Option<FocusHandle>,
    /// A pane or window switch under way: started where it was asked for,
    /// ended on the frame after the one that shows it (docs/perf.md).
    switching: Option<crate::perf::Timer>,
    pub(in crate::shell) focus: FocusHandle,
    _subscriptions: [Subscription; 2],
}

impl WindowRoot {
    /// A window of `kind`, focused on its own root (so the first Tab
    /// reaches the first control). Its own entities are `Windows`' (made
    /// as the window was asked for).
    pub(in crate::shell) fn new(
        app: Entities,
        key: WindowKey,
        kind: WindowKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let own = app
            .windows
            .read(cx)
            .own(key)
            .expect("a window's entities are made as it is asked for")
            .clone();
        let (screen, prefs) = (app.screen.clone(), app.prefs.clone());
        let (desk, overlays) = (own.desk.clone(), own.overlays.clone());
        let console = kind == WindowKind::Console;
        let chrome = console.then(|| {
            let (app, this) = (app.clone(), cx.weak_entity());
            cx.new(|cx| Chrome::new(app, key, &own, this, window, cx))
        });
        let menu = chrome.as_ref().map(|chrome| chrome.read(cx).menu.clone());
        // the dialogs open in the window the app names its console: the
        // console itself, or a `--live` view's window with no bar to hang
        // a menu from
        let dialogs = console || app.windows.read(cx).console() == Some(key);
        let overlay_layer = dialogs.then(|| {
            let (app, menu) = (
                app.clone(),
                menu.clone().unwrap_or_else(|| cx.focus_handle()),
            );
            cx.new(|cx| OverlayLayer::new(app, key, &own, menu, window, cx))
        });
        let launcher = console.then(|| cx.new(|cx| LauncherLayer::new(&app, key, window, cx)));
        let dot = console.then(|| cx.new(|cx| StatusDot::new(&app, key, &own, cx)));
        let toast = cx.new(|cx| ToastView::new(&app, key, cx));
        let panes = {
            let (app, root, this) = (app.clone(), focus.clone(), cx.weak_entity());
            cx.new(|cx| PaneLayer::new(app, key, kind, own, root, this, window, cx))
        };
        let subscriptions = [
            // a switch to this window is timed to the frame that shows it
            // (`Windows` hears the activation too: the front, the hold)
            cx.observe_window_activation(window, move |this: &mut Self, window, _| {
                if window.is_window_active() {
                    this.start_switch();
                }
            }),
            cx.on_focus_lost(window, |this, window, cx| this.focus_lost(window, cx)),
        ];
        Self {
            key,
            kind,
            desk,
            screen: Observed::new(&screen, cx),
            overlays: Observed::new(&overlays, cx),
            prefs: Observed::new(&prefs, cx),
            chrome,
            panes,
            overlay_layer,
            launcher,
            toast,
            dot,
            menu,
            switching: None,
            focus,
            app,
            _subscriptions: subscriptions,
        }
    }

    /// A switch asked for: timed from here to the frame after the one that
    /// shows it. One under way already keeps its start.
    pub(in crate::shell) fn start_switch(&mut self) {
        if self.switching.is_none() {
            self.switching = crate::perf::time(crate::perf::Key::Window(self.key), "switch");
        }
    }

    /// On the desk: connected, and past the key and account steps.
    pub(in crate::shell) fn on_desk(&self, cx: &gpui_kit::App) -> bool {
        *self.screen.read(cx).get() == Screen::Desk
    }

    /// The desk's own keys reach its windows: the console, on the desk,
    /// with no overlay (Spotlight, a menu, Settings) keeping its keys.
    pub(in crate::shell) fn desk_keys(&self, cx: &gpui_kit::App) -> bool {
        self.kind == WindowKind::Console
            && self.on_desk(cx)
            && self.overlays.read(cx).get().is_none()
    }

    /// What ⌘W closes: the focused desk window, when the desk's keys reach
    /// it and it has one; `None` is the app's window (`close_by_key`).
    pub(in crate::shell) fn command_w_pane(&self, cx: &gpui_kit::App) -> Option<usize> {
        let layout = self.layout(cx);
        (self.desk_keys(cx) && !layout.panes.is_empty()).then_some(layout.focused)
    }

    /// ⌘W closes a window, never the app. A pop-out closes as its pane's ×
    /// does. The console closes too where the status item reopens it
    /// (macOS); elsewhere there is no tray to bring it back from, and the
    /// last window closing would quit — so it minimizes instead.
    pub(in crate::shell) fn close_by_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match (self.kind, cfg!(target_os = "macos")) {
            (WindowKind::Console, false) => window.minimize_window(),
            _ => super::super::windows::remove(window.window_handle(), cx),
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
    ///
    /// What vanished sat in a menu that still shows (a row it cleared, one
    /// menu giving way to the next): the keys stay in the menu, at its first
    /// control, and it stays open.
    fn focus_lost(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &self.menu
            && window.focus_lost_restore_target(cx).as_ref() == Some(menu)
        {
            menu.focus(window, cx);
            window.focus_next(cx);
            // gpui draws no frame for a move made here: the ring shows now
            cx.notify();
            return;
        }
        self.focus.focus(window, cx);
    }

    /// The console's launcher: a test's way to its fields and figure.
    #[cfg(test)]
    pub(in crate::shell) fn launcher(&self) -> &Entity<LauncherLayer> {
        self.launcher.as_ref().expect("the console has a launcher")
    }

    /// The console's dialogs: a test's way to where their keys go.
    #[cfg(test)]
    pub(in crate::shell) fn dialogs(&self) -> &Entity<OverlayLayer> {
        self.overlay_layer
            .as_ref()
            .expect("the console has dialogs")
    }

    /// What is open over this window's desk: a test's way to open or close
    /// it as the bar and the keys do.
    #[cfg(test)]
    pub(in crate::shell) fn overlays(&self) -> Entity<Overlays> {
        self.overlays.entity().clone()
    }

    /// This window's panes (the `Desk` slice).
    pub(in crate::shell) fn layout(&self, cx: &gpui_kit::App) -> layout::Layout {
        self.desk.read(cx).get().clone()
    }
}

impl Render for WindowRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::{InteractiveElement as _, StatefulInteractiveElement as _};
        let _timed = crate::perf::time(crate::perf::Key::Window(self.key), "frame.render");
        crate::perf::count(crate::perf::Key::Window(self.key), "renders", 1);
        if let Some(switching) = self.switching.take() {
            // next-frame callbacks run once the frame showing the switch
            // was presented: an upper bound, high by one frame interval
            window.on_next_frame(move |_, _| drop(switching));
        }
        let layer = || StyleRefinement::default().absolute().inset_0();
        // the footer, over an open menu or dialog; all three in the host's
        // band, over anything a view defers
        let toast = deferred(AnyView::from(self.toast.clone()).cached(layer()))
            .with_priority(HOST_BAND + 3);
        let launcher = self.kind == WindowKind::Console && !self.on_desk(cx);
        let content = match (launcher, &self.launcher) {
            // The launcher (the sign-in and unlock screens) is drawn
            // uncached, against the spec's cached `size_full` (chief,
            // 2026-09-30): a cached launcher moves the unlock screen's
            // pixels by 1 LSB (GPU sprite order within one draw order),
            // which the look rule forbids, and these screens gain nothing
            // from the cache. For the same reason its figure follows a new
            // screen a frame late (`LauncherLayer::render`).
            (true, Some(launcher)) => div()
                .size_full()
                .child(launcher.clone())
                .child(toast)
                .into_any_element(),
            _ => {
                // the bar: a cached view of its own, its menus hanging from it
                let bar = self.chrome.clone().map(|chrome| {
                    AnyView::from(chrome).cached(
                        StyleRefinement::default()
                            .w_full()
                            .h(px(BAR))
                            .flex_shrink_0(),
                    )
                });
                let overlays = self.overlay_layer.clone().map(|layer_view| {
                    deferred(AnyView::from(layer_view).cached(layer())).with_priority(HOST_BAND)
                });
                div()
                    .id("console")
                    .size_full()
                    .flex()
                    .flex_col()
                    .children(bar)
                    // the panes: a layer of their own, measuring the desk they sit on
                    .child(
                        div()
                            .id("seat")
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(self.panes.clone()),
                    )
                    .children(overlays)
                    .children(self.dot.clone())
                    .child(toast)
                    .into_any_element()
            }
        };
        let ink = ink::Ink::of(self.prefs.read(cx).get().dark());
        let mut root = div();
        root.text_style().font_fallbacks = Some(crate::fonts::fallback_chain());
        root.text_style().font_family = Some(theme::FAMILY_UI.into());
        // the window's root takes the keys whenever nothing inside has them
        // (a screen gave way, a pane moved): named, so assistive technology
        // says where the keys are instead of reading the whole window out
        let root = crate::a11y::Patch::default().keys_fallback().on(root
            .id("desktop-root")
            .role(gpui_kit::Role::Group)
            .aria_label("Ducktape"));
        self.on_keys(root, cx)
            .size_full()
            .bg(ink.bg)
            .text_color(ink.ink)
            .track_focus(&self.focus)
            .child(content)
    }
}
