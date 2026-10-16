//! The tab roots: Home (the network's programs, IA B, wasm-mobile SPEC
//! §7.2), Open (the programs open, where they were left), and the tab bar
//! under them. Drawn in the Expo kit's measures ([`look`]); the words are
//! the shell boards' (`design/canvas/shell`), English as the Expo app's.

use super::Root;
use super::look::{self, Palette};
use super::stack::{Opened, Tab};
use ducktape_host::a11y::Control as _;
use ducktape_host::runtime::RailRow;
use ducktape_host::session::Shared;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{
    App, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Role, SharedString, StatefulInteractiveElement as _, Styled as _, div, px, relative,
    svg,
};
use std::collections::BTreeMap;
use std::time::Duration;

/// Home's title: the store's shape, never the store's word (SPEC §7.2).
pub const HOME_TITLE: &str = "Programs on this network";

/// A program's name before its view's manifest names it: its id with a
/// capital ("chat" -> "Chat"), as the desktop's bar shows it.
pub fn name_of(row: &RailRow) -> String {
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
pub fn listed(shared: &Shared, cx: &App) -> Vec<RailRow> {
    shared
        .rail
        .read(cx)
        .rows()
        .iter()
        .filter(|row| !row.empty)
        .cloned()
        .collect()
}

/// `program`'s row, while the network lists it.
pub fn row_of(shared: &Shared, program: &str, cx: &App) -> Option<RailRow> {
    listed(shared, cx)
        .into_iter()
        .find(|row| row.program == program)
}

/// The person's name and account number, once the account is found.
pub fn me(shared: &Shared, cx: &App) -> Option<(u64, String)> {
    shared.account.read(cx).get().account.clone().flatten()
}

/// A name's initial, as the Expo `Avatar` draws it.
pub fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default()
}

/// An avatar (Expo `Avatar`): `name`'s initial on `surface_raised`, round,
/// at 32 (13 text) or 40 (16 text).
pub fn avatar(name: &str, side: f32, palette: Palette) -> Div {
    let text = if side >= 40. { 16. } else { 13. };
    div()
        .size(px(side))
        .flex_none()
        .rounded_full()
        .bg(palette.surface_raised())
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .font_family(ducktape_host::fonts::FAMILY_UI)
                .text_size(px(text))
                .line_height(px(text * 1.25))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.muted())
                .child(initial(name)),
        )
}

/// The 56 header of a tab root (Expo `Header` with `root`): the reader's
/// avatar in a 44 hit, which opens the account sheet; the 22 title, a 12
/// caption under it.
fn root_header(
    title: &'static str,
    caption: String,
    name: &str,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Div {
    let avatar = div()
        .id("header/account")
        .control(Role::Button, "Account")
        .size(px(look::HIT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(avatar(name, look::AVATAR, palette))
        .on_click(cx.listener(|root, _, _, cx| root.account_sheet(cx)));
    let words = div()
        .flex_1()
        .min_w_0()
        .px(px(look::gap::SM))
        .flex()
        .flex_col()
        .justify_center()
        .child(
            look::text(look::BIG_TITLE, FontWeight::MEDIUM, palette.foreground())
                .truncate()
                .child(title),
        )
        .child(
            look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                .truncate()
                .child(caption),
        );
    div()
        .h(px(look::HEADER))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(look::gap::XXS))
        .px(px(look::gap::SM))
        .child(avatar)
        .child(words)
}

/// The caption under a root's title: the network in hand and the reader's
/// name ("testkit · Dev"), or where the phone is trying to get to.
fn caption(shared: &Shared, cx: &App) -> String {
    let session = shared.session.read(cx).get();
    if session.connected && session.reconnecting {
        format!("{} · reconnecting", session.network)
    } else if session.connected {
        match me(shared, cx) {
            Some((_, name)) if !name.is_empty() => format!("{} · {name}", session.network),
            _ => session.network.clone(),
        }
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
        .pt(px(look::gap::LG))
        .pb(px(look::gap::XXS))
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
pub fn glyph(row: &RailRow, side: f32, icon: f32, palette: Palette) -> Div {
    let tile = div()
        .size(px(side))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .bg(palette.surface_raised());
    if row.icon.is_empty() {
        let initial = initial(&name_of(row));
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

/// Whether `row` answers the filter `query` (lower case): its name or its
/// id holds it.
fn answers(row: &RailRow, query: &str) -> bool {
    query.is_empty()
        || name_of(row).to_lowercase().contains(query)
        || row.program.contains(query)
}

/// The filter field (Expo `Field` with a search lead): a 44 square box, a
/// 1 `border_strong` edge, 2 in the foreground while it has the keys.
fn search(field: &Entity<InputState>, focused: bool, palette: Palette) -> Div {
    let (edge, thick) = if focused {
        (palette.foreground(), 2.)
    } else {
        (palette.border_strong(), 1.)
    };
    div().px(px(look::GUTTER)).child(
        div()
            .h(px(look::CONTROL))
            .flex()
            .items_center()
            .gap(px(look::gap::SM))
            .px(px(look::gap::LG - (thick - 1.)))
            .border(px(thick))
            .border_color(edge)
            .bg(palette.background())
            .child(
                svg()
                    .path("icons/search.svg")
                    .size(px(look::ICON_SHEET))
                    .flex_none()
                    .text_color(palette.muted()),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(field)
                        .appearance(false)
                        .text_size(px(look::BODY.size()))
                        .py_0()
                        .px_0(),
                ),
            ),
    )
}

/// Home: the network's programs that answer the filter, three across; a
/// tile opens its program; one a view asked about shows a dot (never a
/// count, Expo `Dot`).
pub fn home(
    shared: &Shared,
    field: &Entity<InputState>,
    searching: bool,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Div {
    let query = field.read(cx).value().trim().to_lowercase();
    let rows: Vec<RailRow> = listed(shared, cx)
        .into_iter()
        .filter(|row| answers(row, &query))
        .collect();
    let badges: BTreeMap<&'static str, i64> = shared.rail.read(cx).badges().clone();
    let tiles = rows.iter().map(|row| {
        let program = row.program;
        let name = name_of(row);
        let mut face = glyph(row, 56., 28., palette).relative();
        if badges.get(program).is_some_and(|count| *count > 0) {
            face = face.child(
                dot(palette)
                    .absolute()
                    .top(px(-look::gap::XXS))
                    .right(px(-look::gap::XXS)),
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
            .gap(px(look::gap::SM))
            .pt(px(look::gap::MD))
            .pb(px(look::gap::LG))
            .px(px(look::gap::XXS))
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
                    .mt(px(-look::gap::XXS))
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
        .child(search(field, searching, palette))
        .child(section_head("Programs", rows.len(), palette))
        .child(
            div()
                .flex()
                .flex_wrap()
                .px(px(look::GUTTER - look::gap::XXS))
                .py(px(look::gap::SM))
                .children(tiles),
        );
    let name = me(shared, cx).map(|(_, name)| name).unwrap_or_default();
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(root_header(HOME_TITLE, caption(shared, cx), &name, palette, cx))
        .child(body)
}

/// The unread dot (Expo `Dot`): 8, the accent.
fn dot(palette: Palette) -> Div {
    div()
        .size(px(8.))
        .flex_none()
        .rounded_full()
        .bg(palette.accent())
}

/// How long ago `then` was, as a row's caption says it.
pub fn ago(since: Duration) -> String {
    let minutes = since.as_secs() / 60;
    match minutes {
        0 => "now".to_owned(),
        1..60 => format!("{minutes}m ago"),
        60..1440 => format!("{}h ago", minutes / 60),
        1440..2880 => "yesterday".to_owned(),
        _ => format!("{}d ago", minutes / 1440),
    }
}

/// Open: the programs open, the latest first, each with the title its view
/// gave its window and when it was last on screen; a press goes back to
/// where it was left, the × lets it go (it stays on Home).
pub fn open(
    shared: &Shared,
    opened: &[Opened],
    titles: &BTreeMap<u64, String>,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Div {
    let name = me(shared, cx).map(|(_, name)| name).unwrap_or_default();
    let header = root_header("Open", caption(shared, cx), &name, palette, cx);
    if opened.is_empty() {
        return div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(empty(
                "icons/archive.svg",
                "Nothing open",
                "A program you open stays here, where you left it.",
                palette,
            ));
    }
    let badges = shared.rail.read(cx).badges().clone();
    let now = cx.background_executor().now();
    let rows = opened.iter().map(|opened| {
        let program = opened.program;
        let row = row_of(shared, program, cx);
        let name = row.as_ref().map_or_else(|| program.to_owned(), name_of);
        let when = opened.left.map_or_else(|| "now".to_owned(), |left| ago(now - left));
        let caption = match titles.get(&opened.instance) {
            Some(title) if !title.is_empty() => format!("{title} · {when}"),
            _ => when,
        };
        let icon = row
            .as_ref()
            .map(|row| row.icon.clone())
            .filter(|icon| !icon.is_empty());
        let mut line = div()
            .id(SharedString::from(format!("open/{program}")))
            .control(Role::Button, name.clone())
            .min_h(px(56.))
            .flex()
            .items_center()
            .gap(px(look::gap::LG))
            .pl(px(look::GUTTER))
            .border_b_1()
            .border_color(palette.border())
            .child(match icon {
                Some(icon) => svg()
                    .path(SharedString::from(icon))
                    .size(px(look::ICON_HIT))
                    .flex_none()
                    .text_color(palette.foreground())
                    .into_any_element(),
                None => div().size(px(look::ICON_HIT)).flex_none().into_any_element(),
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .py(px(look::gap::MD))
                    .flex()
                    .flex_col()
                    .child(
                        look::text(look::BODY, FontWeight::NORMAL, palette.foreground())
                            .truncate()
                            .child(name.clone()),
                    )
                    .child(
                        look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                            .truncate()
                            .child(caption),
                    ),
            )
            .on_click(cx.listener(move |root, _, _, cx| root.open(program, cx)));
        if badges.get(program).is_some_and(|count| *count > 0) {
            line = line.child(dot(palette));
        }
        line.child(
            div()
                .id(SharedString::from(format!("open/{program}/close")))
                .control(Role::Button, format!("Close {name}"))
                .size(px(look::HIT))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path("icons/x.svg")
                        .size(px(look::ICON_SHEET))
                        .text_color(palette.muted()),
                )
                .on_click(cx.listener(move |root, _, _, cx| {
                    cx.stop_propagation();
                    root.close_program(program, cx)
                })),
        )
    });
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(header)
        .child(
            div()
                .id("open")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(rows)
                .child(
                    look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted())
                        .px(px(look::GUTTER + 4.))
                        .py(px(look::gap::LG))
                        .child("A closed program stays on Home."),
                ),
        )
}

/// Nothing to show yet (Expo `EmptyState`): a 28 faint icon, what is
/// missing (15 medium), what would fill it (13 muted).
fn empty(icon: &'static str, title: &str, body: &str, palette: Palette) -> Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(look::gap::SM))
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
pub fn opening(row: &RailRow, palette: Palette) -> Div {
    let mut words = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(look::gap::XXS))
        .child(look::text(look::BODY, FontWeight::MEDIUM, palette.foreground()).child(name_of(row)));
    if let Some(stage) = row.note {
        words = words
            .child(look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted()).child(stage));
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(look::gap::LG))
        .child(glyph(row, 64., 32., palette))
        .child(words)
}

/// The tab bar (Expo `TabBar`): 56 over the bottom inset, icon 22 over an 11
/// label; the chosen tab in the foreground with a 2 rule over it. Account
/// opens the account sheet over the tab in hand; it is never a root.
pub fn tab_bar(on: Tab, inset: Pixels, palette: Palette, cx: &mut Context<Root>) -> Div {
    let item = |id: &'static str, label: &'static str, icon: &'static str, chosen: bool| {
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
            .child(look::text(look::TINY, weight, tone).child(label));
        if chosen {
            item = item.child(
                div()
                    .absolute()
                    .top(px(-1.))
                    .left(px(look::GUTTER))
                    .right(px(look::GUTTER))
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
        .child(
            item("tab/home", "Home", "icons/square.svg", on == Tab::Home)
                .on_click(cx.listener(|root, _, _, cx| root.pick(Tab::Home, cx))),
        )
        .child(
            item("tab/open", "Open", "icons/archive.svg", on == Tab::Open)
                .on_click(cx.listener(|root, _, _, cx| root.pick(Tab::Open, cx))),
        )
        .child(
            item("tab/account", "Account", "icons/circle-user.svg", false)
                .on_click(cx.listener(|root, _, _, cx| root.account_sheet(cx))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &str, note: Option<&'static str>) -> RailRow {
        RailRow {
            program: "chat",
            label: label.into(),
            icon: String::new(),
            description: String::new(),
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

    /// The filter reads a name or an id, in any case.
    #[test]
    fn the_filter_reads_names_and_ids() {
        assert!(answers(&row("Chat rooms", None), ""));
        assert!(answers(&row("Chat rooms", None), "rooms"));
        assert!(answers(&row("Rooms", None), "chat"));
        assert!(!answers(&row("Rooms", None), "forge"));
    }

    /// A row says how long ago its program left the screen.
    #[test]
    fn ago_says_minutes_hours_then_days() {
        let minutes = |m: u64| ago(Duration::from_secs(m * 60));
        assert_eq!(minutes(0), "now");
        assert_eq!(minutes(4), "4m ago");
        assert_eq!(minutes(130), "2h ago");
        assert_eq!(minutes(1500), "yesterday");
        assert_eq!(minutes(3 * 1440), "3d ago");
    }
}
