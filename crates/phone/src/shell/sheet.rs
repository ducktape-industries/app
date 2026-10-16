//! The phone's bottom sheets (Expo `Sheet`, `SheetHead`, `SheetRow`, mobile
//! `src/kit/Sheet.tsx`): docked to the bottom over the scrim, square, a
//! hairline over the top, the 20 handle band, padded by the bottom inset.
//! A press on the scrim closes the sheet, as Back does; a sheet is the
//! first thing Back closes (SPEC §7.6). It slides up by its own height as
//! it comes and down as it goes, the scrim fading with it (gorhom's
//! `BottomSheetModal` and `BottomSheetBackdrop`, `motion::SHEET_IN`); a
//! sheet going draws what it drew open and takes no press.

use super::Root;
use super::look::{self, Palette};
use ducktape_host::a11y::Control as _;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, canvas,
    div, px, svg,
};
use std::cell::Cell;
use std::rc::Rc;

/// A sheet the shell has open over the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    /// The account menu: the Account tab, or the avatar on a root's header.
    Account,
    /// The capsule's `···` over `program`.
    Capsule(&'static str),
    /// The three words for `program`'s notices.
    Notifications(&'static str),
}

/// Where a sheet is drawn: the window's bottom inset and the room over it,
/// how far in it has come, and its panel's height as last laid out.
pub struct At {
    /// The bottom inset the panel is padded by.
    pub bottom: Pixels,
    /// The tallest the panel is.
    pub room: Pixels,
    /// How far in it is: 0 gone, 1 in place.
    pub progress: f32,
    /// Whether it slides (else, under Reduce Motion, it fades in place).
    pub slides: bool,
    /// Whether it is going: it takes no press.
    pub leaving: bool,
    /// The panel's height at its last layout; `None` before the first.
    pub height: Rc<Cell<Option<Pixels>>>,
}

/// A sheet over the window, `id` its element's: the scrim over everything,
/// `content` docked to its bottom under the handle, at most `at.room`
/// tall, padded by the bottom inset, as far in as `at` says; a press on the
/// scrim is `close`.
pub fn frame(
    id: &'static str,
    name: &'static str,
    content: impl IntoElement,
    at: At,
    close: fn(&mut Root, &mut Context<Root>),
    palette: Palette,
    cx: &mut Context<Root>,
) -> AnyElement {
    let handle = div()
        .flex_none()
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(32.))
                .h(px(4.))
                .rounded(px(2.))
                .bg(palette.border_strong()),
        );
    // down by its own height at 0; below the window until it is laid out
    let down = match (at.slides, at.height.get()) {
        (false, _) => px(0.),
        (true, Some(height)) => height * (1. - at.progress),
        (true, None) => at.room + at.bottom,
    };
    let height = at.height.clone();
    let measure = canvas(
        move |bounds, _, _| height.set(Some(bounds.size.height)),
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0();
    let mut panel = div()
        .id(id)
        .control(Role::Dialog, name)
        .occlude()
        .relative()
        .top(down)
        .opacity(if at.slides { 1. } else { at.progress })
        .flex()
        .flex_col()
        .w_full()
        .max_h(at.room)
        .pb(at.bottom)
        .bg(palette.background())
        .border_t_1()
        .border_color(palette.border_strong())
        // a press on the panel is not one on the scrim
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(measure)
        .child(handle)
        .child(content);
    let mut scrim = div()
        .id(SharedString::from(format!("{id}/scrim")))
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .bg(palette.scrim().opacity(at.progress))
        .flex()
        .flex_col()
        .justify_end();
    if at.leaving {
        // what it drew open, under a cover that takes its presses; the
        // screen under the scrim takes them already
        panel = panel.child(div().absolute().inset_0().occlude());
        return scrim.child(panel).into_any_element();
    }
    scrim = scrim
        .occlude()
        .on_click(cx.listener(move |root, _, _, cx| close(root, cx)));
    scrim
        .child(ducktape_host::a11y::modal(panel))
        .into_any_element()
}

/// A sheet's head (Expo `SheetHead`): what the sheet is about, a hairline
/// under it.
pub fn head(palette: Palette) -> Div {
    div()
        .pt(px(look::gap::XXS))
        .pb(px(look::gap::LG))
        .px(px(look::SHEET_INSET))
        .border_b_1()
        .border_color(palette.border())
}

/// A 52 sheet row (Expo `SheetRow`): its icon 20 muted (its place kept
/// empty with none, so a list of choices lines up), its label, a hint at
/// the right.
pub fn row(
    id: impl Into<SharedString>,
    icon: Option<&'static str>,
    label: impl Into<SharedString>,
    hint: Option<SharedString>,
    palette: Palette,
) -> Stateful<Div> {
    let label = label.into();
    let icon = match icon {
        Some(icon) => svg()
            .path(icon)
            .size(px(look::ICON_SHEET))
            .flex_none()
            .text_color(palette.muted())
            .into_any_element(),
        None => div()
            .size(px(look::ICON_SHEET))
            .flex_none()
            .into_any_element(),
    };
    let mut row = div()
        .id(id.into())
        .control(Role::Button, label.clone())
        .h(px(look::SHEET_ROW))
        .flex()
        .items_center()
        .gap(px(14.))
        .px(px(look::SHEET_INSET))
        .active(|style| style.opacity(look::PRESSED))
        .child(icon)
        .child(
            look::text(look::BODY, FontWeight::NORMAL, palette.foreground())
                .flex_1()
                .min_w_0()
                .truncate()
                .child(label),
        );
    if let Some(hint) = hint {
        row = row.child(
            look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                .flex_none()
                .child(hint),
        );
    }
    row
}
