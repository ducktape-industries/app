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
//! `BottomSheetModal`, `motion::SHEET_IN`): it slides up by its own height
//! as it first draws, a release settles at its detent, and one the view
//! drops goes down as it was last drawn ([`Leaving`]), the scrim fading
//! with it. A pointer window's sheet does not move.
use super::*;
use crate::motion::{self, Tween};
use gpui_kit::prelude::FluentBuilder as _;

/// What the host keeps of a drawn sheet between frames: the detent it
/// rests at, the drag in progress, and, for a sheet with no detents, the
/// height its child gave it at rest, where a drag starts and snaps back to.
#[derive(Default)]
pub(super) struct SheetState {
    rest: usize,
    drag: Option<Drag>,
    fit: Option<Pixels>,
    /// Under touch: the panel coming up as it first draws.
    came: Option<Tween>,
    /// Under touch: a release settling at its rest, from the height it was
    /// let go at.
    settle: Option<(Pixels, Tween)>,
    /// Under touch: the panel's height as last drawn (a drag's, a detent's,
    /// its child's), which a sheet the view drops goes down from.
    drawn: Option<Pixels>,
}

/// A docked sheet the view dropped under touch, going down as it was last
/// drawn: its node and the style table it was drawn with, where it sat in
/// the tree, its height, its motion (1 in place, 0 gone). It takes no
/// press, and nothing it draws is a dialog.
pub(super) struct Leaving {
    node: wire::Node,
    styles: wire::Styles,
    /// Its scrolls and lists are kept while it goes (`ViewTree::replace`).
    pub(super) path: AuthoredPath,
    height: Pixels,
    tween: Tween,
}

impl SheetState {
    /// Whether it was drawn docked under touch, where a sheet moves.
    pub(super) fn moves(&self) -> bool {
        self.drawn.is_some()
    }

    /// What goes down when the view drops the sheet `node` (at `path`, drawn
    /// with `styles`) this state kept: `None` unless it was drawn docked
    /// under touch.
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
            tween: Tween::between(from, 0., motion::SHEET_OUT, cx),
        })
    }
}

impl SheetState {
    /// The detent it rests at, by its place in the sheet's `detents`.
    #[cfg(test)]
    pub(super) fn rest(&self) -> usize {
        self.rest
    }
}

/// A drag of the handle: where the press was, the panel's height then, and
/// its height now.
struct Drag {
    from: Pixels,
    start: Pixels,
    now: Pixels,
}

/// The widest a sheet's panel is drawn: the widest Compact body, so a
/// sheet reads the same in a centred modal as docked on a phone.
const WIDEST: f32 = design::width::COMPACT_BELOW as f32;
/// The handle's band, and the handle in it (the Expo app's `Sheet` handle).
pub(super) const HANDLE_BAND: f32 = 20.;
const HANDLE: (f32, f32) = (32., 4.);
/// The scrim's black, by appearance: the Expo palette's `scrim`
/// (mobile `src/design/theme.ts`, 0.4 light, 0.55 dark).
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
        // under touch a docked sheet moves; a pointer's never does
        let moves = compact && crate::contract::os().facts().touch_first;
        let now = motion::now(cx);
        // a sheet opened again as it goes down comes up from where it is
        let from = self
            .leaving
            .as_ref()
            .filter(|leaving| leaving.path == path)
            .map_or(0., |leaving| leaving.tween.at(now));
        // a sheet the view dropped, walked once more as it goes down
        // (`leaving_sheet`): its height as last drawn, no state of its own
        let leaving = self.leaving_at;
        let mut kept = SheetState::default();
        let state = match leaving {
            Some(_) => &mut kept,
            None => self.sheets.entry(path.clone()).or_default(),
        };
        if moves && leaving.is_none() && state.drawn.is_none() && state.came.is_none() {
            state.came = Some(Tween::between(from, 1., motion::SHEET_IN, cx));
        }
        // sanitize keeps at most eight ascending fractions in (0, 1]; with
        // none the panel is laid out as tall as its child, and only a drag
        // gives it a height
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
            // a release settling at its rest, a fitted one at its child's
            (None, Some((from, tween)), None) => {
                let to = rest.or(state.fit).unwrap_or(from);
                Some(from + (to - from) * tween.frame(window, cx))
            }
            (None, None, None) => rest,
        };
        // how far in the panel is, and whether it slides there (else, under
        // Reduce Motion, it fades)
        if state.came.is_some_and(|came| came.done(now)) {
            state.came = None;
        }
        let shown = match (leaving, state.came) {
            (Some((progress, _, slides)), _) => Some((progress, slides)),
            (None, Some(came)) => Some((came.frame(window, cx), came.motion.slides)),
            (None, None) => None,
        };
        // down by its own height at 0; unmeasured, below the body
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
            // the view's own look (its ground, its padding) is the panel's
            .refine_style(&style)
            // a press on the panel is the panel's, not the scrim's
            .occlude();
        if compact {
            let view = cx.entity().downgrade();
            let (pressed, moved, released) = (path.clone(), path.clone(), path.clone());
            // a fitted sheet rests at the height its child gave it, measured
            // as it last lay at rest, and read as the drag moves and ends:
            // the frame that first draws it has measured nothing yet
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
                            // a press takes the panel where it is, settling or not
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
            // the drag goes on over the scrim and past the panel: the
            // window's moves and release, in capture, while it lasts
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
                            // a fitted sheet's one rest; unmeasured, where
                            // the press found it, so a tap never dismisses
                            let heights = match fitted {
                                true => vec![state.fit.unwrap_or(drag.start)],
                                false => heights.clone(),
                            };
                            match snap(drag.now, &heights) {
                                Some(rest) => {
                                    state.rest = rest;
                                    // under touch it settles there from where
                                    // it was let go; under Reduce Motion it
                                    // is there at once
                                    if moves && !motion::reduced(cx) {
                                        let settle = Tween::start(motion::SHEET_IN, cx);
                                        state.settle = Some((drag.now, settle));
                                    }
                                }
                                None => {
                                    // back to where it opens: the view closes it
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
            // the height a fitted panel's child gives it, taken at rest:
            // mid-drag the panel is as tall as the drag
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
                // a hairline over the panel, square corners: the Expo sheet
                .border_t_1()
                .border_color(crate::theme::hsla(palette.border_strong))
                .child(grab)
                .child(capture)
                .children(measure)
                // coming up or going down by its own height, or fading
                .when_some(shown, |panel, (progress, slides)| match slides {
                    true => panel.top(lifted * (1. - progress)),
                    false => panel.opacity(progress),
                });
        } else {
            panel = panel
                .w_full()
                .max_w(px(WIDEST))
                // a modal is as tall as its child, at most the body: the
                // detents are the docked sheet's
                .max_h(body.height)
                .rounded(px(8.))
                // under touch the view leaves the panel's top to the
                // handle's band; a centred modal does not drag, so the band
                // stays, empty, and the view's head is never flush
                .when(crate::contract::os().facts().touch_first, |panel| {
                    panel.child(div().flex_none().h(px(HANDLE_BAND)))
                });
        }
        let panel = panel
            .child(div().flex_1().min_h_0().child(content))
            // a sheet going takes no press: a cover over what it draws
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
            // a tap on the scrim, or assistive technology's press on Close:
            // a press, so a gesture
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
            // going: no dialog, and what it covers takes presses again
            Some(_) => layer.into_any_element(),
            None => {
                // what the sheet covers takes no press while it is open
                let layer = layer.occlude();
                self.dialog(path, named, nested, layer, dismiss, node, window, cx)
            }
        };
        // the walk goes on where the node sits; the layer goes over the body
        self.sheet_layer = Some(layer);
        div().absolute().into_any_element()
    }

    /// The sheet the view dropped, walked once more where it sat, with the
    /// table it was drawn with, as it goes down: its layer, while its motion
    /// lasts and no other sheet is drawn.
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
        // the walk enters the sheet's own scope from where it sat
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

/// The detent a release at `height` rests at: the nearest of `heights`,
/// or `None` when the bottom (a dismissal) is nearer than the first.
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

    /// A release rests at the nearest detent; nearer the bottom than the
    /// first, it dismisses.
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
