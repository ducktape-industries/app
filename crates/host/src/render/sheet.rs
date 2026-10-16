//! The Sheet node: a panel over a scrim, the view's one child inside it.
//! Under a Compact body (`design::width::COMPACT_BELOW`, the class the
//! view itself reads off `Event::Viewport`) it docks to the bottom edge at
//! its first detent, is dragged by its handle between its detents, and a
//! release nearer the bottom than the first detent dismisses it; at a wider
//! body it is a centred modal. Either way it covers the view's whole body,
//! not the box it sits in: the walk builds it where the node is (its ids,
//! fields and focus keep their paths) and `ViewTree::render` lays it over
//! the root. The view owns whether it is open: a dismissal (the scrim, a
//! drag down, Escape) is the view's `on_dismiss`, and the panel stays at
//! its first detent until the view's next frame drops it, so a sheet the
//! view keeps (or opens again) is never left half-closed.
use super::*;
use gpui_kit::prelude::FluentBuilder as _;

/// What the host keeps of a drawn sheet between frames: the detent it
/// rests at, and the drag in progress.
#[derive(Default)]
pub(super) struct SheetState {
    rest: usize,
    drag: Option<Drag>,
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
        let state = self.sheets.entry(path.clone()).or_default();
        // sanitize keeps one to eight ascending fractions in (0, 1]
        let rest = detents[state.rest.min(detents.len() - 1)];
        let height = match &state.drag {
            Some(drag) => drag.now,
            None => body.height * rest,
        };
        let content = self.node(child, window, cx);
        let dismiss = *on_dismiss;
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
            let top = *detents.last().unwrap_or(&1.);
            let view = cx.entity().downgrade();
            let (pressed, moved, released) = (path.clone(), path.clone(), path.clone());
            let heights: Vec<Pixels> = detents.iter().map(|d| body.height * *d).collect();
            let highest = body.height * top;
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
                            state.drag = Some(Drag {
                                from: event.position.y,
                                start: height,
                                now: height,
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
                            let Some(drag) =
                                this.sheets.get_mut(&moved).and_then(|s| s.drag.as_mut())
                            else {
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
                            match snap(drag.now, &heights) {
                                Some(rest) => state.rest = rest,
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
            panel = panel
                .w_full()
                .h(height)
                .flex_none()
                // a hairline over the panel, square corners: the Expo sheet
                .border_t_1()
                .border_color(crate::theme::hsla(palette.border_strong))
                .child(grab)
                .child(capture);
        } else {
            panel = panel
                .w_full()
                .max_w(px(WIDEST))
                .max_h(body.height * *detents.last().unwrap_or(&1.))
                .rounded(px(8.))
                // under touch the view leaves the panel's top to the
                // handle's band; a centred modal does not drag, so the band
                // stays, empty, and the view's head is never flush
                .when(crate::contract::os().facts().touch_first, |panel| {
                    panel.child(div().flex_none().h(px(HANDLE_BAND)))
                });
        }
        let panel = panel.child(div().flex_1().min_h_0().child(content));
        let named = label.as_deref().is_some_and(|label| !label.is_empty());
        let nested = has_named_overlay(child);
        let scrim = div()
            .id(host_id("sheet-scrim"))
            .absolute()
            .inset_0()
            .bg(rgb(0x000000))
            .opacity(match gpui_kit::component::Theme::global(cx).is_dark() {
                true => SCRIM.1,
                false => SCRIM.0,
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
            // what the sheet covers takes no press while it is open
            .occlude()
            .flex()
            .flex_col()
            .child(scrim)
            .child(panel);
        layer = match compact {
            true => layer.justify_end(),
            false => layer.justify_center().items_center().p(px(16.)),
        };
        if let Some(message) = dismiss {
            layer = layer.on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                // a dismissal is no gesture, as Escape is not on the web
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    cx.emit(wire::Event::Message(message));
                }
            }));
        }
        let layer = self.dialog(path, named, nested, layer, node, window, cx);
        // the walk goes on where the node sits; the layer goes over the body
        self.sheet_layer = Some(layer);
        div().absolute().into_any_element()
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
