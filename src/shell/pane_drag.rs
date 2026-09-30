//! A pane held by the pointer: the title bar moves it, the grips around it
//! size it, and a press anywhere raises it. The pointer is followed
//! window-wide, so a fast hand cannot slip off what it holds. The hold is
//! the `PaneLayer`'s; the grips are drawn by the `PaneView` they size.
use super::layers::{PaneLayer, PaneView};
use super::*;

impl PaneLayer {
    /// Takes hold of window `index` at `at`: `sides` follow the pointer;
    /// none, and the whole window does.
    pub(super) fn hold(
        &mut self,
        index: usize,
        sides: Sides,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &gpui_kit::App,
    ) {
        if let Some(drag) = Drag::of(&self.layout(cx), index, sides, (at.x.into(), at.y.into())) {
            self.drag = Some(drag);
        }
    }
}

impl PaneView {
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
    /// The right edge: the keyboard sizes a window's width by it.
    pub(super) const RIGHT: Sides = Sides {
        right: true,
        ..Sides::NONE
    };
    /// The bottom edge: the keyboard sizes a window's height by it.
    pub(super) const BOTTOM: Sides = Sides {
        bottom: true,
        ..Sides::NONE
    };
}

/// A window held by the pointer.
#[derive(Clone, Copy, Debug)]
pub(super) struct Drag {
    index: usize,
    sides: Sides,
    from: (f32, f32),
    start: layout::Frame,
    /// The window's [`layout::Pane::min_width`].
    min_w: f32,
}

impl Drag {
    /// Window `index` of `layout` held at `from`, once it has a frame. Its
    /// edges stop at the pane's floor, or at the desk's width where the
    /// desk is narrower: the frame is capped there, and an edge held past
    /// it would carry the whole window.
    pub(super) fn of(
        layout: &layout::Layout,
        index: usize,
        sides: Sides,
        from: (f32, f32),
    ) -> Option<Self> {
        let pane = layout.panes.get(index)?;
        let desk = layout.desk().0.max(layout::MIN_WIDTH);
        Some(Self {
            index,
            sides,
            from,
            start: pane.frame?,
            min_w: pane.min_width().min(desk),
        })
    }

    /// The frame with the pointer at `to`. A side held past the window's
    /// narrowest (or the smallest window's height) stops; the side across
    /// from it stays put.
    pub(super) fn frame(&self, to: (f32, f32)) -> layout::Frame {
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
            frame.w = (start.w + dx).max(self.min_w);
        }
        if bottom {
            frame.h = (start.h + dy).max(layout::MIN_HEIGHT);
        }
        if left {
            frame.w = (start.w - dx).max(self.min_w);
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
/// from rising. The messages go through the window (`pane_message` marks
/// the layer's panes moved), so the layer is read, never held, here.
pub(super) fn raise(
    this: gpui_kit::Entity<PaneLayer>,
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
        let (covered, layout, desk_window) = {
            let this = this.read(cx);
            // a press on something open over the desk is not on a window
            let covered = this.overlays.read(cx).get().is_some();
            (covered, this.layout(cx), this.window.clone())
        };
        // a press ends the hold the keyboard has on a window, as it is
        if layout.held.is_some() {
            let _ = desk_window.update(cx, |desk, cx| {
                desk.hold_message(PaneMessage::Release { keep: true }, cx)
            });
        }
        let Some(index) = layout.under(at).filter(|_| !covered) else {
            return;
        };
        let on_top = layout.stacking().last() == Some(&index);
        if index != layout.focused || !on_top {
            let _ = desk_window.update(cx, |desk, cx| {
                desk.pane_message(PaneMessage::Focus(index), window, cx)
            });
        }
    });
}

/// The pointer, window-wide, so a fast one can't slip off the window it
/// holds: while a window is held, a move carries it and a release lets it
/// go, a release that comes before the next frame too. Every move
/// dispatches `Pane(Frame)`: `Desk` is bridged until s11, so a frame set
/// on it directly would be snapped back by the next dispatch.
pub(super) fn follow(this: gpui_kit::Entity<PaneLayer>, window: &mut Window) {
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
            min_w: MIN_WIDTH,
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
        // a window holding a view laid out from 680 stops at 682
        let wide = Frame { w: 900., ..start };
        let left = Drag {
            start: wide,
            min_w: 682.,
            ..drag(Sides {
                left: true,
                ..Sides::NONE
            })
        }
        .frame((1000., 0.));
        assert_eq!((left.w, left.x + left.w), (682., 1000.));
    }

    /// On a desk narrower than the view the window is the desk: its left
    /// edge held inwards stays put rather than carrying the window.
    #[test]
    fn an_edge_on_a_desk_narrower_than_the_view_stays_put() {
        crate::runtime::seat_for_test("drag-cramped-view", 680);
        let desk = (600., 400.);
        let mut layout = Layout::default();
        layout.split("drag-cramped-view");
        layout.measure(desk);
        layout.settle();
        let start = layout.panes[0].frame.unwrap();
        assert_eq!((start.x, start.w), (0., 600.));
        let left = Sides {
            left: true,
            ..Sides::NONE
        };
        let drag = Drag::of(&layout, 0, left, (0., 0.)).unwrap();
        let held = drag.frame((50., 0.));
        assert!(layout.set_frame(0, held, desk));
        assert_eq!(layout.panes[0].frame, Some(start));
    }
}
