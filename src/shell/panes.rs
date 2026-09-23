//! View entities belong to panes, including when a pane changes OS windows.
use super::*;

pub(super) struct MountedPane {
    pub(super) module: &'static str,
    pub(super) view: Entity<crate::runtime::NativeModuleView>,
    pub(super) route: Option<gpui_kit::Subscription>,
    context: std::rc::Rc<std::cell::RefCell<String>>,
}

pub(super) fn label(module: &str) -> String {
    crate::runtime::rail()
        .into_iter()
        .find(|row| row.module == module)
        .map(|row| row.label)
        .unwrap_or_else(|| module.to_owned())
}

/// What an empty pane strip says: the rail lists what the network runs, so
/// an empty pane list beside a non-empty rail means every pane here was
/// closed or popped out, not that the network has nothing to show — that
/// reused the "no program" sentence and read as if Chat/Forge/Settings had
/// vanished from the rail right beside it.
fn empty_panes_message(rail: &[crate::runtime::RailRow]) -> &'static str {
    if rail.iter().any(|row| !row.empty) {
        match cfg!(target_os = "macos") {
            true => "Press ⌘K to open something.",
            false => "Press Ctrl K to open something.",
        }
    } else {
        "This network runs no program with a view."
    }
}

impl DesktopWindow {
    pub(super) fn focused_view(&self) -> Option<Entity<crate::runtime::NativeModuleView>> {
        self.layout
            .panes
            .get(self.layout.focused)
            .and_then(|pane| self.mounted.get(&pane.instance))
            .map(|pane| pane.view.clone())
    }

    pub(super) fn initialize_panes(
        &mut self,
        module: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if !self.initialized {
            let module = match self.kind {
                crate::shell::WindowKind::View { module } => Some(module),
                _ => module,
            };
            if let Some(module) = module {
                self.layout.select(module);
                self.initialized = true;
            }
        }
        self.sync_panes(cx);
    }

    fn sync_panes(&mut self, cx: &mut Context<Self>) {
        let obsolete: Vec<_> = self
            .mounted
            .keys()
            .copied()
            .filter(|id| !self.layout.panes.iter().any(|pane| pane.instance == *id))
            .collect();
        for id in obsolete {
            if let Some(pane) = self.mounted.remove(&id) {
                self.hide_pane(pane, cx);
            }
        }
        for pane in &self.layout.panes {
            self.mounted.entry(pane.instance).or_insert_with(|| {
                let module = pane.module;
                let view = cx.new(|_| crate::runtime::NativeModuleView::new(module));
                MountedPane {
                    module,
                    view,
                    route: None,
                    context: Default::default(),
                }
            });
            let mounted = self.mounted.get_mut(&pane.instance).expect("mounted pane");
            if mounted.route.is_none() {
                let model = self.model.clone();
                let context = mounted.context.clone();
                let module = pane.module;
                mounted.route = Some(cx.subscribe(&mounted.view, move |_, _, event, cx| {
                    if event.kind == "context" {
                        *context.borrow_mut() = crate::runtime::event_text(event, "text");
                        cx.notify();
                    }
                    model.update(cx, |model, cx| {
                        model.dispatch(Message::ViewEvent(module, event.clone()), cx)
                    });
                }));
            }
        }
    }

    pub(super) fn hide_pane(&self, pane: MountedPane, cx: &mut gpui_kit::App) {
        let intents = pane.view.update(cx, |view, _| view.hide());
        for intent in intents {
            self.model.update(cx, |model, cx| {
                model.dispatch(Message::ViewEvent(pane.module, intent), cx)
            });
        }
    }

    pub(super) fn pane_message(
        &mut self,
        message: Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.initialized = true;
        match message {
            Message::SelectView(module) => {
                self.layout.select(module);
                self.model.update(cx, |model, cx| {
                    model.dispatch(Message::SelectView(module), cx)
                });
            }
            Message::SplitView(module) => {
                self.layout.split(module);
            }
            Message::ClosePane(index) => {
                self.layout.close(index);
                if matches!(self.kind, crate::shell::WindowKind::View { .. }) {
                    window.remove_window();
                }
            }
            Message::FocusPane(index) => {
                self.layout.focus(index);
            }
            Message::PopOut(index) => {
                if let Some(pane) = self.layout.popout(index)
                    && let Some(mut mounted) = self.mounted.remove(&pane.instance)
                {
                    mounted.route = None;
                    let kind = crate::shell::WindowKind::View {
                        module: pane.module,
                    };
                    let source = cx.weak_entity();
                    let at = super::windows::cascade(
                        window.bounds(),
                        window.display(cx).map(|display| display.bounds()),
                    );
                    self.model.update(cx, |model, cx| {
                        let (reply, _) = oneshot::channel();
                        model.open_window(
                            WindowKey::unique(),
                            kind,
                            reply,
                            Some((pane, mounted, source)),
                            Some(at),
                            cx,
                        );
                    });
                }
            }
            Message::PopIn(key) if key == self.key => {
                let destination = self
                    .model
                    .read(cx)
                    .views
                    .values()
                    .filter_map(|view| view.upgrade())
                    .find(|view| {
                        view.entity_id() != cx.entity_id()
                            && view.read(cx).kind == crate::shell::WindowKind::Console
                    });
                if let Some(destination) = destination
                    && let Some(pane) = self.layout.popout(0)
                    && let Some(mut mounted) = self.mounted.remove(&pane.instance)
                {
                    mounted.route = None;
                    destination.update(cx, |this, cx| {
                        this.mounted.insert(pane.instance, mounted);
                        this.layout.popin(pane);
                        this.initialized = true;
                        this.sync_panes(cx);
                        cx.notify();
                    });
                    window.remove_window();
                }
            }
            _ => return,
        }
        self.sync_panes(cx);
        for (index, pane) in self.layout.panes.iter().enumerate() {
            self.mounted[&pane.instance].view.update(cx, |view, cx| {
                view.set_focused(index == self.layout.focused, cx)
            });
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn pane_button(
        &self,
        index: usize,
        action: &'static str,
        glyph: gpui_kit::assets::IconName,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::*;
        let ink = super::ink::Ink::of(self.model.read(cx).state.dark());
        let hover = ink.surface;
        // The action drives the element id and the dispatch below (stable
        // for the AX door and tests); the AX name is a phrase a screen
        // reader can announce on its own, not the bare verb.
        let name = match action {
            "split" => "Open another window",
            "popout" => "Open in new window",
            "popin" => "Move to main window",
            _ => "Close pane",
        };
        crate::a11y::disabled(
            crate::a11y::keyboard(
                div()
                    .id(SharedString::from(format!("pane/{index}/{action}")))
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
                            "split" => Message::SplitView(this.layout.panes[index].module),
                            "close" => Message::ClosePane(index),
                            "popout" => Message::PopOut(index),
                            _ => Message::PopIn(this.key),
                        };
                        this.pane_message(message, window, cx);
                    }))
                    .child(gpui_kit::component::Icon::new(glyph).size(px(18.))),
            ),
            !enabled,
        )
    }

    /// The desk windows sit on: the window below its bar.
    fn desk(&self, window: &Window) -> (f32, f32) {
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
        if self.layout.panes.is_empty() {
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
                    super::figure::Figure::Node,
                    moving,
                    ink.figure,
                    window,
                    cx,
                ))
                .child(
                    super::ink::mono(400, 12.)
                        .text_color(ink.muted)
                        .child(empty_panes_message(&crate::runtime::rail())),
                )
                .into_any_element();
        }
        let props = self.model.read(cx).state.view_props();
        let console = self.kind == crate::shell::WindowKind::Console;
        let desk = self.desk(window);
        self.layout.place(desk);
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
        let multi = self.layout.panes.len() > 1;
        for index in self.layout.stacking() {
            let pane = &self.layout.panes[index];
            let focused = index == self.layout.focused;
            let view = self.mounted[&pane.instance].view.clone();
            view.update(cx, |view, cx| {
                view.set_focused(focused, cx);
                view.set_props(props.clone(), cx);
            });
            use gpui_kit::assets::IconName;
            let controls = div()
                .flex()
                .items_center()
                .gap(px(2.))
                .when(console, |strip| {
                    strip
                        .child(self.pane_button(
                            index,
                            "split",
                            IconName::Plus,
                            self.layout.panes.len() < layout::MAX_PANES,
                            cx,
                        ))
                        .child(self.pane_button(
                            index,
                            "popout",
                            IconName::SquareArrowOutUpRight,
                            true,
                            cx,
                        ))
                })
                .when(!console, |strip| {
                    strip.child(self.pane_button(index, "popin", IconName::ArrowDownLeft, true, cx))
                })
                .child(self.pane_button(index, "close", IconName::X, true, cx));
            let context = self.mounted[&pane.instance].context.borrow().clone();
            let body = div()
                .id(SharedString::from(format!("pane/{index}")))
                .flex()
                .flex_col()
                .bg(ink.bg)
                .overflow_hidden()
                .role(gpui_kit::Role::Group)
                .when(!context.is_empty(), |pane| pane.aria_label(context.clone()));
            let seated = match (console, pane.frame) {
                // a window of its own: the view is the window, its controls
                // float over the top-right corner a view leaves free
                // (`design::pane`)
                (false, _) | (true, None) => body
                    .relative()
                    .size_full()
                    .child(div().flex_1().min_h_0().w_full().child(view))
                    .child(
                        div()
                            .id(SharedString::from(format!("pane/{index}/strip")))
                            .absolute()
                            .top(px(8.))
                            .right(px(8.))
                            .bg(ink.bg)
                            .child(controls),
                    ),
                // on the desk: a title bar to hold it by, edges to size it by
                (true, Some(frame)) => {
                    let title = div()
                        .id(SharedString::from(format!("pane/{index}/strip")))
                        .h(px(TITLE))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .pl(px(12.))
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
                                if event.click_count == 2 {
                                    let desk = this.desk(window);
                                    this.layout.toggle_fill(index, desk);
                                    cx.notify();
                                } else {
                                    this.hold(index, [false; 4], event.position);
                                }
                            }),
                        )
                        .child(
                            super::ink::mono(500, 12.)
                                .flex_shrink_0()
                                .text_color(ink.ink)
                                .child(label(pane.module)),
                        )
                        .child(
                            super::ink::sans(400, 13.)
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ink.muted)
                                .child(context),
                        )
                        .child(controls);
                    // what's behind a window doesn't hear presses on it
                    body.occlude()
                        .absolute()
                        .left(px(frame.x))
                        .top(px(frame.y))
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
                        .child(div().flex_1().min_h_0().w_full().child(view))
                        .children(self.grips(index, cx))
                }
            };
            stage = stage.child(seated);
        }
        stage.into_any_element()
    }

    /// Takes hold of window `index` at `at`: `sides` (left, top, right,
    /// bottom) follow the pointer; none, and the whole window does.
    fn hold(&mut self, index: usize, sides: [bool; 4], at: gpui_kit::Point<gpui_kit::Pixels>) {
        if let Some(start) = self.layout.panes.get(index).and_then(|pane| pane.frame) {
            self.drag = Some(Drag {
                index,
                sides,
                from: (at.x.into(), at.y.into()),
                start,
            });
        }
    }

    /// The edges and corners a window is sized by, laid over its border.
    fn grips(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<gpui_kit::Stateful<gpui_kit::Div>> {
        use gpui_kit::*;
        const EDGE: f32 = 5.;
        const CORNER: f32 = 12.;
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
                            this.hold(index, sides, event.position);
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

/// A press anywhere on a window brings it to the front. Window-wide and
/// before anything under the pointer sees it: a program's view may block
/// the pointer from the window it sits in.
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
            let Some(index) = this.layout.under(at) else {
                return;
            };
            let on_top = this.layout.stacking().last() == Some(&index);
            if index != this.layout.focused || !on_top {
                this.pane_message(Message::FocusPane(index), window, cx);
            }
        });
    });
}

/// The pointer, window-wide, while a window is held: a move carries it, a
/// release lets it go.
fn follow(this: gpui_kit::Entity<DesktopWindow>, window: &mut Window) {
    use gpui_kit::*;
    let held = this.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        held.update(cx, |this, cx| {
            let Some(drag) = this.drag else {
                return;
            };
            if event.pressed_button != Some(MouseButton::Left) {
                this.drag = None;
            } else {
                let desk = this.desk(window);
                let to = (event.position.x.into(), event.position.y.into());
                this.layout.set_frame(drag.index, drag.frame(to), desk);
            }
            cx.notify();
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
            "This network runs no program with a view."
        );
        assert_eq!(
            empty_panes_message(&[row(true)]),
            "This network runs no program with a view.",
            "a rail of empty-slot rows still has nothing to open"
        );
        assert!(empty_panes_message(&[row(true), row(false)]).starts_with("Press "));
    }
}
