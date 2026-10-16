//! The phone's bottom sheets (Expo `Sheet`, `SheetHead`, `SheetRow`, mobile
//! `src/kit/Sheet.tsx`): docked to the bottom over the scrim, square, a
//! hairline over the top, the 20 handle band, padded by the bottom inset.
//! A press on the scrim closes the sheet, as Back does; a sheet is the
//! first thing Back closes (SPEC §7.6).

use super::Root;
use super::look::{self, Palette};
use ducktape_host::a11y::Control as _;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div, px,
    svg,
};

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

/// A sheet over the window, `id` its element's: the scrim over everything,
/// `content` docked to its bottom under the handle, at most `room` tall,
/// padded by the bottom inset `bottom`; a press on the scrim is `close`.
pub fn frame(
    id: &'static str,
    name: &'static str,
    content: impl IntoElement,
    (bottom, room): (Pixels, Pixels),
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
    let panel = div()
        .id(id)
        .control(Role::Dialog, name)
        .occlude()
        .flex()
        .flex_col()
        .w_full()
        .max_h(room)
        .pb(bottom)
        .bg(palette.background())
        .border_t_1()
        .border_color(palette.border_strong())
        // a press on the panel is not one on the scrim
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(handle)
        .child(content);
    div()
        .id(SharedString::from(format!("{id}/scrim")))
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .occlude()
        .bg(palette.scrim())
        .flex()
        .flex_col()
        .justify_end()
        .on_click(cx.listener(move |root, _, _, cx| close(root, cx)))
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
