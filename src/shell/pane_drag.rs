//! A pane held by the pointer: the title bar moves it, the grips around it
//! size it, and a press anywhere raises it. The pointer is followed
//! window-wide, so a fast hand cannot slip off what it holds.
use super::*;

impl DesktopWindow {
    /// Takes hold of window `index` at `at`: `sides` follow the pointer;
    /// none, and the whole window does.
    pub(super) fn hold(
        &mut self,
        index: usize,
        sides: Sides,
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
    pub(super) fn grips(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<gpui_kit::Stateful<gpui_kit::Div>> {
        use gpui_kit::*;
        // measured from the outside of the grips' frame, `GRAB` past the border
        const IN: f32 = 4.;
        const EDGE: f32 = layout::GRAB + IN;
        const CORNER: f32 = layout::GRAB + 10.;
        const NONE: Sides = Sides::NONE;
        let grips: [(&str, Sides, CursorStyle); 8] = [
            (
                "left",
                Sides { left: true, ..NONE },
                CursorStyle::ResizeLeftRight,
            ),
            (
                "right",
                Sides {
                    right: true,
                    ..NONE
                },
                CursorStyle::ResizeLeftRight,
            ),
            (
                "top",
                Sides { top: true, ..NONE },
                CursorStyle::ResizeUpDown,
            ),
            (
                "bottom",
                Sides {
                    bottom: true,
                    ..NONE
                },
                CursorStyle::ResizeUpDown,
            ),
            (
                "top-left",
                Sides {
                    left: true,
                    top: true,
                    ..NONE
                },
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                "bottom-right",
                Sides {
                    right: true,
                    bottom: true,
                    ..NONE
                },
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                "top-right",
                Sides {
                    top: true,
                    right: true,
                    ..NONE
                },
                CursorStyle::ResizeUpRightDownLeft,
            ),
            (
                "bottom-left",
                Sides {
                    left: true,
                    bottom: true,
                    ..NONE
                },
                CursorStyle::ResizeUpRightDownLeft,
            ),
        ];
        grips
            .into_iter()
            .map(|(name, sides, cursor)| {
                let Sides {
                    left,
                    top,
                    right,
                    bottom,
                } = sides;
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

/// The sides of a window a hold carries along with the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Sides {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
}

impl Sides {
    /// No side held: the whole window follows.
    pub(super) const NONE: Sides = Sides {
        left: false,
        top: false,
        right: false,
        bottom: false,
    };
}

/// A window held by the pointer.
#[derive(Clone, Copy, Debug)]
pub(super) struct Drag {
    index: usize,
    sides: Sides,
    from: (f32, f32),
    start: layout::Frame,
}

impl Drag {
    /// The frame with the pointer at `to`. A side held past the smallest
    /// window stops; the side across from it stays put.
    fn frame(&self, to: (f32, f32)) -> layout::Frame {
        let (dx, dy) = (to.0 - self.from.0, to.1 - self.from.1);
        let start = self.start;
        let Sides {
            left,
            top,
            right,
            bottom,
        } = self.sides;
        let mut frame = start;
        if self.sides == Sides::NONE {
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
pub(super) fn raise(
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
pub(super) fn follow(this: gpui_kit::Entity<DesktopWindow>, window: &mut Window) {
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
    use super::{Drag, Sides, layout::*};

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
        let moved = drag(Sides::NONE).frame((30., -20.));
        assert_eq!(
            moved,
            Frame {
                x: 130.,
                y: 80.,
                ..start
            }
        );
        let corner = drag(Sides {
            right: true,
            bottom: true,
            ..Sides::NONE
        })
        .frame((40., 50.));
        assert_eq!(
            corner,
            Frame {
                w: 540.,
                h: 450.,
                ..start
            }
        );
        // the left edge past the smallest window: the right edge stays put
        let left = drag(Sides {
            left: true,
            ..Sides::NONE
        })
        .frame((1000., 0.));
        assert_eq!((left.w, left.x + left.w), (MIN_WIDTH, 600.));
        let top = drag(Sides {
            top: true,
            ..Sides::NONE
        })
        .frame((0., -60.));
        assert_eq!((top.y, top.h), (40., 460.));
    }
}
