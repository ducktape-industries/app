//! A pane held by the pointer: the title bar moves it, the grips around it
//! size it, and a press anywhere raises it. The pointer is followed
//! window-wide, so a fast hand cannot slip off what it holds (Settings'
//! title strip is followed the same way, `follow_pointer`). The hold is
//! the `PaneLayer`'s; the grips are drawn by the `PaneView` they size.
use super::layers::{PaneLayer, PaneView};
use super::*;
use gpui_kit::{Context, Window};

impl PaneLayer {
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
    pub(super) fn grips(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<gpui_kit::Stateful<gpui_kit::Div>> {
        use gpui_kit::*;
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Sides {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
}

impl Sides {
    pub(super) const NONE: Sides = Sides {
        left: false,
        top: false,
        right: false,
        bottom: false,
    };
    pub(super) const RIGHT: Sides = Sides {
        right: true,
        ..Sides::NONE
    };
    pub(super) const BOTTOM: Sides = Sides {
        bottom: true,
        ..Sides::NONE
    };
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Drag {
    index: usize,
    sides: Sides,
    from: (f32, f32),
    start: layout::Frame,
    min_w: f32,
}

impl Drag {
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
            frame.w = (start.w - dx).min(start.x + start.w).max(self.min_w);
            frame.x = start.x + start.w - frame.w;
        }
        if top {
            frame.h = (start.h - dy)
                .min(start.y + start.h)
                .max(layout::MIN_HEIGHT);
            frame.y = start.y + start.h - frame.h;
        }
        frame
    }
}

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
            let covered = this.overlays.read(cx).get().is_some();
            (covered, this.layout(cx), this.window.clone())
        };
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

pub(super) fn follow(this: gpui_kit::Entity<PaneLayer>, window: &mut Window) {
    follow_pointer(this, window, |this, to, _, cx| {
        let Some(drag) = this.drag else {
            return;
        };
        match to {
            Some(to) => {
                let frame = drag.frame(to);
                this.desk
                    .update(cx, |desk, cx| desk.set_frame(drag.index, frame, cx));
            }
            None => {
                this.drag = None;
                cx.notify();
            }
        }
    });
}

pub(super) fn follow_pointer<T: 'static>(
    this: gpui_kit::Entity<T>,
    window: &mut Window,
    held: impl Fn(&mut T, Option<(f32, f32)>, &mut Window, &mut Context<T>) + 'static,
) {
    use gpui_kit::*;
    let held = std::rc::Rc::new(held);
    let (moving, moved) = (this.clone(), held.clone());
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        let to = (event.pressed_button == Some(MouseButton::Left))
            .then(|| (event.position.x.into(), event.position.y.into()));
        moving.update(cx, |this, cx| moved(this, to, window, cx));
    });
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            this.update(cx, |this, cx| held(this, None, window, cx));
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
        let past = drag(Sides {
            left: true,
            top: true,
            ..Sides::NONE
        })
        .frame((-1000., -1000.));
        assert_eq!(
            (past.x, past.y, past.x + past.w, past.y + past.h),
            (0., 0., 600., 500.)
        );
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
