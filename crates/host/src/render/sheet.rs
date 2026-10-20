//! The Sheet node: a panel over a scrim, the view's one child inside it.
//! Under a Compact body (`design::width::COMPACT_BELOW`, the class the
//! view itself reads off `Event::Viewport`) it docks to the bottom edge at
//! its first detent, is dragged by its handle between its detents, and a
//! release nearer the bottom than the first detent dismisses it; with no
//! detents it is as tall as its child, at most the body, and its one rest
//! is there. At a wider body it is a centred modal. Either way it covers the view's whole body,
//! not the box it sits in: the walk builds it where the node is (its ids,
//! fields and focus keep their paths) and `ViewTree::render` lays it over
//! the root. The view owns whether it is open: a dismissal (the scrim, a
//! drag down, Escape) is the view's `on_dismiss`, and the panel stays at
//! its first detent until the view's next frame drops it, so a sheet the
//! view keeps (or opens again) is never left half-closed.
//!
//! Under touch a docked sheet moves as the Expo build's (gorhom's
//! `BottomSheetModal`, `motion::Idiom::sheet`): it slides up by its own height
//! as it first draws, a release settles at its detent, and one the view
//! drops goes down as it was last drawn ([`Leaving`]), the scrim fading
//! with it. A pointer window's sheet does not move.
use super::*;
use crate::motion::{self, Tween};
use gpui_kit::prelude::FluentBuilder as _;

#[derive(Default)]
pub(super) struct SheetState {
    rest: usize,
    drag: Option<Drag>,
    fit: Option<Pixels>,
    came: Option<Tween>,
    settle: Option<(Pixels, Tween)>,
    drawn: Option<Pixels>,
}

pub(super) struct Leaving {
    node: wire::Node,
    styles: wire::Styles,
    pub(super) path: AuthoredPath,
    height: Pixels,
    tween: Tween,
}

impl SheetState {
    pub(super) fn moves(&self) -> bool {
        self.drawn.is_some()
    }

    pub(super) fn leave(
        &self,
        node: &wire::Node,
        styles: &wire::Styles,
        path: &AuthoredPath,
        cx: &App,
    ) -> Option<Leaving> {
        let height = self.drawn?;
        let now = motion::now(cx);
        let from = self.came.map_or(1., |came| came.at(now));
        Some(Leaving {
            node: node.clone(),
            styles: styles.clone(),
            path: path.clone(),
            height,
            tween: Tween::between(from, 0., motion::idiom().sheet, cx),
        })
    }
}

impl SheetState {
    #[cfg(test)]
    pub(super) fn rest(&self) -> usize {
        self.rest
    }
}

struct Drag {
    from: Pixels,
    start: Pixels,
    now: Pixels,
}

const WIDEST: f32 = design::width::COMPACT_BELOW as f32;
pub(super) const HANDLE_BAND: f32 = 20.;
const HANDLE: (f32, f32) = (32., 4.);
const SCRIM: (f32, f32) = (0.4, 0.55);

impl ViewTree {
    pub(super) fn sheet(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Sheet {
            id,
            label,
            style,
            detents,
            on_dismiss,
            child,
        } = node
        else {
            unreachable!()
        };
        let style = self.style(*style);
        let path = self.authored_path.clone();
        let body = self.body.unwrap_or_else(|| window.viewport_size());
        let compact = f64::from(f32::from(body.width)) < design::width::COMPACT_BELOW;
        let palette = crate::theme::palette(cx);
        let moves = compact && crate::contract::os().facts().touch_first;
        let now = motion::now(cx);
        let from = self
            .leaving
            .as_ref()
            .filter(|leaving| leaving.path == path)
            .map_or(0., |leaving| leaving.tween.at(now));
        let leaving = self.leaving_at;
        let mut kept = SheetState::default();
        let state = match leaving {
            Some(_) => &mut kept,
            None => self.sheets.entry(path.clone()).or_default(),
        };
        if moves && leaving.is_none() && state.drawn.is_none() && state.came.is_none() {
            state.came = Some(Tween::between(from, 1., motion::idiom().sheet, cx));
        }
        let rest = match detents.len() {
            0 => None,
            len => Some(body.height * detents[state.rest.min(len - 1)]),
        };
        if state.settle.is_some_and(|(_, tween)| tween.done(now)) {
            state.settle = None;
        }
        let height = match (&state.drag, state.settle, leaving) {
            (_, _, Some((_, height, _))) => Some(height),
            (Some(drag), _, None) => Some(drag.now),
            (None, Some((from, tween)), None) => {
                let to = rest.or(state.fit).unwrap_or(from);
                Some(from + (to - from) * tween.frame(window, cx))
            }
            (None, None, None) => rest,
        };
        if state.came.is_some_and(|came| came.done(now)) {
            state.came = None;
        }
        let shown = match (leaving, state.came) {
            (Some((progress, _, slides)), _) => Some((progress, slides)),
            (None, Some(came)) => Some((came.frame(window, cx), came.motion.slides)),
            (None, None) => None,
        };
        let lifted = height.or(state.fit).unwrap_or(body.height);
        if moves && leaving.is_none() {
            state.drawn = Some(lifted);
        }
        let fitted = detents.is_empty();
        let content = self.node(child, window, cx);
        let dismiss = leaving.is_none().then_some(*on_dismiss).flatten();
        let mut panel = div()
            .id(host_id("sheet-panel"))
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(crate::theme::hsla(palette.background))
            .text_color(crate::theme::hsla(palette.foreground))
            .refine_style(&style)
            .occlude();
        if compact {
            let view = cx.entity().downgrade();
            let (pressed, moved, released) = (path.clone(), path.clone(), path.clone());
            let heights: Vec<Pixels> = detents.iter().map(|d| body.height * *d).collect();
            let highest = heights.last().copied().unwrap_or(body.height);
            let grab = div()
                .id(host_id("sheet-handle"))
                .flex_none()
                .h(px(HANDLE_BAND))
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor(CursorStyle::ResizeUpDown)
                .child(
                    div()
                        .w(px(HANDLE.0))
                        .h(px(HANDLE.1))
                        .rounded(px(HANDLE.1 / 2.))
                        .bg(crate::theme::hsla(palette.border_strong)),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        if let Some(state) = this.sheets.get_mut(&pressed) {
                            state.settle = None;
                            let start = height.or(state.fit).unwrap_or(highest);
                            state.drag = Some(Drag {
                                from: event.position.y,
                                start,
                                now: start,
                            });
                        }
                    }),
                );
            let capture = canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let moving = view.clone();
                    let moved = moved.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase != gpui_kit::DispatchPhase::Capture {
                            return;
                        }
                        let _ = moving.update(cx, |this, cx| {
                            let Some(state) = this.sheets.get_mut(&moved) else {
                                return;
                            };
                            let highest = match fitted {
                                true => state.fit.unwrap_or(highest),
                                false => highest,
                            };
                            let Some(drag) = state.drag.as_mut() else {
                                return;
                            };
                            let now = (drag.start - (event.position.y - drag.from))
                                .clamp(px(0.), highest);
                            if now != drag.now {
                                drag.now = now;
                                cx.notify();
                            }
                        });
                    });
                    let releasing = view.clone();
                    let released = released.clone();
                    let heights = heights.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase != gpui_kit::DispatchPhase::Capture
                            || event.button != MouseButton::Left
                        {
                            return;
                        }
                        let _ = releasing.update(cx, |this, cx| {
                            let Some(state) = this.sheets.get_mut(&released) else {
                                return;
                            };
                            let Some(drag) = state.drag.take() else {
                                return;
                            };
                            let heights = match fitted {
                                true => vec![state.fit.unwrap_or(drag.start)],
                                false => heights.clone(),
                            };
                            match snap(drag.now, &heights) {
                                Some(rest) => {
                                    state.rest = rest;
                                    if moves && !motion::reduced(cx) {
                                        let settle = Tween::start(motion::idiom().sheet, cx);
                                        state.settle = Some((drag.now, settle));
                                    }
                                }
                                None => {
                                    state.rest = 0;
                                    if let Some(message) = dismiss {
                                        this.activate();
                                        cx.emit(wire::Event::Message(message));
                                    }
                                }
                            }
                            cx.notify();
                        });
                    });
                },
            )
            .absolute()
            .size_0();
            let measure = fitted.then(|| {
                let view = cx.entity().downgrade();
                let measured = path.clone();
                canvas(
                    move |bounds, _, cx| {
                        let _ = view.update(cx, |this, _| {
                            if let Some(state) = this.sheets.get_mut(&measured)
                                && state.drag.is_none()
                            {
                                state.fit = Some(bounds.size.height);
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
            });
            panel = panel
                .w_full()
                .when_some(height, |panel, height| panel.h(height))
                .max_h(body.height)
                .flex_none()
                .border_t_1()
                .border_color(crate::theme::hsla(palette.border_strong))
                .child(grab)
                .child(capture)
                .children(measure)
                .when_some(shown, |panel, (progress, slides)| match slides {
                    true => panel.top(lifted * (1. - progress)),
                    false => panel.opacity(progress),
                });
        } else {
            panel = panel
                .w_full()
                .max_w(px(WIDEST))
                .max_h(body.height)
                .rounded(px(8.))
                .when(crate::contract::os().facts().touch_first, |panel| {
                    panel.child(div().flex_none().h(px(HANDLE_BAND)))
                });
        }
        let panel = panel
            .child(div().flex_1().min_h_0().child(content))
            .when(leaving.is_some(), |panel| {
                panel.child(div().absolute().inset_0().occlude())
            });
        let named = label.as_deref().is_some_and(|label| !label.is_empty());
        let nested = has_named_overlay(child);
        let faded = shown.map_or(1., |(progress, _)| progress);
        let scrim = div()
            .id(host_id("sheet-scrim"))
            .absolute()
            .inset_0()
            .bg(rgb(0x000000))
            .opacity(match gpui_kit::component::Theme::global(cx).is_dark() {
                true => SCRIM.1 * faded,
                false => SCRIM.0 * faded,
            })
            .role(gpui_kit::Role::Button)
            .aria_label("Close")
            .when_some(dismiss, |scrim, message| {
                scrim.on_click(cx.listener(move |this, _, _, cx| {
                    this.activate();
                    cx.emit(wire::Event::Message(message))
                }))
            });
        let mut layer = div()
            .id(native_id(id))
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .child(scrim)
            .child(panel);
        layer = match compact {
            true => layer.justify_end(),
            false => layer.justify_center().items_center().p(px(16.)),
        };
        let layer = match leaving {
            Some(_) => layer.into_any_element(),
            None => {
                let layer = layer.occlude();
                self.dialog(path, named, nested, layer, dismiss, node, window, cx)
            }
        };
        self.sheet_layer = Some(layer);
        div().absolute().into_any_element()
    }

    pub(super) fn leaving_sheet(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut leaving = self.leaving.take()?;
        if self.sheet_layer.is_some() || leaving.tween.done(motion::now(cx)) {
            return None;
        }
        let progress = leaving.tween.frame(window, cx);
        let slides = leaving.tween.motion.slides;
        let mut path = leaving.path.clone();
        if wire::identity::segment(leaving.node.identity().cloned(), None).is_some() {
            path.pop();
        }
        let path = std::mem::replace(&mut self.authored_path, path);
        let index = std::mem::take(&mut self.render_index);
        std::mem::swap(&mut self.styles, &mut leaving.styles);
        self.leaving_at = Some((progress, leaving.height, slides));
        self.node(&leaving.node, window, cx);
        self.leaving_at = None;
        std::mem::swap(&mut self.styles, &mut leaving.styles);
        self.render_index = index;
        self.authored_path = path;
        self.leaving = Some(leaving);
        self.sheet_layer.take()
    }
}

fn snap(height: Pixels, heights: &[Pixels]) -> Option<usize> {
    let distance = |to: Pixels| f32::from((height - to).abs());
    let (nearest, by) = heights
        .iter()
        .enumerate()
        .map(|(at, to)| (at, distance(*to)))
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    (by <= distance(px(0.))).then_some(nearest)
}

#[cfg(test)]
mod snap_tests {
    use super::*;

    #[test]
    fn a_release_rests_at_the_nearest_detent_or_dismisses() {
        let heights = [px(480.), px(800.)];
        assert_eq!(snap(px(470.), &heights), Some(0));
        assert_eq!(snap(px(700.), &heights), Some(1));
        assert_eq!(snap(px(250.), &heights), Some(0));
        assert_eq!(snap(px(230.), &heights), None);
        assert_eq!(snap(px(0.), &heights), None);
    }
}
