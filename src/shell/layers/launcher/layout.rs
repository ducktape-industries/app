//! The launcher's last step: where the programs sit on this device, across
//! the top or down the side, asked once before the desk (`Screen::Layout`).
//! The pick stays on the layer until Continue saves it (`Account::layout_chosen`).

use super::super::super::ink::{self, *};
use super::{LauncherLayer, LauncherScreen, buttons};
use crate::a11y::Control as _;
use crate::backend::Layout;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

/// The cards, in order: the layout, its name, the line under it.
const CARDS: [(Layout, &str, &str); 2] = [
    (
        Layout::MenuBar,
        "Menu bar",
        "Programs across the top. Their windows open on the desk, to move, stack and split.",
    ),
    (
        Layout::Sidebar,
        "Sidebar",
        "Programs down the side, each with its open windows listed under it.",
    ),
];

impl LauncherLayer {
    /// LayoutStep: two cards, one radio group (one Tab stop, the arrows
    /// pick), and Continue; Enter on the group continues too.
    pub(super) fn layout_step(&self, window: &mut Window, cx: &Context<Self>) -> AnyElement {
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let picked = self.layout;
        let this = cx.weak_entity();
        let pick = Rc::new(move |layout: Layout, cx: &mut App| {
            let _ = this.update(cx, |this, cx| {
                if this.layout != layout {
                    this.layout = layout;
                    cx.notify();
                }
            });
        });
        // read at the press, not the draw: an arrow and Enter can come
        // between two frames
        let go: Rc<dyn Fn(&mut App)> = {
            let (this, account) = (cx.weak_entity(), self.account.entity().clone());
            Rc::new(move |cx| {
                if let Ok(layout) = this.read_with(cx, |this, _| this.layout) {
                    account.update(cx, |account, cx| account.layout_chosen(layout, cx));
                }
            })
        };
        let at = CARDS
            .iter()
            .position(|(layout, ..)| *layout == picked)
            .unwrap_or_default();
        let cards = CARDS.map(|(layout, name, line)| {
            let on = layout == picked;
            let pick = pick.clone();
            let card = div()
                .id(SharedString::from(format!("layout-cards/{name}")))
                .control(Role::RadioButton, name)
                .aria_description(line)
                .aria_toggled(on.into())
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(8.))
                // the chosen card's 1.5px border eats into its padding
                .p(px(if on { 11.5 } else { 12. }))
                .pb(px(if on { 13.5 } else { 14. }))
                .border(px(if on { 1.5 } else { 1. }))
                .border_color(if on { ink.ink } else { ink.strong })
                .cursor_pointer()
                // a press leaves the keys where they were
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(move |_, _, cx| pick(layout, cx))
                .child(thumbnail(layout, &ink))
                .child(
                    sans(400, 15.)
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(radio(on, &ink))
                        .child(name),
                )
                .child(
                    sans(400, 13.)
                        .line_height(px(13. * 1.5))
                        .text_color(ink.muted)
                        .child(line),
                );
            crate::a11y::roving_item(card, on.then_some(&self.layout_stop), ink.ink)
        });
        let enter = go.clone();
        let group = crate::a11y::roving(
            div()
                .id("layout-cards")
                .role(Role::RadioGroup)
                .aria_label("Layout"),
            &self.layout_stop,
            accesskit::Orientation::Horizontal,
            [at, CARDS.len()],
            move |to, _, cx| pick(CARDS[to].0, cx),
        )
        .on_key_down(move |event, _, cx| {
            if event.keystroke.key == "enter" && !event.keystroke.modifiers.modified() {
                cx.stop_propagation();
                enter(cx);
            }
        })
        .flex()
        .gap(px(12.))
        .children(cards);
        let go = buttons([
            ink::button(
                "layout-continue",
                "Continue",
                Kind::Primary,
                move |cx| go(cx),
                false,
                &ink,
            ),
            ink::note(
                "layout-note",
                "Change it any time in Settings, Appearance.",
                ink.muted,
            )
            .into_any_element(),
        ])
        .items_center()
        .gap(px(14.));
        self.frame(
            LauncherScreen {
                id: "layout-step",
                tight: false,
                caption: self.node_caption(cx),
                back: None,
                label: "[04 / 04] Layout".into(),
                headline: "Where should the programs sit?".into(),
                lead: Some("Pick how this device lays out the network's programs.".into()),
                body: vec![group.into_any_element(), go.into_any_element()],
            },
            window,
            cx,
        )
    }
}

/// The card's radio: a 14px ring, chosen a 1.5px ink ring around a 6px dot.
fn radio(on: bool, ink: &Ink) -> Div {
    div()
        .size(px(14.))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .border(px(if on { 1.5 } else { 1. }))
        .border_color(if on { ink.ink } else { ink.strong })
        .when(on, |ring| {
            ring.child(div().size(px(6.)).rounded_full().bg(ink.ink))
        })
}

/// A box at `x, y` of `w × h` on the thumbnail's 240 × 140 grid, scaled to
/// the card's width.
fn at(x: f32, y: f32, w: f32, h: f32) -> Div {
    div()
        .absolute()
        .left(relative(x / 240.))
        .top(relative(y / 140.))
        .w(relative(w / 240.))
        .h(relative(h / 140.))
}

/// A window on the thumbnail's desk: a frame in `line` with its title
/// strip 10 units deep.
fn window_at(x: f32, y: f32, w: f32, h: f32, line: Hsla, ink: &Ink) -> Div {
    at(x, y, w, h)
        .bg(ink.bg)
        .border_1()
        .border_color(line)
        .child(
            div()
                .w_full()
                .h(relative(10. / h))
                .border_b_1()
                .border_color(line),
        )
}

/// The layout as a line drawing (the mock's 240 × 140 one): the bar and
/// two windows, or the column of programs beside them.
fn thumbnail(layout: Layout, ink: &Ink) -> Div {
    let bar = |x, y, w, color| at(x, y, w, 5.).bg(color);
    let frame = div()
        .relative()
        .w_full()
        .aspect_ratio(240. / 140.)
        .flex_shrink_0()
        .overflow_hidden()
        .bg(ink.surface)
        .border_1()
        .border_color(ink.strong);
    match layout {
        Layout::MenuBar => frame
            .child(at(0., 16., 240., 0.).h(px(1.)).bg(ink.strong))
            .child(at(10., 5., 22., 6.).bg(ink.ink))
            .children([40., 62., 84.].map(|x| at(x, 5., 16., 6.).bg(ink.faint)))
            .child(window_at(28., 32., 110., 78., ink.ink, ink))
            .child(window_at(110., 52., 104., 72., ink.strong, ink)),
        Layout::Sidebar => frame
            .child(
                at(0., 0., 62., 140.)
                    .bg(ink.bg)
                    .border_r_1()
                    .border_color(ink.strong),
            )
            .child(at(12., 34., 46., 9.).bg(ink.raised))
            .child(at(8., 6., 26., 6.).bg(ink.ink))
            .children(
                [
                    (8., 26., 30., ink.ink),
                    (16., 36., 34., ink.ink),
                    (16., 46., 26., ink.ink),
                    (8., 60., 30., ink.ink),
                    (16., 70., 38., ink.ink),
                    (8., 84., 30., ink.faint),
                    (8., 96., 22., ink.faint),
                    (8., 108., 28., ink.faint),
                ]
                .map(|(x, y, w, color)| bar(x, y, w, color)),
            )
            .child(window_at(76., 20., 96., 70., ink.ink, ink))
            .child(window_at(132., 56., 96., 72., ink.strong, ink)),
    }
}
