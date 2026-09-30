//! One window's panes, drawn. `PaneLayer` keeps a `PaneView` per pane its
//! `Desk` holds, hands the keys to the one in front, holds one for the
//! pointer (`pane_drag.rs`) or the keyboard (`pane_hold.rs`), and reports
//! the desk's size. A `PaneView` draws a pane's strip (`Strip`: its title
//! bar and, while its view asks to notify, the permission bar), its body (a
//! seat's tree, an empty window's finder or Help) and its grips; the strip
//! and each body are cached views of their own. Neither layer nor pane view
//! is cached: an uncached parent lets the cached views inside it hit (P6),
//! so a seat's tree draws again only when its seat moves, and a strip only
//! when what it shows does.
//!
//! Where panes sit, stack and which has the keys is the window's `Desk`
//! (`ui::layout`); this file draws it and sends `PaneMessage`s through
//! the window (`WindowRoot::pane_message`).

use super::super::entities::{
    Desk, Entities, Notifications, Observed, Overlays, Prefs, Rail, Seats, Slice, WindowEntities,
    Windows,
};
use super::super::{
    PaneMessage, WindowKey, WindowKind, WindowRoot, chord_label, layout, pane_drag, pane_hold,
    panes, theme,
};
use super::{EmptyPane, HelpPane};
use crate::a11y::Control as _;
use crate::runtime::Seat;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;

/// A title bar's height.
const TITLE: f32 = 32.;

/// The layer drew the panes as this layout has them, every pane with its
/// view. An effect: its handler runs after the draw, in the frame that
/// shows the panes, and skips a frame the layout has moved on from.
struct Drawn(layout::Layout);

/// The panes of one window. It observes the window's `Desk` and the app's
/// `Seats`: a pane that came gets a `PaneView`, one that went loses it, and
/// the seat of the pane in front is told so. It observes its `Overlays`
/// too, whose closing hands the keys back after the draw it asks for. The keys' handoff between
/// panes runs after every draw of the layer (`drawn`), where the frame that
/// shows a pane is the one its first control is found in.
pub(in crate::shell) struct PaneLayer {
    pub(in crate::shell) app: Entities,
    pub(in crate::shell) key: WindowKey,
    pub(in crate::shell) kind: WindowKind,
    pub(in crate::shell) desk: Entity<Desk>,
    /// What is open over this window's desk: the keys and the pointer's
    /// presses leave the panes alone while something is. Read at a press
    /// and after a draw, never at one.
    pub(in crate::shell) overlays: Observed<Overlays>,
    seats: Entity<Seats>,
    /// The window this layer draws in: pane messages and the overlay's
    /// `refocus` are its.
    pub(in crate::shell) window: WeakEntity<WindowRoot>,
    /// The desk's body while no pane is on it.
    empty_desk: Entity<EmptyPane>,
    /// One view per pane, by the pane's instance.
    views: HashMap<u64, Entity<PaneView>>,
    /// Each pane's own focus (its view's box), by instance, and what in it
    /// last had the keys: a pane that comes to the front gets them back.
    pub(in crate::shell) pane_keys: HashMap<u64, (FocusHandle, Option<FocusHandle>)>,
    /// Its panes moved (`WindowRoot::pane_message`): the draw that shows
    /// them hands the keys to the focused one.
    pub(in crate::shell) panes_moved: bool,
    front: Option<u64>,
    /// The keyboard holds the pane in front (⌘⇧M): see `pane_hold.rs`.
    pub(in crate::shell) holding: Option<pane_hold::Holding>,
    /// The pointer holds a pane: see `pane_drag.rs`.
    pub(in crate::shell) drag: Option<pane_drag::Drag>,
    _subscriptions: [Subscription; 3],
}

impl EventEmitter<Drawn> for PaneLayer {}

impl PaneLayer {
    #[allow(clippy::too_many_arguments, reason = "one layer, one window")]
    pub(in crate::shell) fn new(
        app: Entities,
        key: WindowKey,
        kind: WindowKind,
        own: WindowEntities,
        root: FocusHandle,
        desk_window: WeakEntity<WindowRoot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let WindowEntities { desk, overlays, .. } = own;
        let seats = app.seats.clone();
        let empty_desk = cx.new(|cx| EmptyPane::desk(&app, key, kind, root, window, cx));
        let subscriptions = [
            cx.observe_in(&desk, window, |this, _, window, cx| {
                this.reconcile(window, cx)
            }),
            cx.observe_in(&seats, window, |this, _, window, cx| {
                this.reconcile(window, cx)
            }),
            // every draw of the layer ends in the keys' handoff
            cx.subscribe_in(
                &cx.entity(),
                window,
                |this, _, drawn: &Drawn, window, cx| this.drawn(&drawn.0, window, cx),
            ),
        ];
        let mut this = Self {
            app,
            key,
            kind,
            desk,
            // an overlay that opened or closed: the next draw's handoff
            // (`keys_move`) sees it
            overlays: Observed::new(&overlays, cx),
            seats,
            window: desk_window,
            empty_desk,
            views: HashMap::new(),
            pane_keys: HashMap::new(),
            panes_moved: false,
            front: None,
            holding: None,
            drag: None,
            _subscriptions: subscriptions,
        };
        this.reconcile(window, cx);
        this
    }

    /// This window's panes.
    pub(in crate::shell) fn layout(&self, cx: &App) -> layout::Layout {
        self.desk.read(cx).get().clone()
    }

    /// The desk or the seats moved: a view for every pane, none for a pane
    /// that left, and each seat told whether its pane is in front (a
    /// compare, never a turn). A view pane whose seat is not there yet gets
    /// its view when `Seats` says the seat came.
    fn reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.layout(cx);
        let here = |instance: &u64| layout.panes.iter().any(|pane| pane.instance == *instance);
        self.pane_keys.retain(|instance, _| here(instance));
        self.views.retain(|instance, _| here(instance));
        for (index, pane) in layout.panes.iter().enumerate() {
            let seat = self.seats.read(cx).seat(pane.instance);
            if let Some(seat) = &seat {
                seat.update(cx, |seat, _| seat.set_focused(index == layout.focused));
            }
            match self.views.get(&pane.instance) {
                // the seat its view waited for came: the view is made again
                Some(view) if seat.is_some() && view.read(cx).body.awaits_seat() => {
                    self.views.remove(&pane.instance);
                }
                Some(_) => continue,
                None => {}
            }
            let own = self
                .pane_keys
                .entry(pane.instance)
                .or_insert_with(|| (cx.focus_handle(), None))
                .0
                .clone();
            let (app, key, kind, desk) = (self.app.clone(), self.key, self.kind, self.desk.clone());
            let (rail, notifications, prefs, account) = (
                app.rail.clone(),
                app.notifications.clone(),
                app.prefs.clone(),
                app.account.clone(),
            );
            let (layer, desk_window) = (cx.weak_entity(), self.window.clone());
            let (module, instance) = (pane.module, pane.instance);
            let view = cx.new(|cx| {
                let strip = cx.new(|cx| {
                    Strip::new(
                        key,
                        kind,
                        instance,
                        &desk,
                        &rail,
                        &notifications,
                        &prefs,
                        layer.clone(),
                        desk_window.clone(),
                        cx,
                    )
                });
                let body = match seat {
                    Some(seat) => Body::Seat(Observed::new(&seat, cx)),
                    None if module == layout::HELP => {
                        Body::Help(cx.new(|cx| HelpPane::new(&account, &prefs, key, cx)))
                    }
                    None if pane.is_view() => Body::Missing,
                    None => Body::Empty(cx.new(|cx| {
                        EmptyPane::window(
                            &app,
                            key,
                            instance,
                            desk_window.clone(),
                            &own,
                            window,
                            cx,
                        )
                    })),
                };
                PaneView {
                    rail: Observed::new(&rail, cx),
                    notifications: Observed::new(&notifications, cx),
                    prefs: Observed::new(&prefs, cx),
                    strip,
                    key,
                    kind,
                    instance,
                    desk,
                    layer,
                    own,
                    body,
                }
            });
            self.views.insert(pane.instance, view);
        }
        cx.notify();
    }

    /// The layer drew `shown`: the keys go where the panes say, in the
    /// frame that shows them. Whether they move is `keys_move`'s; the hold's
    /// take and release, and each pane remembering what in it had them, are
    /// read off the frame just drawn, as they were when this ran at the
    /// draw. After the draw, so a pane's first control is in the frame
    /// `focus_next` walks. A frame the model has moved on from is left
    /// alone: the draw that shows the move follows, and this runs after it.
    fn drawn(&mut self, shown: &layout::Layout, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.layout(cx);
        if *shown != layout {
            return;
        }
        let moved = self.keys_move(&layout, cx);
        // before the panes are seated, so a pane coming to the front has
        // the keys after the hold gives them back, not before
        self.sync_hold(&layout, window, cx);
        for index in layout.stacking() {
            let pane = &layout.panes[index];
            let focused = index == layout.focused;
            self.pane_focus(pane.instance, focused && moved, window, cx);
        }
    }

    /// Whether the keys go to the pane in front on this draw: another came
    /// there, whoever brought it (a key, the bar, the model), or this
    /// window moved its panes. Not while something open over the desk
    /// holds them; once it closes, and then to the new front (its close
    /// gave the keys back first, before the draw, `OverlayLayer::moved`).
    fn keys_move(&mut self, layout: &layout::Layout, cx: &mut App) -> bool {
        let covered = self.overlays.read(cx).get().is_some();
        if covered {
            return false;
        }
        let front = layout.panes.get(layout.focused).map(|pane| pane.instance);
        let turned = std::mem::replace(&mut self.front, front) != front;
        std::mem::take(&mut self.panes_moved) || turned
    }

    /// The pane with `instance` remembers what had the keys inside it.
    /// `restore` (the pane just came to the front) puts them back: what had
    /// them in it, if it is still there; else its first control; else the
    /// pane itself.
    fn pane_focus(
        &mut self,
        instance: u64,
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let holding = self.holding.is_some();
        let Some((own, last)) = self.pane_keys.get_mut(&instance) else {
            return;
        };
        // the box of a held pane has the keys, and nothing in it does
        if own.contains_focused(window, cx) && !holding {
            *last = window.focused(cx);
        }
        if !restore {
            return;
        }
        let (own, last) = (own.clone(), last.clone());
        match last {
            Some(last) if own.contains(&last, window) => last.focus(window, cx),
            _ => {
                own.focus(window, cx);
                window.focus_next(cx);
                if !own.contains_focused(window, cx) {
                    own.focus(window, cx);
                }
            }
        }
    }

    /// The desk's size, from the frame just measured, when it moved or the
    /// console's desk is still untouched: committed from the next frame's
    /// callback (`shown`), never from the draw.
    fn measured(&self, bounds: Bounds<Pixels>, cx: &App) -> Option<(f32, f32)> {
        let desk = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let layout = self.desk.read(cx).get();
        let console = self.kind == WindowKind::Console;
        (layout.desk != Some(desk) || (console && !layout.initialized)).then_some(desk)
    }

    /// The desk measured `size` (the next frame's callback): the `Desk`
    /// takes the size, and an untouched console desk opens the active
    /// program (`Windows.active`: one a link opened before the desk
    /// existed, else it starts empty), read now, not at the draw.
    fn shown(
        desk: &Entity<Desk>,
        windows: &Entity<Windows>,
        console: bool,
        size: (f32, f32),
        cx: &mut App,
    ) {
        desk.update(cx, |desk, cx| desk.resize(size, cx));
        if !console || desk.read(cx).get().initialized {
            return;
        }
        if let Some(module) = windows.read(cx).active() {
            desk.update(cx, |desk, cx| desk.seed(module, cx));
        }
    }
}

impl Render for PaneLayer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.panes", 1);
        let layout = self.layout(cx);
        // an effect, run after this draw: the keys' handoff (`drawn`), once
        // every pane the layout holds is drawn (a pane that just came is
        // drawn by the frame after its view is made)
        if layout
            .panes
            .iter()
            .all(|pane| self.views.contains_key(&pane.instance))
        {
            cx.emit(Drawn(layout.clone()));
        }
        let this = cx.entity();
        let (measure, desk, windows) = (this.clone(), self.desk.clone(), self.app.windows.clone());
        let console = self.kind == WindowKind::Console;
        // `follow` listens on every frame, not only once a hold is drawn:
        // the moves after a press may come before the next frame does
        let canvas = canvas(
            move |bounds, window, cx| {
                if let Some(size) = measure.read(cx).measured(bounds, cx) {
                    window.on_next_frame(move |_, cx| {
                        Self::shown(&desk, &windows, console, size, cx)
                    });
                }
            },
            move |bounds, _, window, _| {
                pane_drag::raise(this.clone(), bounds, window);
                pane_drag::follow(this, window);
            },
        )
        .absolute()
        .size_full();
        if layout.panes.is_empty() {
            // the window itself takes the keys once the last pane leaves:
            // the empty desk's observer gives them (`EmptyPane::moved`)
            return div()
                .relative()
                .size_full()
                .child(canvas)
                .child(
                    AnyView::from(self.empty_desk.clone())
                        .cached(StyleRefinement::default().size_full()),
                )
                .into_any_element();
        }
        let views = layout
            .stacking()
            .into_iter()
            .filter_map(|index| self.views.get(&layout.panes[index].instance).cloned());
        div()
            .id("panes")
            .relative()
            .size_full()
            .child(canvas)
            .children(views)
            .into_any_element()
    }
}

/// What a pane shows: its seat's tree or standin, the app's own Help, or
/// an empty window's finder, each a cached view of its own.
enum Body {
    Seat(Observed<Seat>),
    Empty(Entity<EmptyPane>),
    Help(Entity<HelpPane>),
    /// A view pane whose seat is not there yet: nothing, until `Seats` says.
    Missing,
}

impl Body {
    fn awaits_seat(&self) -> bool {
        matches!(self, Body::Missing)
    }
}

/// One pane, drawn: its title bar with its buttons, its body, and on the
/// desk its frame and grips. Uncached, over its cached body; it observes its
/// seat, so a seat that moved (a tree, a standin, a minimum) draws it again.
pub(in crate::shell) struct PaneView {
    key: WindowKey,
    kind: WindowKind,
    instance: u64,
    desk: Entity<Desk>,
    /// Its program's name, for the view's own label and the hold's words.
    rail: Observed<Rail>,
    /// Its view asks to notify: the strip grows a bar, and is not cached
    /// meanwhile (the bar's height is its words').
    notifications: Observed<Notifications>,
    /// Dark or light, for the frame's ink.
    prefs: Observed<Slice<Prefs>>,
    /// Its title bar and permission bar, cached.
    strip: Entity<Strip>,
    layer: WeakEntity<PaneLayer>,
    /// The pane's own focus: it holds the keys when nothing in the view does.
    own: FocusHandle,
    body: Body,
}

impl PaneView {
    /// What the pane shows: its seat's tree or standin, or else the app's
    /// own Help or an empty window's finder, each a cached view of its own.
    fn body(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.body {
            Body::Seat(seat) => {
                let seat = seat.read(cx);
                // every draw of the seat; the tree's own `renders` are the
                // cache misses among them
                crate::perf::count(
                    crate::perf::Key::View {
                        module: seat.module(),
                        instance: seat.instance(),
                    },
                    "draws",
                    1,
                );
                // a standin (a load, a failure, a stopped view) goes over
                // any tree the seat keeps: that tree is stale until the
                // next live frame, and its presentation carries over then
                match (seat.standin(), seat.tree()) {
                    (Some(standin), _) => {
                        standin.element(seat.module(), seat.instance(), seat.ax_mark())
                    }
                    (None, Some(tree)) => {
                        let guest =
                            AnyView::from(tree).cached(StyleRefinement::default().size_full());
                        let mut context = KeyContext::default();
                        context.set("ducktape_guest", format!("view{}", seat.instance()));
                        // A view owns its own inset: a split pane runs to the
                        // edges. Narrower than its minimum, it scrolls sideways
                        // rather than being squeezed and cut at the window's
                        // edge. The layer occludes: a guest under another one
                        // is never hovered, as if each were its own window.
                        div()
                            .id(seat.ax_mark())
                            .key_context(context)
                            .size_full()
                            .overflow_hidden()
                            .overflow_x_scroll()
                            .child(
                                div()
                                    .size_full()
                                    .min_w(px(seat.min_width()))
                                    .occlude()
                                    .child(guest),
                            )
                            .into_any_element()
                    }
                    (None, None) => div().size_full().into_any_element(),
                }
            }
            Body::Help(help) => AnyView::from(help.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            Body::Empty(empty) => AnyView::from(empty.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            Body::Missing => div().size_full().into_any_element(),
        }
    }

    /// The pointer takes hold of this pane (`pane_drag.rs`).
    pub(in crate::shell) fn hold(
        &self,
        index: usize,
        sides: pane_drag::Sides,
        at: Point<Pixels>,
        cx: &mut App,
    ) {
        let _ = self
            .layer
            .update(cx, |layer, cx| layer.hold(index, sides, at, cx));
    }

    /// The pane's box around `contents` (title bar, permission bar, body),
    /// seated: filling the window it is alone in, or at its frame on the
    /// desk with grips around it.
    fn place(
        &self,
        index: usize,
        layout: &layout::Layout,
        contents: Vec<AnyElement>,
        ink: &super::super::ink::Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &layout.panes[index];
        let focused = index == layout.focused;
        let multi = layout.panes.len() > 1;
        let on_desk = self.kind == WindowKind::Console && pane.frame.is_some();
        let held = on_desk
            && layout
                .held
                .is_some_and(|held| held.instance == pane.instance);
        let body = div()
            .id(SharedString::from(format!("pane/{index}")))
            .flex()
            .flex_col()
            .bg(ink.bg)
            .overflow_hidden()
            .role(Role::Group);
        match pane.frame.filter(|_| on_desk) {
            None => body.size_full().children(contents).into_any_element(),
            // on the desk: a title bar to hold it by, and edges to size
            // it by that reach past its border, so the grips sit in a
            // frame `GRAB` wider than the window (the body clips)
            Some(frame) => {
                let grab = layout::GRAB;
                let inner = body
                    // what's behind a window doesn't hear presses on it
                    .occlude()
                    .absolute()
                    .left(px(grab))
                    .top(px(grab))
                    .w(px(frame.w))
                    .h(px(frame.h))
                    .border_1()
                    .border_color(match focused && multi {
                        true => ink.ink,
                        false => ink.strong,
                    })
                    .when(focused, |pane| pane.shadow_lg())
                    .when(!focused, |pane| pane.shadow_sm())
                    .children(contents)
                    // the keyboard holds it (`pane_hold.rs`): the focus ring
                    // on its border, over the title bar
                    .when(held, |pane| {
                        pane.child(
                            div()
                                .absolute()
                                .size_full()
                                .shadow(vec![crate::a11y::ring(ink.ink)]),
                        )
                    });
                div()
                    .absolute()
                    .left(px(frame.x - grab))
                    .top(px(frame.y - grab))
                    .w(px(frame.w + 2. * grab))
                    .h(px(frame.h + 2. * grab))
                    .child(inner)
                    .children(self.grips(index, cx))
                    .into_any_element()
            }
        }
    }
}

impl Render for PaneView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = self.desk.read(cx).get().clone();
        let Some(index) = layout
            .panes
            .iter()
            .position(|pane| pane.instance == self.instance)
        else {
            // gone from its desk: dropped at the layer's next reconcile
            return div().into_any_element();
        };
        if crate::perf::on() {
            crate::perf::count(
                crate::perf::Key::Window(self.key),
                crate::runtime::intern(&format!("renders.pane.{index}")),
                1,
            );
        }
        let ink = super::super::ink::Ink::of(self.prefs.read(cx).get().dark());
        let pane = &layout.panes[index];
        let held = layout
            .held
            .is_some_and(|held| held.instance == pane.instance)
            && self.kind == WindowKind::Console;
        let rail = self.rail.read(cx).rows().to_vec();
        let view = self.body(cx);
        // it holds the pane's keys when nothing in the view does; Tab
        // never lands on it, so it offers assistive technology no focus
        // either (as the window's root). On the desk it names the chord
        // that hands it the keys (`keys::FocusPane`).
        let view = crate::a11y::Patch::default()
            .keys_fallback()
            .on(div()
                .id(SharedString::from(format!("pane/{index}/view")))
                .role(Role::Group)
                .aria_label(panes::label(&rail, pane.module))
                .when(self.kind == WindowKind::Console, |view| {
                    view.aria_keyshortcuts(chord_label(&(index + 1).to_string()))
                })
                // a held window's keys are the hold's (`pane_hold.rs`)
                .when(held, |view| {
                    view.key_context("hold")
                        .on_key_down(cx.listener(move |this, event, _, cx| {
                            let _ = this
                                .layer
                                .update(cx, |layer, cx| layer.held_key(index, event, cx));
                        }))
                        .child(
                            crate::a11y::live(
                                div().id("hold-say").role(Role::Status),
                                accesskit::Live::Polite,
                                pane_hold::hold_words(&rail, pane.module),
                            )
                            .absolute()
                            .size(px(1.))
                            .overflow_hidden(),
                        )
                })
                .track_focus(&self.own))
            .flex_1()
            .min_h_0()
            .w_full()
            .child(view);
        // Every window has a title bar, so a view owns all of its
        // rectangle: nothing floats over its corners. The strip is cached
        // at the title bar's height; while its view asks to notify it
        // grows a bar of its words' height, and draws uncached
        let asking = pane.is_view() && self.notifications.read(cx).asking(pane.module);
        let strip = match asking {
            true => self.strip.clone().into_any_element(),
            false => AnyView::from(self.strip.clone())
                .cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(px(TITLE))
                        .flex_shrink_0(),
                )
                .into_any_element(),
        };
        self.place(
            index,
            &layout,
            vec![strip, view.into_any_element()],
            &ink,
            cx,
        )
    }
}

/// What of its desk a strip shows, compared: a frame that moves leaves the
/// strip cached; a focus, a place in the stack or a pop-out draws it again.
#[derive(Clone, Copy, Default, PartialEq)]
struct Shown {
    /// The pane is on its desk (else the strip is stale, and not drawn).
    here: bool,
    index: usize,
    focused: bool,
    /// Framed on the console's desk (a title bar to hold it by).
    on_desk: bool,
    module: &'static str,
    is_view: bool,
    /// Split is offered (the desk has room for another window).
    split: bool,
}

impl Shown {
    fn of(layout: &layout::Layout, instance: u64, kind: WindowKind) -> Self {
        let Some(index) = layout
            .panes
            .iter()
            .position(|pane| pane.instance == instance)
        else {
            return Self::default();
        };
        let pane = &layout.panes[index];
        Self {
            here: true,
            index,
            focused: index == layout.focused,
            on_desk: kind == WindowKind::Console && pane.frame.is_some(),
            module: pane.module,
            is_view: pane.is_view(),
            split: layout.panes.len() < layout::MAX_PANES,
        }
    }
}

/// A pane's strip: its title bar (the handle it is held by on the desk,
/// its name, its buttons at the right end) and, while its view asks to
/// notify, the permission bar. A cached view: it observes the rail (its
/// name), the notifications (the ask), the prefs (dark, the burst) and
/// reads its desk through a compared `Shown`.
pub(in crate::shell) struct Strip {
    key: WindowKey,
    kind: WindowKind,
    layer: WeakEntity<PaneLayer>,
    window: WeakEntity<WindowRoot>,
    rail: Observed<Rail>,
    notifications: Observed<Notifications>,
    prefs: Observed<Slice<Prefs>>,
    shown: Shown,
    _desk: Subscription,
}

/// A button on a pane's title bar.
#[derive(Clone, Copy)]
enum PaneAction {
    Split,
    PopOut,
    PopIn,
    Close,
}

impl PaneAction {
    /// Its element id, its accessible name on the window of `module` (one
    /// window's Close is not another's: "Close Chat window"), its glyph.
    fn parts(
        self,
        rail: &[crate::runtime::RailRow],
        module: &str,
    ) -> (&'static str, String, gpui_kit::assets::IconName) {
        use gpui_kit::assets::IconName;
        let program = panes::label(rail, module);
        let window = match module {
            layout::EMPTY => "empty window".to_owned(),
            _ => format!("{program} window"),
        };
        match self {
            Self::Split => ("split", format!("Open another {window}"), IconName::Plus),
            Self::PopOut => (
                "popout",
                format!("Open {program} in a new window"),
                IconName::SquareArrowOutUpRight,
            ),
            Self::PopIn => (
                "popin",
                format!("Move {program} to the main window"),
                IconName::ArrowDownLeft,
            ),
            Self::Close => ("close", format!("Close {window}"), IconName::X),
        }
    }
}

impl Strip {
    #[allow(clippy::too_many_arguments, reason = "one strip, one pane")]
    fn new(
        key: WindowKey,
        kind: WindowKind,
        instance: u64,
        desk: &Entity<Desk>,
        rail: &Entity<Rail>,
        notifications: &Entity<Notifications>,
        prefs: &Entity<Slice<Prefs>>,
        layer: WeakEntity<PaneLayer>,
        window: WeakEntity<WindowRoot>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observing = cx.observe(desk, move |this, desk, cx| {
            let shown = Shown::of(desk.read(cx).get(), instance, kind);
            if shown != this.shown {
                this.shown = shown;
                cx.notify();
            }
        });
        Self {
            shown: Shown::of(desk.read(cx).get(), instance, kind),
            key,
            kind,
            layer,
            window,
            rail: Observed::new(rail, cx),
            notifications: Observed::new(notifications, cx),
            prefs: Observed::new(prefs, cx),
            _desk: observing,
        }
    }

    /// Something done to this window's panes, through the window: it hands
    /// the keys to the pane in front.
    fn pane_message(&self, message: PaneMessage, window: &mut Window, cx: &mut App) {
        let _ = self
            .window
            .update(cx, |desk, cx| desk.pane_message(message, window, cx));
    }

    /// The pointer takes hold of this pane (`pane_drag.rs`).
    fn hold(&self, index: usize, sides: pane_drag::Sides, at: Point<Pixels>, cx: &mut App) {
        let _ = self
            .layer
            .update(cx, |layer, cx| layer.hold(index, sides, at, cx));
    }

    fn pane_button(
        &self,
        index: usize,
        module: &'static str,
        action: PaneAction,
        enabled: bool,
        ink: &super::super::ink::Ink,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover = ink.surface;
        // The element id is stable for the AX door and tests; the AX name
        // is a phrase a screen reader can announce on its own, not the bare
        // verb.
        let (id, name, glyph) = action.parts(self.rail.read(cx).rows(), module);
        crate::a11y::keyboard(
            div()
                .id(SharedString::from(format!("pane/{index}/{id}")))
                .control(Role::Button, name)
                .size(px(28.))
                .text_color(ink.muted)
                .flex()
                .items_center()
                .justify_center()
                .when(enabled, |button| {
                    button.cursor_pointer().hover(move |style| style.bg(hover))
                })
                .opacity(if enabled { 1. } else { 0.35 })
                // a press on a control isn't a hold on the title bar
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if !enabled {
                        return;
                    }
                    let message = match action {
                        PaneAction::Split => PaneMessage::Split(module),
                        PaneAction::Close => PaneMessage::Close(index),
                        PaneAction::PopOut => PaneMessage::PopOut { index, at: None },
                        PaneAction::PopIn => PaneMessage::PopIn,
                    };
                    this.pane_message(message, window, cx);
                }))
                .child(gpui_kit::component::Icon::new(glyph).size(px(18.))),
            ink.ink,
        )
        .aria_disabled(!enabled)
    }

    /// The pane's title bar: the handle it is held by on the desk, its name,
    /// and its buttons at the right end.
    fn title_bar(
        &self,
        ink: &super::super::ink::Ink,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Shown {
            index,
            focused,
            on_desk,
            module,
            is_view,
            split,
            ..
        } = self.shown;
        let console = self.kind == WindowKind::Console;
        let controls = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .when(console, |strip| {
                strip.child(self.pane_button(index, module, PaneAction::Split, split, ink, cx))
            })
            // an empty or Help window has no program view to carry out
            .when(console && is_view, |strip| {
                strip.child(self.pane_button(index, module, PaneAction::PopOut, true, ink, cx))
            })
            .when(!console, |strip| {
                strip.child(self.pane_button(index, module, PaneAction::PopIn, true, ink, cx))
            })
            .child(self.pane_button(index, module, PaneAction::Close, true, ink, cx));
        // A window of its own on macOS draws no title bar of the
        // system's: this bar is its handle, and the traffic lights sit
        // over its left end. Elsewhere the system's bar names it.
        let lights = theme::traffic_lights(window).filter(|_| !on_desk);
        let handle = lights.is_some();
        div()
            .id(SharedString::from(format!("pane/{index}/strip")))
            .h(px(TITLE))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(lights.unwrap_or(12.)))
            .pr(px(2.))
            .border_b_1()
            .border_color(ink.line)
            .bg(match focused {
                true => ink.surface,
                false => ink.bg,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    match (on_desk, event.click_count) {
                        (true, 2) => this.pane_message(PaneMessage::Fill(index), window, cx),
                        (true, _) => this.hold(index, pane_drag::Sides::NONE, event.position, cx),
                        (false, 2) if handle => window.titlebar_double_click(),
                        (false, _) if handle => window.start_window_move(),
                        (false, _) => {}
                    }
                }),
            )
            .when(on_desk || handle, |title| {
                title.child(
                    super::super::ink::mono(500, 12.)
                        .flex_shrink_0()
                        .text_color(ink.ink)
                        .child(panes::label(self.rail.read(cx).rows(), module)),
                )
            })
            // pushes the controls to the bar's right end
            .child(div().flex_1().min_w_0())
            .child(controls)
            .into_any_element()
    }

    /// The NotifPermission board: a view posted before the person said
    /// anything about its notices, so its window asks, at the top. Until
    /// they answer its notices wait in Notifications, silently.
    fn permission_bar(
        &self,
        module: &'static str,
        ink: &super::super::ink::Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use super::super::ink::*;
        use crate::runtime::notify::Permission;
        let name = panes::label(self.rail.read(cx).rows(), module);
        let burst = self.prefs.read(cx).get().notify.burst;
        let (notifications, prefs) = (
            self.notifications.entity().clone(),
            self.prefs.entity().clone(),
        );
        let button =
            |id: &'static str, text: &'static str, filled: bool, answer: Box<dyn Fn(&mut App)>| {
                crate::a11y::keyboard(
                    sans(500, 14.)
                        .id(SharedString::from(format!("notify-ask/{module}/{id}")))
                        .control(Role::Button, text)
                        .h(px(tall(30.)))
                        .px(px(12.))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .cursor_pointer()
                        .border(px(1.5))
                        .border_color(ink.ink)
                        .when(filled, |button| button.bg(ink.ink).text_color(ink.bg))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            answer(cx);
                        })
                        .child(text),
                    ink.ring(filled),
                )
            };
        // announced as it appears: nothing else says a view is waiting
        crate::a11y::live(
            div()
                .id(SharedString::from(format!("notify-ask/{module}")))
                .role(Role::Group),
            accesskit::Live::Polite,
            format!("{name} wants to show desktop notifications"),
        )
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(10.))
        .bg(ink.surface)
        .border_b_1()
        .border_color(ink.line)
        .child(
            gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Bell)
                .size(px(14.))
                .text_color(ink.ink),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    sans(500, 14.)
                        .text_color(ink.ink)
                        .child(format!("{name} wants to show desktop notifications")),
                )
                .child(note(
                    "burst",
                    format!("At most {burst} banners a minute; the rest wait in Notifications."),
                    ink.muted,
                )),
        )
        .child(button("allow", "Allow", true, {
            let (notifications, prefs) = (notifications.clone(), prefs.clone());
            Box::new(move |cx| {
                super::super::entities::permission(
                    &notifications,
                    &prefs,
                    module,
                    Permission::Allow,
                    cx,
                )
            })
        }))
        .child(button(
            "not-now",
            "Not now",
            false,
            Box::new(move |cx| {
                notifications.update(cx, |notifications, cx| notifications.not_now(module, cx))
            }),
        ))
        .into_any_element()
    }
}

impl Render for Strip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Shown {
            here,
            index,
            module,
            is_view,
            ..
        } = self.shown;
        if !here {
            // gone from its desk: dropped with its pane view
            return div().into_any_element();
        }
        if crate::perf::on() {
            crate::perf::count(
                crate::perf::Key::Window(self.key),
                crate::runtime::intern(&format!("renders.strip.{index}")),
                1,
            );
        }
        let ink = super::super::ink::Ink::of(self.prefs.read(cx).get().dark());
        let title = self.title_bar(&ink, window, cx);
        let asking = (is_view && self.notifications.read(cx).asking(module))
            .then(|| self.permission_bar(module, &ink, cx));
        div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(title)
            .children(asking)
            .into_any_element()
    }
}
