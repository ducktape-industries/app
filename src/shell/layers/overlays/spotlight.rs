//! ⌘K: search the programs, the networks and the things to do.

use super::super::super::ink::*;
use super::super::fields;
use super::{OverlayLayer, dialog_fit, run, scrim};
use crate::a11y::Control as _;
use crate::shell::entities::{Overlay, Spot};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

impl OverlayLayer {
    /// ⌘K: one field, and what it finds among the programs, the networks
    /// and the things to do. ↑↓ pick, Enter runs, Escape closes. To
    /// assistive technology the field and its rows are one combo box whose
    /// active row is the picked one.
    pub(super) fn spotlight(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let input = self.field.clone();
        let field = fields::bare("spotlight", &input, 20., false, cx);
        let rows = self.rows(cx);
        let count = rows.len();
        let pick = self
            .spotlight
            .read(cx)
            .get()
            .pick
            .min(count.saturating_sub(1));
        let mut list = div()
            .id("spotlight-rows")
            .control(Role::ListBox, "Results")
            .max_h(px(380.))
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.spotlight_rows)
            .py(px(6.));
        // each row's place among the list's children (its group's label and
        // the hairline before it count too), for ↑↓ to scroll it into view
        let mut children = 0;
        let mut at = Vec::with_capacity(count);
        // a row Help lists with a chord reports it (AX-114)
        let chords = crate::shell::chords();
        let mut group = "";
        for (nth, row) in rows.into_iter().enumerate() {
            if row.group != group {
                group = row.group;
                // groups `padding: 6px 0`, a hairline between them
                if nth > 0 {
                    list = list.child(div().mt(px(6.)).mb(px(6.)).h(px(1.)).bg(ink.line));
                    children += 1;
                }
                children += 1;
                list = list.child(
                    mono(400, 12.)
                        .text_color(ink.muted)
                        .px(px(16.))
                        .py(px(6.))
                        .child(group),
                );
            }
            let (overlays, app) = (self.overlays.entity().clone(), self.app.clone());
            let spot = row.spot.clone();
            let picked = nth == pick;
            let chord = chords
                .iter()
                .find(|(name, _)| *name == row.title)
                .map(|(_, chord)| chord.clone());
            let hint = match (&row.spot, picked) {
                (_, false) => "",
                (Spot::Switch(_), true) => "switch",
                (_, true) => "↵",
            };
            // the first row brings its group's label back into view with it
            at.push(if nth == 0 { 0 } else { children });
            children += 1;
            list = list.child(
                sans(400, 13.)
                    .id(SharedString::from(format!("spotlight/{nth}")))
                    .control(Role::ListBoxOption, SharedString::from(row.title.clone()))
                    // the picked row is the one the field's ↑↓ move
                    .aria_selected(picked)
                    .when(picked, |row| row.aria_active_descendant())
                    .when_some(chord, |row, chord| row.aria_keyshortcuts(chord))
                    .flex()
                    .items_baseline()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(10.))
                    .cursor_pointer()
                    .when(picked, |row| row.bg(ink.surface))
                    .on_click(move |_, _, cx| run(&overlays, &app, Some(spot.clone()), cx))
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
        // the field and its rows, one combo box: the field holds the keys,
        // the picked row is what they pick
        let combo = crate::a11y::combo_box("spotlight/search", &input.read(cx).focus_handle(cx), {
            let input = input.clone();
            move |value, window, cx| {
                input.update(cx, |input, cx| input.replace_all(value, window, cx))
            }
        })
        .aria_label("Search")
        .aria_value(input.read(cx).value().to_string())
        .aria_expanded(count > 0)
        .flex()
        .flex_col()
        .min_h_0();
        // the field's box, which wears its ring
        let header = div()
            .h(px(tall(56.)))
            .flex_shrink_0()
            .px(px(16.))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(ink.line)
            .gap(px(12.))
            .child(div().flex_1().child(field))
            .child(mono(400, 12.).text_color(ink.muted).child("esc"));
        let header = crate::a11y::around_field(
            header,
            input.read(cx).focus_handle(cx).is_focused(window),
            ink.ring(false),
        );
        let spotlight = self.spotlight.entity().clone();
        let scroll = self.spotlight_rows.clone();
        // the field, the longest list and the key hints
        let (top, tall) = dialog_fit(f32::from(window.viewport_size().height), 490., 84.);
        scrim(
            "spotlight",
            Role::Dialog,
            "Search",
            Overlay::Spotlight,
            self.overlays.entity(),
            &self.modal,
            &ink,
            |card| {
                card.mt(px(top))
                    .max_h(px(tall))
                    .w(px(600.))
                    .max_w_full()
                    .h_auto()
                    .self_start()
                    .capture_key_down(move |event: &KeyDownEvent, _, cx| {
                        let down = match event.keystroke.key.as_str() {
                            "up" => false,
                            "down" => true,
                            _ => return,
                        };
                        let to = match down {
                            false => pick.saturating_sub(1),
                            true => (pick + 1).min(count.saturating_sub(1)),
                        };
                        if let Some(child) = at.get(to) {
                            scroll.scroll_to_item(*child);
                        }
                        cx.stop_propagation();
                        spotlight.update(cx, |spotlight, cx| spotlight.move_pick(down, count, cx));
                    })
                    .child(combo.child(header).child(list))
                    .child(
                        mono(400, 12.)
                            .flex_shrink_0()
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
