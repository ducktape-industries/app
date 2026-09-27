//! The desk's panes. `Desktop::mount` gives every view pane the model
//! holds a program view and keeps it across OS windows; `pane_stage` draws
//! each pane with its title bar, buttons and grips, or the empty desk when
//! there is none; the permission bar asks about a view's notices; the
//! pointer drags, sizes and raises panes. Where panes sit, stack and which
//! has the keys is `ui::layout`'s: this file only draws it and sends
//! `PaneMessage`s.
use super::*;

/// The program view shown in the pane with this layout `instance`
/// (`layout::Pane.instance`; the view counts its own, unrelated
/// `NativeModuleView.instance`).
pub(super) struct MountedPane {
    pub(super) module: &'static str,
    pub(super) view: Entity<crate::runtime::NativeModuleView>,
    /// The view's intents, to the model as `Message::ViewEvent`; dropping
    /// it unsubscribes.
    _route: gpui_kit::Subscription,
}

impl Desktop {
    /// A view for every pane the model has, and none for a pane it no
    /// longer has (told it is hidden first).
    pub(super) fn mount(&mut self, cx: &mut Context<Self>) {
        let wanted: BTreeMap<u64, &'static str> = self
            .state
            .layouts
            .values()
            .flat_map(|layout| &layout.panes)
            .filter(|pane| pane.is_view())
            .map(|pane| (pane.instance, pane.module))
            .collect();
        let gone: Vec<u64> = self
            .mounted
            .keys()
            .filter(|instance| !wanted.contains_key(instance))
            .copied()
            .collect();
        for (instance, module) in wanted {
            if self.mounted.contains_key(&instance) {
                continue;
            }
            let view = cx.new(|_| crate::runtime::NativeModuleView::new(module));
            let route = cx.subscribe(&view, move |model, _, event, cx| {
                model.dispatch(Message::ViewEvent(module, event.clone()), cx)
            });
            self.mounted.insert(
                instance,
                MountedPane {
                    module,
                    view,
                    _route: route,
                },
            );
        }
        for instance in gone {
            let Some(pane) = self.mounted.remove(&instance) else {
                continue;
            };
            let intents = pane.view.update(cx, |view, _| view.hide());
            for intent in intents {
                self.dispatch(Message::ViewEvent(pane.module, intent), cx);
            }
        }
    }
}

pub(super) fn label(module: &str) -> String {
    if module == layout::EMPTY {
        return "Empty".to_owned();
    }
    if module == layout::HELP {
        return "Help".to_owned();
    }
    crate::runtime::rail()
        .into_iter()
        .find(|row| row.module == module)
        .map(|row| row.label)
        .unwrap_or_else(|| module.to_owned())
}

/// What an empty desk says when it has nothing to offer: `None` when there
/// is something to open, and the desk shows its buttons instead.
fn empty_panes_message(rail: &[crate::runtime::RailRow]) -> Option<&'static str> {
    (!rail.iter().any(|row| !row.empty)).then_some("This network runs no program with a view.")
}

/// An empty desk's way out: the chord and what it does, and a press does
/// what the chord would.
fn desk_button(
    id: &'static str,
    key: &str,
    name: &'static str,
    ink: &super::ink::Ink,
    action: fn() -> Box<dyn gpui_kit::Action>,
) -> gpui_kit::AnyElement {
    use super::ink::*;
    use gpui_kit::*;
    let hover = ink.surface;
    let button = sans(500, 14.)
        .id(id)
        .control(Role::Button, SharedString::from(name))
        .h(px(tall(34.)))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(10.))
        .border_1()
        .border_color(ink.line)
        .text_color(ink.ink)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            window.dispatch_action(action(), cx);
        })
        .child(mono(400, 12.).text_color(ink.muted).child(chord_label(key)))
        .child(name);
    crate::a11y::keyboard(button).into_any_element()
}

/// What an empty window lists: the rail's programs, as the menu bar shows them.
pub(super) fn openable() -> Vec<crate::runtime::RailRow> {
    crate::runtime::rail()
        .into_iter()
        .filter(|row| !row.empty)
        .collect()
}

impl DesktopWindow {
    /// Something done to this window's panes: the model moves them, and
    /// the keys go to the focused one (`pane_stage`).
    pub(super) fn pane_message(
        &mut self,
        message: PaneMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = match message {
            // it opens where it sat on the desk
            PaneMessage::PopOut { index, at: None } => PaneMessage::PopOut {
                index,
                at: Some(super::windows::unseated(
                    window.bounds(),
                    self.layout(cx).panes.get(index).and_then(|pane| pane.frame),
                    window.display(cx).map(|display| display.bounds()),
                )),
            },
            message => message,
        };
        let key = self.key;
        self.model.update(cx, |model, cx| {
            model.dispatch(Message::Pane(key, message), cx)
        });
        self.panes_moved = true;
        cx.notify();
    }

    /// A menu bar click, or a pick in an empty window: see `Layout::open`.
    pub(super) fn open_view(
        &mut self,
        module: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_message(PaneMessage::Open(module), window, cx);
        self.clear_command(window, cx);
    }

    fn pane_button(
        &self,
        index: usize,
        action: PaneAction,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::*;
        let ink = super::ink::Ink::of(self.model.read(cx).state.dark());
        let hover = ink.surface;
        // The element id is stable for the AX door and tests; the AX name
        // is a phrase a screen reader can announce on its own, not the bare
        // verb.
        let (id, name, glyph) = action.parts();
        crate::a11y::disabled(
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
                            PaneAction::Split => match this.layout(cx).panes.get(index) {
                                Some(pane) => PaneMessage::Split(pane.module),
                                None => return,
                            },
                            PaneAction::Close => PaneMessage::Close(index),
                            PaneAction::PopOut => PaneMessage::PopOut { index, at: None },
                            PaneAction::PopIn => PaneMessage::PopIn,
                        };
                        this.pane_message(message, window, cx);
                    }))
                    .child(gpui_kit::component::Icon::new(glyph).size(px(18.))),
            ),
            !enabled,
        )
    }

    /// The desk windows sit on: the window below its bar.
    pub(super) fn desk(&self, window: &Window) -> (f32, f32) {
        let size = window.viewport_size();
        (
            f32::from(size.width),
            f32::from(size.height) - super::desk::BAR,
        )
    }

    pub(super) fn pane_stage(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let ink = super::ink::Ink::of(self.model.read(cx).state.dark());
        let layout = self.layout(cx);
        let moved = std::mem::take(&mut self.panes_moved);
        if layout.panes.is_empty() {
            if moved {
                self.focus.focus(window, cx);
            }
            let moving = self.model.read(cx).state.motion;
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(20.))
                .child(super::spin::drawing(
                    "empty-desk-figure",
                    super::figure::Figure::Roll,
                    moving,
                    ink.figure,
                    window,
                    cx,
                ))
                .child(match empty_panes_message(&crate::runtime::rail()) {
                    Some(message) => super::ink::mono(400, 12.)
                        .text_color(ink.muted)
                        .child(message)
                        .into_any_element(),
                    None => div()
                        .flex()
                        .gap(px(12.))
                        .child(desk_button(
                            "empty-desk/new",
                            "N",
                            "New window",
                            &ink,
                            || Box::new(super::keys::NewWindow),
                        ))
                        .child(desk_button(
                            "empty-desk/search",
                            "K",
                            "Search",
                            &ink,
                            || Box::new(super::keys::ToggleSpotlight),
                        ))
                        .child(desk_button("empty-desk/help", "/", "Help", &ink, || {
                            Box::new(super::keys::OpenHelp)
                        }))
                        .into_any_element(),
                })
                .into_any_element();
        }
        let props = self.model.read(cx).state.view_props();
        let console = self.kind == crate::shell::WindowKind::Console;
        let this = cx.entity();
        let mut stage = div().id("panes").relative().size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| raise(this, bounds, window),
            )
            .absolute()
            .size_full(),
        );
        if self.drag.is_some() {
            // window-wide, so a fast pointer can't slip off the window it holds
            let this = cx.entity();
            stage = stage.child(
                canvas(|_, _, _| {}, move |_, _, window, _| follow(this, window))
                    .absolute()
                    .size_full(),
            );
        }
        let multi = layout.panes.len() > 1;
        self.pane_keys
            .retain(|instance, _| layout.panes.iter().any(|pane| pane.instance == *instance));
        for index in layout.stacking() {
            let pane = &layout.panes[index];
            let focused = index == layout.focused;
            let (own, last) = self
                .pane_keys
                .entry(pane.instance)
                .or_insert_with(|| (cx.focus_handle(), None));
            if own.contains_focused(window, cx) {
                *last = window.focused(cx);
            }
            let (own, last) = (own.clone(), last.clone());
            if focused && moved {
                // what had the keys in it, if it is still there; else its
                // first control; else the window itself
                let own = own.clone();
                window.defer(cx, move |window, cx| match last {
                    Some(last) if own.contains(&last, window) => last.focus(window, cx),
                    _ => {
                        own.focus(window, cx);
                        window.focus_next(cx);
                        if !own.contains_focused(window, cx) {
                            own.focus(window, cx);
                        }
                    }
                });
            }
            let view_in = pane.is_view();
            let mounted = self
                .model
                .read(cx)
                .mounted
                .get(&pane.instance)
                .map(|mounted| mounted.view.clone());
            let view = match mounted {
                Some(view) => {
                    view.update(cx, |view, cx| {
                        view.set_focused(focused, cx);
                        view.set_props(props.clone(), cx);
                    });
                    view.into_any_element()
                }
                None => {
                    // the bare window box has the keys: the field takes them
                    if focused && own.is_focused(window) {
                        self.focus_command(window, cx);
                    }
                    match pane.module == layout::HELP {
                        true => self.help_view(cx),
                        false => self.command_view(focused, window, cx),
                    }
                }
            };
            let view = div()
                .id(SharedString::from(format!("pane/{index}/view")))
                .role(gpui_kit::Role::Group)
                .aria_label(label(pane.module))
                .track_focus(&own)
                .flex_1()
                .min_h_0()
                .w_full()
                .child(view);
            let controls = div()
                .flex()
                .items_center()
                .gap(px(2.))
                .when(console, |strip| {
                    strip.child(self.pane_button(
                        index,
                        PaneAction::Split,
                        layout.panes.len() < layout::MAX_PANES,
                        cx,
                    ))
                })
                // an empty or Help window has no program view to carry out
                .when(console && view_in, |strip| {
                    strip.child(self.pane_button(index, PaneAction::PopOut, true, cx))
                })
                .when(!console, |strip| {
                    strip.child(self.pane_button(index, PaneAction::PopIn, true, cx))
                })
                .child(self.pane_button(index, PaneAction::Close, true, cx));
            let body = div()
                .id(SharedString::from(format!("pane/{index}")))
                .flex()
                .flex_col()
                .bg(ink.bg)
                .overflow_hidden()
                .role(gpui_kit::Role::Group);
            // Every window has a title bar, so a view owns all of its
            // rectangle: nothing floats over its corners.
            let on_desk = console && pane.frame.is_some();
            // A window of its own on macOS draws no title bar of the
            // system's: this bar is its handle, and the traffic lights sit
            // over its left end. Elsewhere the system's bar names it.
            let lights = theme::traffic_lights(window).filter(|_| !on_desk);
            let handle = lights.is_some();
            let title = div()
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
                            (true, _) => this.hold(index, [false; 4], event.position, cx),
                            (false, 2) if handle => window.titlebar_double_click(),
                            (false, _) if handle => window.start_window_move(),
                            (false, _) => {}
                        }
                    }),
                )
                .when(on_desk || handle, |title| {
                    title.child(
                        super::ink::mono(500, 12.)
                            .flex_shrink_0()
                            .text_color(ink.ink)
                            .child(label(pane.module)),
                    )
                })
                // pushes the controls to the bar's right end
                .child(div().flex_1().min_w_0())
                .child(controls);
            let asking = (view_in && crate::runtime::notify::center().asking(pane.module))
                .then(|| self.permission_bar(pane.module, cx));
            let seated = match pane.frame.filter(|_| on_desk) {
                None => body
                    .size_full()
                    .child(title)
                    .children(asking)
                    .child(view)
                    .into_any_element(),
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
                        .child(title)
                        .children(asking)
                        .child(view);
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
            };
            stage = stage.child(seated);
        }
        stage.into_any_element()
    }

    /// The NotifPermission board: a view posted before the person said
    /// anything about its notices, so its window asks, at the top. Until
    /// they answer its notices wait in Notifications, silently.
    fn permission_bar(&self, module: &'static str, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use super::ink::*;
        use crate::runtime::notify::{self, Permission};
        use gpui_kit::*;
        let ink = Ink::of(self.model.read(cx).state.dark());
        let name = super::panes::label(module);
        let burst = notify::Settings::load().burst;
        let button = |id: &'static str, text: &'static str, filled: bool, message: Message| {
            let model = self.model.clone();
            let message = std::cell::RefCell::new(Some(message));
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
                        if let Some(message) = message.borrow_mut().take() {
                            model.update(cx, |model, cx| model.dispatch(message, cx));
                        }
                    })
                    .child(text),
            )
        };
        div()
            .id(SharedString::from(format!("notify-ask/{module}")))
            .role(Role::Group)
            .aria_label(SharedString::from(format!(
                "{name} wants to show desktop notifications"
            )))
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
                        format!(
                            "At most {burst} banners a minute; the rest wait in Notifications."
                        ),
                        ink.muted,
                    )),
            )
            .child(button(
                "allow",
                "Allow",
                true,
                Message::NotifyPermission(module, Permission::Allow),
            ))
            .child(button(
                "not-now",
                "Not now",
                false,
                Message::NotifyNotNow(module),
            ))
            .into_any_element()
    }

    /// Takes hold of window `index` at `at`: `sides` (left, top, right,
    /// bottom) follow the pointer; none, and the whole window does.
    fn hold(
        &mut self,
        index: usize,
        sides: [bool; 4],
        at: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &gpui_kit::App,
    ) {
        if let Some(start) = self.layout(cx).panes.get(index).and_then(|pane| pane.frame) {
            self.drag = Some(Drag {
                index,
                sides,
                from: (at.x.into(), at.y.into()),
                start,
            });
        }
    }

    /// The edges and corners a window is sized by: each reaches `GRAB`
    /// out past the border and `IN` over it, a corner further in.
    fn grips(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<gpui_kit::Stateful<gpui_kit::Div>> {
        use gpui_kit::*;
        // measured from the outside of the grips' frame, `GRAB` past the border
        const IN: f32 = 4.;
        const EDGE: f32 = layout::GRAB + IN;
        const CORNER: f32 = layout::GRAB + 10.;
        let grips: [(&str, [bool; 4], CursorStyle); 8] = [
            (
                "left",
                [true, false, false, false],
                CursorStyle::ResizeLeftRight,
            ),
            (
                "right",
                [false, false, true, false],
                CursorStyle::ResizeLeftRight,
            ),
            (
                "top",
                [false, true, false, false],
                CursorStyle::ResizeUpDown,
            ),
            (
                "bottom",
                [false, false, false, true],
                CursorStyle::ResizeUpDown,
            ),
            (
                "top-left",
                [true, true, false, false],
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                "bottom-right",
                [false, false, true, true],
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                "top-right",
                [false, true, true, false],
                CursorStyle::ResizeUpRightDownLeft,
            ),
            (
                "bottom-left",
                [true, false, false, true],
                CursorStyle::ResizeUpRightDownLeft,
            ),
        ];
        grips
            .into_iter()
            .map(|(name, sides, cursor)| {
                let [left, top, right, bottom] = sides;
                let corner = (left || right) && (top || bottom);
                let grip = div()
                    .id(SharedString::from(format!("pane/{index}/grip/{name}")))
                    .absolute()
                    .cursor(cursor)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.hold(index, sides, event.position, cx);
                        }),
                    );
                let grip = match corner {
                    true => grip.size(px(CORNER)),
                    false if left || right => grip.w(px(EDGE)).top(px(CORNER)).bottom(px(CORNER)),
                    false => grip.h(px(EDGE)).left(px(CORNER)).right(px(CORNER)),
                };
                let grip = if left {
                    grip.left_0()
                } else if right {
                    grip.right_0()
                } else {
                    grip
                };
                if top {
                    grip.top_0()
                } else if bottom {
                    grip.bottom_0()
                } else {
                    grip
                }
            })
            .collect()
    }
}

/// A button on a window's title bar.
#[derive(Clone, Copy)]
enum PaneAction {
    Split,
    PopOut,
    PopIn,
    Close,
}

impl PaneAction {
    /// Its element id, its accessible name, its glyph.
    fn parts(self) -> (&'static str, &'static str, gpui_kit::assets::IconName) {
        use gpui_kit::assets::IconName;
        match self {
            Self::Split => ("split", "Open another window", IconName::Plus),
            Self::PopOut => (
                "popout",
                "Open in new window",
                IconName::SquareArrowOutUpRight,
            ),
            Self::PopIn => ("popin", "Move to main window", IconName::ArrowDownLeft),
            Self::Close => ("close", "Close pane", IconName::X),
        }
    }
}

/// A title bar's height.
const TITLE: f32 = 32.;

/// A window held by the pointer.
#[derive(Clone, Copy, Debug)]
pub(super) struct Drag {
    index: usize,
    sides: [bool; 4],
    from: (f32, f32),
    start: layout::Frame,
}

impl Drag {
    /// The frame with the pointer at `to`. A side held past the smallest
    /// window stops; the side across from it stays put.
    fn frame(&self, to: (f32, f32)) -> layout::Frame {
        let (dx, dy) = (to.0 - self.from.0, to.1 - self.from.1);
        let start = self.start;
        let [left, top, right, bottom] = self.sides;
        let mut frame = start;
        if self.sides == [false; 4] {
            frame.x += dx;
            frame.y += dy;
        }
        if right {
            frame.w = (start.w + dx).max(layout::MIN_WIDTH);
        }
        if bottom {
            frame.h = (start.h + dy).max(layout::MIN_HEIGHT);
        }
        if left {
            frame.w = (start.w - dx).max(layout::MIN_WIDTH);
            frame.x = start.x + start.w - frame.w;
        }
        if top {
            frame.h = (start.h - dy).max(layout::MIN_HEIGHT);
            frame.y = start.y + start.h - frame.h;
        }
        frame
    }
}

/// A press anywhere in this OS window raises the pane under the pointer,
/// unless something is open over the desk. Registered on the whole OS
/// window in the capture phase, before anything under the pointer sees it:
/// a program's view may swallow the pointer, and must not keep its pane
/// from rising.
fn raise(
    this: gpui_kit::Entity<DesktopWindow>,
    desk: gpui_kit::Bounds<gpui_kit::Pixels>,
    window: &mut Window,
) {
    use gpui_kit::*;
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture {
            return;
        }
        let at = (
            f32::from(event.position.x - desk.origin.x),
            f32::from(event.position.y - desk.origin.y),
        );
        this.update(cx, |this, cx| {
            // a press on something open over the desk is not on a window
            let covered = this.kind == crate::shell::WindowKind::Console
                && this.model.read(cx).state.overlay.is_some();
            let layout = this.layout(cx);
            let Some(index) = layout.under(at).filter(|_| !covered) else {
                return;
            };
            let on_top = layout.stacking().last() == Some(&index);
            if index != layout.focused || !on_top {
                this.pane_message(PaneMessage::Focus(index), window, cx);
            }
        });
    });
}

/// The pointer, window-wide, while a window is held: a move carries it, a
/// release lets it go.
fn follow(this: gpui_kit::Entity<DesktopWindow>, window: &mut Window) {
    use gpui_kit::*;
    let held = this.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        held.update(cx, |this, cx| {
            let Some(drag) = this.drag else {
                return;
            };
            if event.pressed_button != Some(MouseButton::Left) {
                this.drag = None;
                cx.notify();
            } else {
                let to = (event.position.x.into(), event.position.y.into());
                let (key, frame) = (this.key, drag.frame(to));
                this.model.update(cx, |model, cx| {
                    model.dispatch(
                        Message::Pane(key, PaneMessage::Frame(drag.index, frame)),
                        cx,
                    )
                });
            }
        });
    });
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            this.update(cx, |this, cx| {
                if this.drag.take().is_some() {
                    cx.notify();
                }
            });
        }
    });
}

#[cfg(test)]
mod drag_tests {
    use super::{Drag, layout::*};

    #[test]
    fn a_title_bar_moves_and_an_edge_sizes_from_its_own_side() {
        let start = Frame {
            x: 100.,
            y: 100.,
            w: 500.,
            h: 400.,
        };
        let drag = |sides| Drag {
            index: 0,
            sides,
            from: (0., 0.),
            start,
        };
        let moved = drag([false; 4]).frame((30., -20.));
        assert_eq!(
            moved,
            Frame {
                x: 130.,
                y: 80.,
                ..start
            }
        );
        let corner = drag([false, false, true, true]).frame((40., 50.));
        assert_eq!(
            corner,
            Frame {
                w: 540.,
                h: 450.,
                ..start
            }
        );
        // the left edge past the smallest window: the right edge stays put
        let left = drag([true, false, false, false]).frame((1000., 0.));
        assert_eq!((left.w, left.x + left.w), (MIN_WIDTH, 600.));
        let top = drag([false, true, false, false]).frame((0., -60.));
        assert_eq!((top.y, top.h), (40., 460.));
    }
}

#[cfg(test)]
mod empty_panes_message_tests {
    use super::empty_panes_message;
    use crate::runtime::RailRow;

    fn row(empty: bool) -> RailRow {
        RailRow {
            module: "chat",
            label: "Chat".into(),
            note: None,
            empty,
        }
    }

    #[test]
    fn names_the_network_only_when_the_rail_itself_is_empty() {
        assert_eq!(
            empty_panes_message(&[]),
            Some("This network runs no program with a view.")
        );
        assert_eq!(
            empty_panes_message(&[row(true)]),
            Some("This network runs no program with a view."),
            "a rail of empty-slot rows still has nothing to open"
        );
        assert_eq!(empty_panes_message(&[row(true), row(false)]), None);
    }
}
