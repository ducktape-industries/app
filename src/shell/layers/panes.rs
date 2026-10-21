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
use super::{EmptyPane, HelpPane, MemoryPane};
use crate::a11y::Control as _;
use crate::runtime::Seat;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;

pub(in crate::shell) const TITLE: f32 = 32.;
pub(in crate::shell) const FILL_STRIP: f32 = 36.;
pub(in crate::shell) const BORDER: f32 = 1.;

pub(in crate::shell) fn view_body(
    on_desk: bool,
    layout: &layout::Layout,
    index: usize,
) -> Option<(f32, f32)> {
    let desk = layout.desk;
    match layout.panes[index].frame.filter(|_| on_desk) {
        Some(_) if layout.fills(index) => desk.map(|(width, height)| (width, height - FILL_STRIP)),
        Some(frame) => Some((frame.w - 2. * BORDER, frame.h - 2. * BORDER - TITLE)),
        None => desk.map(|(width, height)| (width, height - TITLE)),
    }
}

pub(in crate::shell) fn strip_name(
    label: &str,
    title: Option<&str>,
    filled: bool,
    ink: &super::super::ink::Ink,
) -> AnyElement {
    use super::super::ink::{mono, sans};
    let name = |first: Div, title: &str, second: Div| {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .child(first.flex_shrink_0().child(label.to_owned()))
            .child(second.min_w_0().truncate().child(title.to_owned()))
            .into_any_element()
    };
    match (filled, title) {
        (false, None) => mono(500, 12.)
            .flex_shrink_0()
            .text_color(ink.ink)
            .child(label.to_owned())
            .into_any_element(),
        (false, Some(title)) => name(
            mono(500, 12.).text_color(ink.ink),
            title,
            sans(400, 12.5).text_color(ink.muted),
        ),
        (true, None) => sans(500, 13.)
            .flex_shrink_0()
            .text_color(ink.ink)
            .child(label.to_owned())
            .into_any_element(),
        (true, Some(title)) => name(
            sans(400, 13.).text_color(ink.muted),
            title,
            sans(500, 13.).text_color(ink.ink),
        ),
    }
}

struct Drawn(layout::Layout);

pub(in crate::shell) struct PaneLayer {
    pub(in crate::shell) app: Entities,
    pub(in crate::shell) key: WindowKey,
    pub(in crate::shell) kind: WindowKind,
    pub(in crate::shell) desk: Entity<Desk>,
    pub(in crate::shell) overlays: Observed<Overlays>,
    seats: Entity<Seats>,
    pub(in crate::shell) window: WeakEntity<WindowRoot>,
    empty_desk: Entity<EmptyPane>,
    views: HashMap<u64, Entity<PaneView>>,
    pub(in crate::shell) pane_keys: HashMap<u64, (FocusHandle, Option<FocusHandle>)>,
    pub(in crate::shell) panes_moved: bool,
    front: Option<u64>,
    pub(in crate::shell) holding: Option<pane_hold::Holding>,
    pub(in crate::shell) drag: Option<pane_drag::Drag>,
    _subscriptions: [Subscription; 4],
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
            cx.observe_in(&overlays, window, |this, _, window, cx| {
                this.reconcile(window, cx)
            }),
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

    pub(in crate::shell) fn layout(&self, cx: &App) -> layout::Layout {
        self.desk.read(cx).get().clone()
    }

    fn reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.layout(cx);
        let here = |instance: &u64| layout.panes.iter().any(|pane| pane.instance == *instance);
        self.pane_keys.retain(|instance, _| here(instance));
        self.views.retain(|instance, _| here(instance));
        let desk_keys = self.overlays.read(cx).get().is_none() && layout.held.is_none();
        for (index, pane) in layout.panes.iter().enumerate() {
            let seat = self.seats.read(cx).seat(pane.instance);
            if let Some(seat) = &seat {
                let keys_free = desk_keys && index == layout.focused;
                seat.update(cx, |seat, cx| seat.set_keys_free(keys_free, cx));
            }
            match self.views.get(&pane.instance) {
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
            let (program, instance) = (pane.program, pane.instance);
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
                    None if program == layout::HELP => {
                        Body::Help(cx.new(|cx| HelpPane::new(&account, &prefs, key, cx)))
                    }
                    None if program == layout::MEMORY => {
                        Body::Memory(cx.new(|_| MemoryPane::new(&app, key)))
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

    fn drawn(&mut self, shown: &layout::Layout, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.layout(cx);
        if *shown != layout {
            return;
        }
        let moved = self.front_pane_or_layout_changed(&layout, cx);
        self.sync_hold(&layout, window, cx);
        for index in layout.stacking() {
            let pane = &layout.panes[index];
            let focused = index == layout.focused;
            self.pane_focus(pane.instance, focused && moved, window, cx);
        }
    }

    fn front_pane_or_layout_changed(&mut self, layout: &layout::Layout, cx: &mut App) -> bool {
        let covered = self.overlays.read(cx).get().is_some();
        if covered {
            return false;
        }
        let front = layout.panes.get(layout.focused).map(|pane| pane.instance);
        let turned = std::mem::replace(&mut self.front, front) != front;
        std::mem::take(&mut self.panes_moved) || turned
    }

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

    fn measured(&self, bounds: Bounds<Pixels>, cx: &App) -> Option<(f32, f32)> {
        let desk = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let layout = self.desk.read(cx).get();
        let console = self.kind == WindowKind::Console;
        (layout.desk != Some(desk) || (console && !layout.initialized)).then_some(desk)
    }

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
        if let Some(program) = windows.read(cx).active() {
            desk.update(cx, |desk, cx| desk.seed(program, cx));
        }
    }
}

impl Render for PaneLayer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.panes", 1);
        let layout = self.layout(cx);
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
            return div()
                .relative()
                .size_full()
                .child(canvas)
                .child(ducktape_host::a11y::cached(
                    AnyView::from(self.empty_desk.clone()),
                    StyleRefinement::default().size_full(),
                ))
                .into_any_element();
        }
        let views = layout
            .stacking()
            .into_iter()
            .filter(|&index| !layout.filled || index == layout.focused)
            .enumerate()
            .filter_map(|(rank, index)| {
                let view = self.views.get(&layout.panes[index].instance)?;
                Some(crate::render::deferred::clipped(
                    view.clone().into_any_element(),
                    rank,
                ))
            });
        div()
            .id("panes")
            .relative()
            .size_full()
            .overflow_hidden()
            .child(canvas)
            .children(views)
            .into_any_element()
    }
}

/// What a pane shows: its seat's tree or standin, the app's own Help, or
/// an empty window's finder, each a cached view of its own; or debug mode's
/// Memory window, uncached, so it reads on every frame the window draws.
enum Body {
    Seat(Observed<Seat>),
    Empty(Entity<EmptyPane>),
    Help(Entity<HelpPane>),
    Memory(Entity<MemoryPane>),
    /// A view pane whose seat is not there yet: nothing, until `Seats` says.
    Missing,
}

impl Body {
    fn awaits_seat(&self) -> bool {
        matches!(self, Body::Missing)
    }
}

pub(in crate::shell) struct PaneView {
    key: WindowKey,
    kind: WindowKind,
    instance: u64,
    desk: Entity<Desk>,
    rail: Observed<Rail>,
    notifications: Observed<Notifications>,
    prefs: Observed<Slice<Prefs>>,
    strip: Entity<Strip>,
    layer: WeakEntity<PaneLayer>,
    own: FocusHandle,
    body: Body,
}

impl PaneView {
    fn body(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.body {
            Body::Seat(seat) => {
                let seat = seat.read(cx);
                crate::perf::count(
                    crate::perf::Key::View {
                        program: seat.program(),
                        instance: seat.instance(),
                    },
                    "draws",
                    1,
                );
                match (seat.standin(), seat.tree()) {
                    (Some(standin), _) => standin.element(
                        seat.network(),
                        seat.program(),
                        seat.instance(),
                        seat.ax_mark(),
                    ),
                    (None, Some(tree)) => {
                        let view = ducktape_host::a11y::cached(
                            AnyView::from(tree),
                            StyleRefinement::default().size_full(),
                        );
                        let mut context = KeyContext::default();
                        context.set("ducktape_view", format!("view{}", seat.instance()));
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
                                    .child(view),
                            )
                            .into_any_element()
                    }
                    (None, None) => div().size_full().into_any_element(),
                }
            }
            Body::Help(help) => ducktape_host::a11y::cached(
                AnyView::from(help.clone()),
                StyleRefinement::default().size_full(),
            )
            .into_any_element(),
            Body::Empty(empty) => ducktape_host::a11y::cached(
                AnyView::from(empty.clone()),
                StyleRefinement::default().size_full(),
            )
            .into_any_element(),
            Body::Memory(memory) => AnyView::from(memory.clone()).into_any_element(),
            Body::Missing => div().size_full().into_any_element(),
        }
    }

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
        let filled = on_desk && layout.fills(index);
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
            Some(_) if filled => body
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .children(contents)
                .into_any_element(),
            Some(frame) => {
                let grab = layout::GRAB;
                let inner = body
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
        let view = crate::a11y::Patch::default()
            .keys_fallback()
            .on(div()
                .id(SharedString::from(format!("pane/{index}/view")))
                .role(Role::Group)
                .aria_label(match &pane.title {
                    Some(title) => format!("{}, {title}", panes::label(&rail, pane.program)),
                    None => panes::label(&rail, pane.program),
                })
                .when(self.kind == WindowKind::Console, |view| {
                    view.aria_keyshortcuts(chord_label(&(index + 1).to_string()))
                })
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
                                pane_hold::hold_words(&rail, pane.program),
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
        let notifications = self.notifications.read(cx);
        let strip_grows = pane.is_view()
            && (notifications.asking(pane.program) || notifications.sharing(pane.program));
        let filled =
            self.kind == WindowKind::Console && pane.frame.is_some() && layout.fills(index);
        let strip = match strip_grows {
            true => self.strip.clone().into_any_element(),
            false => ducktape_host::a11y::cached(
                AnyView::from(self.strip.clone()),
                StyleRefinement::default()
                    .w_full()
                    .h(px(if filled { FILL_STRIP } else { TITLE }))
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

#[derive(Clone, Default, PartialEq)]
struct Shown {
    here: bool,
    index: usize,
    focused: bool,
    on_desk: bool,
    filled: bool,
    title: Option<String>,
    program: &'static str,
    is_view: bool,
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
        let on_desk = kind == WindowKind::Console && pane.frame.is_some();
        Self {
            here: true,
            index,
            focused: index == layout.focused,
            on_desk,
            filled: on_desk && layout.fills(index),
            title: pane.title.clone(),
            program: pane.program,
            is_view: pane.is_view(),
        }
    }
}

pub(in crate::shell) struct Strip {
    key: WindowKey,
    kind: WindowKind,
    layer: WeakEntity<PaneLayer>,
    window: WeakEntity<WindowRoot>,
    rail: Observed<Rail>,
    notifications: Observed<Notifications>,
    prefs: Observed<Slice<Prefs>>,
    shown: Shown,
    os_title: String,
    _desk: Subscription,
}

#[derive(Clone, Copy)]
enum PaneAction {
    Split,
    Restore,
    PopOut,
    PopIn,
    Close,
}

impl PaneAction {
    fn parts(
        self,
        rail: &[crate::runtime::RailRow],
        program: &str,
    ) -> (&'static str, String, gpui_kit::assets::IconName) {
        use gpui_kit::assets::IconName;
        let label = panes::label(rail, program);
        let window = match program {
            layout::EMPTY => "empty window".to_owned(),
            _ => format!("{label} window"),
        };
        match self {
            Self::Split => ("split", format!("Open another {window}"), IconName::Plus),
            Self::Restore => (
                "restore",
                "Put every window back".to_owned(),
                IconName::WindowRestore,
            ),
            Self::PopOut => (
                "popout",
                format!("Open {label} in a new window"),
                IconName::SquareArrowOutUpRight,
            ),
            Self::PopIn => (
                "popin",
                format!("Move {label} to the main window"),
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
            os_title: String::new(),
            _desk: observing,
        }
    }

    fn pane_message(&self, message: PaneMessage, window: &mut Window, cx: &mut App) {
        let _ = self
            .window
            .update(cx, |desk, cx| desk.pane_message(message, window, cx));
    }

    fn hold(&self, index: usize, sides: pane_drag::Sides, at: Point<Pixels>, cx: &mut App) {
        let _ = self
            .layer
            .update(cx, |layer, cx| layer.hold(index, sides, at, cx));
    }

    fn pane_button(
        &self,
        index: usize,
        program: &'static str,
        action: PaneAction,
        ink: &super::super::ink::Ink,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover = ink.surface;
        let (id, name, glyph) = action.parts(self.rail.read(cx).rows(), program);
        crate::a11y::keyboard(
            div()
                .id(SharedString::from(format!("pane/{index}/{id}")))
                .control(Role::Button, name)
                .size(px(28.))
                .text_color(ink.muted)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    let message = match action {
                        PaneAction::Split => PaneMessage::Split(program),
                        PaneAction::Restore => PaneMessage::Fill,
                        PaneAction::Close => PaneMessage::Close(index),
                        PaneAction::PopOut => PaneMessage::PopOut { index, at: None },
                        PaneAction::PopIn => PaneMessage::PopIn,
                    };
                    this.pane_message(message, window, cx);
                }))
                .child(gpui_kit::component::Icon::new(glyph).size(px(18.))),
            ink.ink,
        )
    }

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
            filled,
            program,
            is_view,
            ..
        } = self.shown;
        let console = self.kind == WindowKind::Console;
        let controls = div()
            .flex()
            .items_center()
            .gap(px(if filled { 8. } else { 2. }))
            .when(console, |strip| {
                strip.child(self.pane_button(index, program, PaneAction::Split, ink, cx))
            })
            .when(filled, |strip| {
                strip.child(self.pane_button(index, program, PaneAction::Restore, ink, cx))
            })
            .when(console && is_view, |strip| {
                strip.child(self.pane_button(index, program, PaneAction::PopOut, ink, cx))
            })
            .when(!console, |strip| {
                strip.child(self.pane_button(index, program, PaneAction::PopIn, ink, cx))
            })
            .child(self.pane_button(index, program, PaneAction::Close, ink, cx));
        let lights = theme::traffic_lights(window).filter(|_| !on_desk);
        let handle = lights.is_some();
        let (height, left, right) = match filled {
            true => (FILL_STRIP, 16., 4.),
            false => (TITLE, lights.unwrap_or(12.), 2.),
        };
        div()
            .id(SharedString::from(format!("pane/{index}/strip")))
            .h(px(height))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(left))
            .pr(px(right))
            .border_b_1()
            .border_color(ink.line)
            .bg(match focused && !filled {
                true => ink.surface,
                false => ink.bg,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    match (on_desk, event.click_count) {
                        (true, 2) => this.pane_message(PaneMessage::Fill, window, cx),
                        (true, _) if filled => {}
                        (true, _) => this.hold(index, pane_drag::Sides::NONE, event.position, cx),
                        (false, 2) if handle => window.titlebar_double_click(),
                        (false, _) if handle => window.start_window_move(),
                        (false, _) => {}
                    }
                }),
            )
            .when(on_desk || handle, |title| {
                title.child(strip_name(
                    &panes::label(self.rail.read(cx).rows(), program),
                    self.shown.title.as_deref(),
                    filled,
                    ink,
                ))
            })
            .child(div().flex_1().min_w_0())
            .child(controls)
            .into_any_element()
    }

    fn live_bar(
        &self,
        program: &'static str,
        ink: &super::super::ink::Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use super::super::ink::*;
        let name = panes::label(self.rail.read(cx).rows(), program);
        let line =
            format!("{name} shares live data with the people in its rooms, through your node.");
        let notifications = self.notifications.entity().clone();
        let ok = bar_button(
            format!("live-line/{program}/ok"),
            "OK",
            false,
            ink,
            Box::new(move |cx| {
                notifications.update(cx, |notifications, cx| {
                    notifications.put_away_live(program, cx)
                })
            }),
        );
        bar(format!("live-line/{program}"), line.clone(), ink)
            .child(
                sans(500, 14.)
                    .flex_1()
                    .min_w_0()
                    .text_color(ink.ink)
                    .child(line),
            )
            .child(ok)
            .into_any_element()
    }

    fn permission_bar(
        &self,
        program: &'static str,
        ink: &super::super::ink::Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use super::super::ink::*;
        use crate::runtime::notify::Permission;
        let name = panes::label(self.rail.read(cx).rows(), program);
        let burst = self.prefs.read(cx).get().notify.burst;
        let (notifications, prefs) = (
            self.notifications.entity().clone(),
            self.prefs.entity().clone(),
        );
        let button = |id: &str, text: &'static str, filled: bool, answer: Box<dyn Fn(&mut App)>| {
            bar_button(
                format!("notify-ask/{program}/{id}"),
                text,
                filled,
                ink,
                answer,
            )
        };
        bar(
            format!("notify-ask/{program}"),
            format!("{name} wants to show desktop notifications"),
            ink,
        )
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
            let notifications = notifications.clone();
            Box::new(move |cx| {
                super::super::entities::permission(
                    &notifications,
                    &prefs,
                    program,
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
                notifications.update(cx, |notifications, cx| notifications.not_now(program, cx))
            }),
        ))
        .into_any_element()
    }
}

fn bar(id: String, announced: String, ink: &super::super::ink::Ink) -> Stateful<Div> {
    crate::a11y::live(
        div().id(SharedString::from(id)).role(Role::Group),
        accesskit::Live::Polite,
        announced,
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
}

fn bar_button(
    id: String,
    text: &'static str,
    filled: bool,
    ink: &super::super::ink::Ink,
    answer: Box<dyn Fn(&mut App)>,
) -> AnyElement {
    use super::super::ink::{sans, tall};
    crate::a11y::keyboard(
        sans(500, 14.)
            .id(SharedString::from(id))
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
    .into_any_element()
}

impl Render for Strip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Shown {
            here,
            index,
            program,
            is_view,
            ..
        } = self.shown;
        if !here {
            return div().into_any_element();
        }
        if crate::perf::on() {
            crate::perf::count(
                crate::perf::Key::Window(self.key),
                crate::runtime::intern(&format!("renders.strip.{index}")),
                1,
            );
        }
        if self.kind != WindowKind::Console {
            let label = panes::label(self.rail.read(cx).rows(), program);
            let name = match &self.shown.title {
                Some(title) => format!("{label} — {title}"),
                None => label,
            };
            if name != self.os_title {
                window.set_window_title(&name);
                self.os_title = name;
            }
        }
        let ink = super::super::ink::Ink::of(self.prefs.read(cx).get().dark());
        let title = self.title_bar(&ink, window, cx);
        let asking = (is_view && self.notifications.read(cx).asking(program))
            .then(|| self.permission_bar(program, &ink, cx));
        let sharing = (is_view && self.notifications.read(cx).sharing(program))
            .then(|| self.live_bar(program, &ink, cx));
        div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(title)
            .children(asking)
            .children(sharing)
            .into_any_element()
    }
}
