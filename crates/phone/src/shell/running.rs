//! A program on top of the stack: its view fills the screen inside the safe
//! area, and the capsule (`···` and `×`) is the one piece of chrome over it,
//! fixed at the top right (the owner's (가): no host bar, no back, no
//! title; the view's own header leaves the capsule's place empty).
//!
//! The view is laid out at the screen's width whatever its manifest's
//! `min_width` (the narrowest pointer window): never widened, scrolled or
//! zoomed for it (the seat sends it the body as it is under touch).

use super::look::{self, Palette};
use super::{Root, home};
use ducktape_host::a11y::Control as _;
use ducktape_host::runtime::{RailRow, Seat};
use gpui_kit::{
    AnyElement, AnyView, App, Context, Entity, InteractiveElement as _, IntoElement, KeyContext,
    ParentElement as _, Role, StatefulInteractiveElement as _, StyleRefinement, Styled as _, div,
    px, svg,
};

/// The capsule: two 42 cells, 36 tall with its border.
const CAPSULE_CELL: f32 = 42.;
const CAPSULE: f32 = 36.;
/// Where the capsule sits in the 56 header band, and its gap to the edge.
const CAPSULE_TOP: f32 = (look::HEADER - CAPSULE) / 2.;
const CAPSULE_RIGHT: f32 = 8.;

/// What the seat shows: its standin (a load, a failure, a stopped view)
/// over any tree it keeps, else its tree, else the program's opening state
/// until the seat is there.
pub fn body(seat: Option<&Entity<Seat>>, row: Option<&RailRow>, palette: Palette, cx: &App) -> AnyElement {
    let Some(seat) = seat.map(|seat| seat.read(cx)) else {
        return opening(row, palette);
    };
    match (seat.standin(), seat.tree()) {
        (Some(standin), _) => standin.element(seat.program(), seat.instance(), seat.ax_mark()),
        (None, Some(tree)) => {
            let view = AnyView::from(tree).cached(StyleRefinement::default().size_full());
            let mut context = KeyContext::default();
            context.set("ducktape_view", format!("view{}", seat.instance()));
            div()
                .id(seat.ax_mark())
                .key_context(context)
                .size_full()
                .overflow_hidden()
                .child(view)
                .into_any_element()
        }
        (None, None) => opening(row, palette),
    }
}

fn opening(row: Option<&RailRow>, palette: Palette) -> AnyElement {
    match row {
        Some(row) => home::opening(row, palette).into_any_element(),
        None => div().size_full().into_any_element(),
    }
}

/// The capsule over `program`, at the top right of the safe area whose
/// top and right insets are `top` and `right`.
pub fn capsule(
    program: &'static str,
    top: gpui_kit::Pixels,
    right: gpui_kit::Pixels,
    palette: Palette,
    cx: &mut Context<Root>,
) -> AnyElement {
    let cell = |id: &'static str, name: &'static str, icon: &'static str| {
        div()
            .id(id)
            .control(Role::Button, name)
            .w(px(CAPSULE_CELL))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(icon)
                    .size(px(look::ICON_SHEET))
                    .text_color(palette.foreground()),
            )
    };
    div()
        .absolute()
        .top(top + px(CAPSULE_TOP))
        .right(right + px(CAPSULE_RIGHT))
        .h(px(CAPSULE))
        .flex()
        .occlude()
        .bg(palette.background())
        .border_1()
        .border_color(palette.border_strong())
        .child(
            cell("capsule/menu", "More", "icons/ellipsis.svg")
                .on_click(cx.listener(move |root, _, _, cx| root.capsule_menu(program, cx))),
        )
        .child(div().w(px(1.)).h_full().bg(palette.border_strong()))
        .child(
            cell("capsule/close", "Close", "icons/x.svg")
                .on_click(cx.listener(|root, _, _, cx| root.go_home(cx))),
        )
        .into_any_element()
}
