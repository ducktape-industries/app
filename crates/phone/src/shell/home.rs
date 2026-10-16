//! What the stack's screens draw: Home (the network's programs, IA B,
//! wasm-mobile SPEC §7.2), Open, Account, a program pushed over them, and
//! the tab bar. Drawn in the Expo kit's measures ([`look`]); the words are
//! the host chrome's, English as the Expo app's.
//!
//! No view is seated here yet: a program pushed shows the host's opening
//! state (its glyph, name and the stage its rail row is at, SPEC §7.3) until
//! the seating cut puts its view under the capsule.

use super::look::{self, Palette};
use super::{Root, stack::Tab};
use ducktape_host::a11y::Control as _;
use ducktape_host::runtime::RailRow;
use ducktape_host::session::Shared;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Role, SharedString, StatefulInteractiveElement as _, Styled as _, div, px, relative,
    svg,
};

/// Home's title: the store's shape, never the store's word (SPEC §7.2).
pub const HOME_TITLE: &str = "Programs on this network";

/// A program's name before its view's manifest names it: its id with a
/// capital ("chat" -> "Chat"), as the desktop's bar shows it.
fn name_of(row: &RailRow) -> String {
    match row.note {
        None => row.label.clone(),
        Some(_) => {
            let spaced = row.label.replace('-', " ");
            let mut chars = spaced.chars();
            chars.next().map_or(spaced.clone(), |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        }
    }
}

/// The rows Home lists: every program the network lists with a view, in the
/// roster's order (the desktop rail's), none privileged.
fn listed(shared: &Shared, cx: &gpui_kit::App) -> Vec<RailRow> {
    shared
        .rail
        .read(cx)
        .rows()
        .iter()
        .filter(|row| !row.empty)
        .cloned()
        .collect()
}

/// The 56 header of a tab root (Expo `Header` with `root`): the account's
/// avatar in a 44 hit, the 22 title, a 12 caption under it.
fn root_header(title: &str, caption: Option<String>, palette: Palette) -> Div {
    let avatar = div()
        .size(px(look::HIT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .size(px(look::AVATAR))
                .rounded_full()
                .bg(palette.surface_raised())
                .flex()
                .items_center()
                .justify_center()
                .child(look::text(look::SECONDARY, FontWeight::MEDIUM, palette.muted()).child("?")),
        );
    let mut words = div()
        .flex_1()
        .min_w_0()
        .px(px(8.))
        .flex()
        .flex_col()
        .justify_center()
        .child(
            look::text(look::BIG_TITLE, FontWeight::MEDIUM, palette.foreground())
                .child(title.to_owned()),
        );
    if let Some(caption) = caption {
        words = words
            .child(look::text(look::CAPTION, FontWeight::NORMAL, palette.muted()).child(caption));
    }
    div()
        .h(px(look::HEADER))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.))
        .px(px(8.))
        .child(avatar)
        .child(words)
}

/// The caption under Home's title: the network in hand, or where the phone
/// is trying to get to.
fn network_caption(shared: &Shared, cx: &gpui_kit::App) -> String {
    let session = shared.session.read(cx).get();
    if session.connected && session.reconnecting {
        format!("{} · reconnecting", session.network)
    } else if session.connected {
        session.network.clone()
    } else if session.connecting {
        format!("Connecting to {}…", session.reaching)
    } else if !session.error.is_empty() {
        session.error.clone()
    } else {
        "Not connected".to_owned()
    }
}

/// A section's head (Expo `SectionHead`): 12 medium muted label, its count
/// at the right.
fn section_head(label: &str, count: usize, palette: Palette) -> Div {
    div()
        .min_h(px(40.))
        .flex()
        .items_center()
        .justify_between()
        .pt(px(12.))
        .pb(px(4.))
        .px(px(look::GUTTER))
        .child(
            look::text(look::CAPTION, FontWeight::MEDIUM, palette.muted()).child(label.to_owned()),
        )
        .child(
            look::text(look::CAPTION, FontWeight::NORMAL, palette.muted()).child(count.to_string()),
        )
}

/// A program's glyph: the icon its view's manifest declares, monochrome, or
/// its name's initial when it declares none (SPEC §7.7).
fn glyph(row: &RailRow, side: f32, icon: f32, palette: Palette) -> Div {
    let tile = div()
        .size(px(side))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .bg(palette.surface_raised());
    if row.icon.is_empty() {
        let initial = name_of(row)
            .chars()
            .next()
            .map(String::from)
            .unwrap_or_default();
        tile.child(look::text(look::BODY, FontWeight::MEDIUM, palette.foreground()).child(initial))
    } else {
        tile.child(
            svg()
                .path(SharedString::from(row.icon.clone()))
                .size(px(icon))
                .text_color(palette.foreground()),
        )
    }
}

/// Home: the network's programs, three across; a tile opens its program.
pub fn home(shared: &Shared, palette: Palette, cx: &mut Context<Root>) -> Div {
    let rows = listed(shared, cx);
    let badges = shared.rail.read(cx).badges().clone();
    let tiles = rows.iter().map(|row| {
        let program = row.program;
        let name = name_of(row);
        let mut face = glyph(row, 56., 28., palette).relative();
        if badges.get(program).is_some_and(|count| *count > 0) {
            // the unread dot: never a count (SPEC §7.2, Expo `Dot`)
            face = face.child(
                div()
                    .absolute()
                    .top(px(-4.))
                    .right(px(-4.))
                    .size(px(10.))
                    .rounded_full()
                    .bg(palette.accent())
                    .border_2()
                    .border_color(palette.background()),
            );
        }
        let mut tile = div()
            .id(SharedString::from(format!("home/{program}")))
            .control(Role::Button, name.clone())
            .w(relative(1. / 3.))
            .min_h(px(112.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .pt(px(10.))
            .pb(px(12.))
            .px(px(4.))
            .child(face)
            .child(
                look::text(look::SECONDARY, FontWeight::NORMAL, palette.foreground())
                    .max_w_full()
                    .truncate()
                    .child(name),
            )
            .on_click(cx.listener(move |root, _, _, cx| root.open(program, cx)));
        if let Some(stage) = row.note {
            tile = tile.child(
                look::text(look::TINY, FontWeight::NORMAL, palette.muted())
                    .mt(px(-4.))
                    .child(stage),
            );
        }
        tile
    });
    let body = div()
        .id("home")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .child(section_head("Programs", rows.len(), palette))
        .child(
            div()
                .flex()
                .flex_wrap()
                .px(px(look::GUTTER - 4.))
                .py(px(8.))
                .children(tiles),
        );
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(root_header(
            HOME_TITLE,
            Some(network_caption(shared, cx)),
            palette,
        ))
        .child(body)
}

/// Open: the programs already open. None is kept warm before the seating
/// cut, so it says what it will hold (Expo `EmptyState`).
pub fn open(palette: Palette) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(root_header("Open", None, palette))
        .child(empty(
            "icons/archive.svg",
            "Nothing open",
            "A program you open stays here, where you left it.",
            palette,
        ))
}

/// Account: the `programs` program's view, the one the desktop's Account
/// menu opens by name (SPEC §7.4).
pub fn account(shared: &Shared, palette: Palette, cx: &mut Context<Root>) -> Div {
    let row = listed(shared, cx)
        .into_iter()
        .find(|row| row.program == ACCOUNT_PROGRAM);
    let body = match row {
        Some(row) => opening(&row, palette).into_any_element(),
        None => empty(
            "icons/circle-user.svg",
            "No account program",
            "This network lists no account program yet.",
            palette,
        )
        .into_any_element(),
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(root_header("Account", None, palette))
        .child(body)
}

/// The system program whose view is the account (`ARCHITECTURE.md` §1: the
/// host names `identity` and `programs` only).
const ACCOUNT_PROGRAM: &str = "programs";

/// Nothing to show yet (Expo `EmptyState`): a 28 faint icon, what is
/// missing (15 medium), what would fill it (13 muted).
fn empty(icon: &'static str, title: &str, body: &str, palette: Palette) -> Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .py(px(40.))
        .px(px(32.))
        .child(
            svg()
                .path(icon)
                .size(px(look::ICON_EMPTY))
                .text_color(palette.faint()),
        )
        .child(
            look::text(look::BODY, FontWeight::MEDIUM, palette.foreground())
                .child(title.to_owned()),
        )
        .child(
            look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted())
                .max_w(px(280.))
                .text_center()
                .child(body.to_owned()),
        )
}

/// A program on its way (SPEC §7.3 "loading"): its glyph, its name and the
/// stage its rail row is at.
fn opening(row: &RailRow, palette: Palette) -> Div {
    let mut words = div().flex().flex_col().items_center().gap(px(4.)).child(
        look::text(look::BODY, FontWeight::MEDIUM, palette.foreground()).child(name_of(row)),
    );
    if let Some(stage) = row.note {
        words = words
            .child(look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted()).child(stage));
    }
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .child(glyph(row, 64., 32., palette))
        .child(words)
}

/// A program pushed over the tabs: its name at the left of the host's top
/// line, the capsule at the right (the owner's (가): the capsule is the only
/// chrome over a running program), its body under them.
pub fn program(
    program: &'static str,
    shared: &Shared,
    palette: Palette,
    cx: &mut Context<Root>,
) -> AnyElement {
    let Some(row) = listed(shared, cx)
        .into_iter()
        .find(|row| row.program == program)
    else {
        // the network stopped listing it while it was open
        return div().flex_1().into_any_element();
    };
    let close = div()
        .id("capsule/close")
        .control(Role::Button, "Close")
        .w(px(44.))
        .h(px(34.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path("icons/x.svg")
                .size(px(20.))
                .text_color(palette.foreground()),
        )
        .on_click(cx.listener(|root, _, _, cx| root.close(cx)));
    let bar = div()
        .h(px(48.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(2.))
        .pl(px(4.))
        .pr(px(8.))
        .border_b_1()
        .border_color(palette.border())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(4.))
                .child(glyph(&row, 24., 15., palette))
                .child(
                    look::text(look::BODY, FontWeight::MEDIUM, palette.foreground())
                        .truncate()
                        .child(name_of(&row)),
                ),
        )
        .child(
            div()
                .flex_none()
                .h(px(36.))
                .border_1()
                .border_color(palette.border_strong())
                .child(close),
        );
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(bar)
        .child(opening(&row, palette))
        .into_any_element()
}

/// The tab bar (Expo `TabBar`): 56 over the bottom inset, icon 22 over an 11
/// label; the chosen tab in the foreground with a 2 rule over it.
pub fn tab_bar(on: Tab, inset: Pixels, palette: Palette, cx: &mut Context<Root>) -> Div {
    let tab = |tab: Tab, id: &'static str, label: &'static str, icon: &'static str| {
        let chosen = tab == on;
        let tone = if chosen {
            palette.foreground()
        } else {
            palette.muted()
        };
        let weight = if chosen {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        };
        let mut item = div()
            .id(id)
            .control(Role::Tab, label)
            .aria_selected(chosen)
            .flex_1()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(3.))
            .child(svg().path(icon).size(px(look::ICON_HIT)).text_color(tone))
            .child(look::text(look::TINY, weight, tone).child(label))
            .on_click(cx.listener(move |root, _, _, cx| root.pick(tab, cx)));
        if chosen {
            item = item.child(
                div()
                    .absolute()
                    .top(px(-1.))
                    .left(px(16.))
                    .right(px(16.))
                    .h(px(2.))
                    .bg(palette.foreground()),
            );
        }
        item
    };
    div()
        .flex_none()
        .h(px(look::TAB_BAR) + inset)
        .pb(inset)
        .flex()
        .border_t_1()
        .border_color(palette.border())
        .bg(palette.background())
        .child(tab(Tab::Home, "tab/home", "Home", "icons/square.svg"))
        .child(tab(Tab::Open, "tab/open", "Open", "icons/archive.svg"))
        .child(tab(
            Tab::Account,
            "tab/account",
            "Account",
            "icons/circle-user.svg",
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &str, note: Option<&'static str>) -> RailRow {
        RailRow {
            program: "chat",
            label: label.into(),
            icon: String::new(),
            note,
            empty: false,
        }
    }

    /// A tile names its program by its manifest once the view is in, by its
    /// id with a capital until then.
    #[test]
    fn a_tile_names_its_program_by_manifest_else_by_id() {
        assert_eq!(name_of(&row("Chat rooms", None)), "Chat rooms");
        assert_eq!(name_of(&row("forge-view", Some("Loading"))), "Forge view");
    }
}
