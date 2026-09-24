//! ⌘K: search the programs, the networks and the things to do.

use super::*;
use crate::Spot;
use screens::Facts;

impl DesktopWindow {
    /// ⌘K: one field, and what it finds among the programs, the networks
    /// and the things to do. ↑↓ pick, Enter runs, Escape closes.
    pub(super) fn spotlight(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let field = self.input(
            "spotlight",
            "Search programs, networks, actions",
            false,
            |state| &state.spotlight_query,
            Message::SpotlightTyped,
            || Message::SpotlightSubmit,
            Some("Search".into()),
            false,
            20.,
            window,
            cx,
        );
        if !self.spotlight_focused {
            self.spotlight_focused = true;
            if let Some(input) = self.inputs.get("spotlight") {
                let state = input.state.clone();
                window.defer(cx, move |window, cx| {
                    state.update(cx, |state, cx| state.focus(window, cx));
                });
            }
        }
        let rows = self.model.read(cx).state.spotlight_rows();
        let count = rows.len();
        let pick = state.spotlight_pick.min(count.saturating_sub(1));
        let mut list = div()
            .id("spotlight-rows")
            .role(Role::Menu)
            .max_h(px(380.))
            .overflow_y_scroll()
            .py(px(6.));
        let mut group = "";
        for (nth, row) in rows.into_iter().enumerate() {
            if row.group != group {
                group = row.group;
                // groups `padding: 6px 0`, a hairline between them
                if nth > 0 {
                    list = list.child(div().mt(px(6.)).mb(px(6.)).h(px(1.)).bg(ink.line));
                }
                list = list.child(
                    mono(400, 12.)
                        .text_color(ink.muted)
                        .px(px(16.))
                        .py(px(6.))
                        .child(group),
                );
            }
            let model = self.model.clone();
            let spot = row.spot.clone();
            let picked = nth == pick;
            let hint = match (&row.spot, picked) {
                (_, false) => "",
                (Spot::Switch(_), true) => "switch",
                (_, true) => "↵",
            };
            list = list.child(
                sans(400, 13.)
                    .id(SharedString::from(format!("spotlight/{nth}")))
                    .control(Role::MenuItem, SharedString::from(row.title.clone()))
                    .flex()
                    .items_baseline()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(10.))
                    .cursor_pointer()
                    .when(picked, |row| row.bg(ink.surface))
                    .on_click(move |_, _, cx| {
                        let spot = spot.clone();
                        model.update(cx, |model, cx| model.dispatch(Message::Spot(spot), cx));
                    })
                    .child(
                        sans(if picked { 500 } else { 400 }, 15.)
                            .text_color(ink.ink)
                            .child(row.title),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(ink.muted)
                            .child(row.meta),
                    )
                    .child(mono(400, 12.).text_color(ink.muted).child(hint)),
            );
        }
        if count == 0 {
            list = list.child(
                sans(400, 14.)
                    .px(px(16.))
                    .py(px(12.))
                    .text_color(ink.muted)
                    .child("Nothing here by that name."),
            );
        }
        let keys = self.model.clone();
        self.overlay(
            "spotlight",
            Role::Dialog,
            "Search",
            crate::Overlay::Spotlight,
            true,
            &ink,
            |card| {
                card.mt(px(84.))
                    .w(px(600.))
                    .max_w_full()
                    .h_auto()
                    .self_start()
                    .capture_key_down(move |event: &KeyDownEvent, _, cx| {
                        let message = match event.keystroke.key.as_str() {
                            "up" => Message::SpotlightMove {
                                down: false,
                                rows: count,
                            },
                            "down" => Message::SpotlightMove {
                                down: true,
                                rows: count,
                            },
                            _ => return,
                        };
                        cx.stop_propagation();
                        keys.update(cx, |model, cx| model.dispatch(message, cx));
                    })
                    .child(
                        div()
                            .h(px(56.))
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(ink.line)
                            .gap(px(12.))
                            .child(div().flex_1().child(field))
                            .child(mono(400, 12.).text_color(ink.muted).child("esc")),
                    )
                    .child(list)
                    .child(
                        mono(400, 12.)
                            .flex()
                            .gap(px(20.))
                            .px(px(16.))
                            .py(px(10.))
                            .border_t_1()
                            .border_color(ink.line)
                            .text_color(ink.muted)
                            .child("↑↓ move")
                            .child("↵ open")
                            .child("esc close"),
                    )
                    .into_any_element()
            },
        )
    }
}
