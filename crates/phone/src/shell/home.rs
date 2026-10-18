//! The tab roots: Home (the network's programs, IA B, wasm-mobile SPEC
//! §7.2: a tinted grid, four across, the ones with news first), Open (the
//! programs open, where they were left), Notifications (the notices the
//! programs posted, as the host's centre logs them), and the tab bar under
//! them (the Account tab is `account`'s). Drawn in the Expo kit's measures
//! ([`look`]); the words are the shell boards' (`design/canvas/shell`),
//! English as the Expo app's.

use super::Root;
use super::debug::Readout;
use super::look::{self, Palette};
use super::stack::{Opened, Tab};
use ducktape_host::a11y::Control as _;
use ducktape_host::runtime::notify::{Entry, wall};
use ducktape_host::runtime::{Prep, RailRow};
use ducktape_host::session::Shared;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{
    AnimationExt as _, AnyElement, App, Context, Div, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Role, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, div, px, relative, svg,
};
use std::collections::BTreeMap;
use std::time::Duration;

/// The label over Home's grid of every program: the store's shape, never
/// the store's word (SPEC §7.2, App Store 4.7), on screen whenever Home is.
pub const ON_THIS_NETWORK: &str = "Programs on this network";

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

/// Every program the network lists with a view, or still on its way, in
/// the roster's order (the desktop rail's), none privileged: what opens
/// (Home's grid leaves out the ones still on their way).
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
    shared
        .rail
        .read(cx)
        .rows()
        .iter()
        .find(|row| !row.empty && row.program == program)
        .cloned()
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

/// The 56 header of a tab root (Expo `Header` with `root`): the 22 title,
/// a 12 caption under it; what the root adds (Home's filter) at the right.
/// No avatar: who is reading is the Account tab's.
fn root_header(title: String, caption: String, palette: Palette) -> Div {
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
        .child(words)
}

/// The caption under a root's title: `on` while on the network, else
/// where the phone is trying to get to.
pub(super) fn caption(shared: &Shared, on: String, cx: &App) -> String {
    let session = shared.session.read(cx).get();
    if session.connected && session.reconnecting {
        format!("{} · reconnecting", session.network)
    } else if session.connected {
        on
    } else if session.connecting {
        format!("Connecting to {}…", session.reaching)
    } else if !session.error.is_empty() {
        session.error.clone()
    } else {
        "Not connected".to_owned()
    }
}

/// A section's head (Expo `SectionHead`): 12 medium muted label.
fn section_head(label: &'static str, palette: Palette) -> Div {
    div()
        .min_h(px(40.))
        .flex()
        .items_center()
        .pt(px(look::gap::LG))
        .pb(px(look::gap::XXS))
        .px(px(look::GUTTER))
        .child(look::text(look::CAPTION, FontWeight::MEDIUM, palette.muted()).child(label))
}

/// A program's glyph: the icon its view's manifest declares, or its name's
/// initial when it declares none, in the ink of the tint the manifest names
/// on that tint's ground (SPEC §7.7; no tint: the neutral filled tile).
pub fn glyph(row: &RailRow, side: f32, icon: f32, palette: Palette) -> Div {
    let (ground, ink) = palette.tint(row.tint);
    let tile = div()
        .size(px(side))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .bg(ground);
    if row.icon.is_empty() {
        let initial = initial(&name_of(row));
        tile.child(look::text(look::BODY, FontWeight::MEDIUM, ink).child(initial))
    } else {
        tile.child(
            svg()
                .path(SharedString::from(row.icon.clone()))
                .size(px(icon))
                .text_color(ink),
        )
    }
}

/// Whether `row` answers the filter `query` (lower case): its name or its
/// id holds it.
fn answers(row: &RailRow, query: &str) -> bool {
    query.is_empty() || name_of(row).to_lowercase().contains(query) || row.program.contains(query)
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
                    .size(px(look::ICON_ROW))
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

/// The programs Home's caption counts while they are on their way: the
/// ones not settled when the run begins, until every one of them is. The
/// host reads every one's bytes before it prepares any: while some are still
/// being read the caption counts those read (`Loading 7/18`: on a first join
/// every program the roster lists, those that ship no view too, since
/// whether one does is known only once its bytes are here), then the ones
/// with a view as they are prepared (`Preparing 3/11 · Forge`). Later, the
/// ones the network moved to new code. The count never grows: a program
/// that starts while a run is on waits for the next run.
#[derive(Default)]
pub struct Run(Vec<&'static str>);

impl Run {
    /// The run once the rail reads `rows`: over when none it counts is
    /// being prepared, and then the next one begins with those that are.
    pub fn follow(&mut self, rows: &[RailRow]) {
        if !self.0.iter().any(|program| preparing(rows, program)) {
            self.0 = rows
                .iter()
                .filter(|row| row.prep != Prep::Settled)
                .map(|row| row.program)
                .collect();
        }
    }

    /// The caption while a run is on: `Loading 7/18` while some of its
    /// programs' bytes are being read (the place of the next one to come
    /// among them), then `Preparing 3/11 · Forge` (the place of the one in
    /// hand among those with a view, the settled ones before it, and that
    /// one's name while a loader is at it).
    pub fn caption(&self, rows: &[RailRow]) -> Option<String> {
        let run: Vec<&RailRow> = (rows.iter())
            .filter(|row| self.0.contains(&row.program))
            .collect();
        if run
            .iter()
            .any(|row| row.pending || row.prep == Prep::Reading)
        {
            let read = run.iter().filter(|row| row.prep != Prep::Reading).count();
            let total = run.len();
            return Some(format!("Loading {}/{total}", (read + 1).min(total)));
        }
        let views: Vec<&RailRow> = run.into_iter().filter(|row| !row.empty).collect();
        let left = views.iter().filter(|row| row.prep != Prep::Settled).count();
        if left == 0 {
            return None;
        }
        let (total, place) = (views.len(), views.len() - left + 1);
        Some(match views.iter().find(|row| row.prep == Prep::Working) {
            Some(row) => format!("Preparing {place}/{total} · {}", name_of(row)),
            None => format!("Preparing {place}/{total}"),
        })
    }
}

/// Whether `program`'s row in `rows` is being prepared.
fn preparing(rows: &[RailRow], program: &str) -> bool {
    rows.iter()
        .any(|row| row.program == program && row.prep != Prep::Settled)
}

/// The ring around a tile's glyph while a loader prepares its program (the
/// shell boards' opening spinner, `.opening .gl .spin`): a 2 line in
/// `border` just outside the 60 square, sharp, with one side's length of it
/// in the foreground running round once in [`RING_LAP`]; held still on the
/// top side when the system asks for less motion. It paces itself at 30 fps
/// (the desktop's status dot does too) and is drawn only on the tile being
/// prepared, so nothing redraws for it while none is. `id` is its selector,
/// `{id}/run` its running part's.
fn ring(id: String, moving: bool, palette: Palette) -> AnyElement {
    let side = 60. + 2. * RING;
    let run = format!("{id}/run");
    let piece = move |[left, top, width, height]: [f32; 4]| {
        let run = run.clone();
        div()
            .debug_selector(move || run)
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(height))
            .bg(palette.foreground())
    };
    let frame = div()
        .debug_selector(move || id)
        .absolute()
        .top(px(-RING))
        .left(px(-RING))
        .size(px(side))
        .child(
            div()
                .absolute()
                .inset_0()
                .border(px(RING))
                .border_color(palette.border()),
        );
    match moving {
        false => frame
            .children(segment(0., side).map(piece))
            .into_any_element(),
        true => frame
            .with_animation(
                "ring",
                gpui_kit::Animation::new(RING_LAP)
                    .repeat()
                    .with_max_fps(30.),
                move |frame, at| frame.children(segment(at, side).map(&piece)),
            )
            .into_any_element(),
    }
}

/// The ring's line.
const RING: f32 = 2.;
/// Once round the ring.
const RING_LAP: Duration = Duration::from_millis(1500);

/// The ring's running part, `at` (0..1) of the way round its `side` square
/// clockwise from the top left corner: one side's length, [`RING`] thick,
/// in a piece for each side it lies on (left, top, width, height).
fn segment(at: f32, side: f32) -> impl Iterator<Item = [f32; 4]> {
    let round = 4. * side;
    let head = at.rem_euclid(1.) * round;
    (0..4u8).flat_map(move |edge| {
        let start = f32::from(edge) * side;
        // the part before the top left corner, and the part past it
        [head, head - round].into_iter().filter_map(move |from| {
            let (a, b) = ((from - start).max(0.), (from + side - start).min(side));
            (b > a).then_some(match edge {
                0 => [a, 0., b - a, RING],
                1 => [side - RING, a, RING, b - a],
                2 => [side - b, side - RING, b - a, RING],
                _ => [0., side - b, RING, b - a],
            })
        })
    })
}

/// Home's two sections of `rows`: New, the programs with news (a view
/// asked about it: a dot) in `rows`' order, empty while none has news; and
/// the grid, every program of `rows` in its order, those with news too:
/// New repeats a tile above the grid, never moves it.
fn sections<'a>(
    rows: &'a [RailRow],
    badges: &BTreeMap<&'static str, i64>,
) -> (Vec<&'a RailRow>, Vec<&'a RailRow>) {
    let new = rows
        .iter()
        .filter(|row| badges.get(row.program).is_some_and(|count| *count > 0))
        .collect();
    (new, rows.iter().collect())
}

/// One program's tile in Home's grid, four across: its glyph on its tint's
/// ground, a dot when it has news (never a count, Expo `Dot`), the name
/// under it; while it is on its way, the glyph dimmed and no word, in a
/// running [`ring`] while a loader prepares it; a failure says so under the
/// name. `section` keeps
/// the New section's copy apart from the grid's (`new/chat`, `home/chat`).
fn tile(
    row: &RailRow,
    section: &'static str,
    news: bool,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Stateful<Div> {
    let program = row.program;
    let name = name_of(row);
    let prepared = row.prep == Prep::Settled;
    let mut face = glyph(row, 60., 30., palette);
    if !prepared {
        face = div().child(face.opacity(look::PREPARING));
    }
    if row.prep == Prep::Working {
        let id = format!("{section}/{program}/ring");
        face = face.child(ring(id, !cx.reduce_motion(), palette));
    }
    let mut face = face.relative();
    if news {
        // the board's dot: 10 in a 2 ring of the ground, at -5 / -5
        face = face.child(
            dot(palette)
                .size(px(14.))
                .border_2()
                .border_color(palette.background())
                .absolute()
                .top(px(-5.))
                .right(px(-5.)),
        );
    }
    let mut tile = div()
        .id(SharedString::from(format!("{section}/{program}")))
        .debug_selector(move || format!("{section}/{program}"))
        .control(Role::Button, name.clone())
        .w(relative(1. / 4.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(look::gap::SM))
        .pt(px(look::gap::SM))
        .pb(px(look::gap::MD))
        .px(px(look::gap::XXS))
        .active(|style| style.opacity(look::PRESSED))
        .child(face)
        .child(
            look::text(look::SECONDARY, FontWeight::NORMAL, palette.foreground())
                .max_w_full()
                .truncate()
                .child(name),
        )
        .on_click(cx.listener(move |root, _, window, cx| {
            window.blur(cx);
            root.open(program, cx);
        }));
    if let Some(stage) = row.note.filter(|_| prepared) {
        tile = tile.child(
            look::text(look::TINY, FontWeight::NORMAL, palette.muted())
                .debug_selector(move || format!("{section}/{program}/note"))
                .mt(px(-look::gap::XS))
                .child(stage),
        );
    }
    tile
}

/// Home: the network's name over who reads and how many programs it
/// lists, a magnifier that opens the filter; then, while some program has
/// news, those programs' tiles under "New"; then every program the filter
/// answers in the roster's order, four across, under "Programs on this
/// network". New repeats a tile, it never moves one, so the grid's places
/// stay put. A tile comes once its program's code is read, and the host
/// shows every program a roster read read at once, so they come together;
/// while none is there and programs are still on their way, a `Loading` line
/// stands where the tiles go. While a `run` is on, the caption counts it.
pub fn home(
    shared: &Shared,
    run: &Run,
    field: &Entity<InputState>,
    filtering: bool,
    searching: bool,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Div {
    let query = field.read(cx).value().trim().to_lowercase();
    let counting = run.caption(shared.rail.read(cx).rows());
    let all = listed(shared, cx);
    let loading = all.is_empty() || all.iter().any(|row| row.pending);
    // a program still on its way may ship no view: not a tile until its
    // code is read
    let shown: Vec<RailRow> = all.into_iter().filter(|row| !row.pending).collect();
    let count = shown.len();
    let rows: Vec<RailRow> = shown
        .into_iter()
        .filter(|row| answers(row, &query))
        .collect();
    let badges: BTreeMap<&'static str, i64> = shared.rail.read(cx).badges().clone();
    let grid = |rows: &[&RailRow], section: &'static str, cx: &mut Context<Root>| {
        let tiles: Vec<_> = rows
            .iter()
            .map(|row| {
                let news = badges.get(row.program).is_some_and(|count| *count > 0);
                tile(row, section, news, palette, cx)
            })
            .collect();
        div()
            .flex()
            .flex_wrap()
            .px(px(look::gap::SM))
            .py(px(look::gap::LG))
            .children(tiles)
    };
    let (new, every) = sections(&rows, &badges);
    let mut body = div().id("home").flex_1().min_h_0().overflow_y_scroll();
    if filtering {
        body = body.child(search(field, searching, palette));
    }
    if !new.is_empty() {
        body = body
            .child(section_head("New", palette))
            .child(grid(&new, "new", cx));
    }
    body = body.child(section_head(ON_THIS_NETWORK, palette));
    let body = if !rows.is_empty() || !loading {
        body.child(grid(&every, "home", cx))
    } else {
        body.child(
            look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted())
                .px(px(look::GUTTER))
                .py(px(look::gap::LG))
                .child("Loading"),
        )
    };
    let (title, on) = {
        let network = shared.session.read(cx).get().network.clone();
        let programs = match count {
            1 => "1 program".to_owned(),
            count => format!("{count} programs"),
        };
        let on = match me(shared, cx) {
            Some((_, name)) if !name.is_empty() => format!("{name} · {programs}"),
            _ => programs,
        };
        (network, counting.unwrap_or(on))
    };
    let (icon, label) = match filtering {
        true => ("icons/x.svg", "Close the filter"),
        false => ("icons/search.svg", "Filter by name"),
    };
    let filter = div()
        .id("home/filter")
        .control(Role::Button, label)
        .size(px(look::HIT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .active(|style| style.opacity(look::PRESSED))
        .child(
            svg()
                .path(icon)
                .size(px(look::ICON_HIT))
                .text_color(palette.foreground()),
        )
        .on_click(cx.listener(move |root, _, window, cx| root.filter(!filtering, window, cx)));
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(root_header(title, caption(shared, on, cx), palette).child(filter))
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
/// where it was left, the × lets it go (it stays on Home). In debug mode
/// (`readout`) each says what it costs under its name, and the app's own
/// memory heads the list.
pub fn open(
    shared: &Shared,
    opened: &[Opened],
    titles: &BTreeMap<u64, String>,
    readout: Option<&Readout>,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Div {
    let mono = |text: String| {
        look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
            .font_family(ducktape_host::fonts::FAMILY_MONO)
            .truncate()
            .child(text)
    };
    // the network in hand and the reader's name ("garage · phone test")
    let on = {
        let network = shared.session.read(cx).get().network.clone();
        match me(shared, cx) {
            Some((_, name)) if !name.is_empty() => format!("{network} · {name}"),
            _ => network,
        }
    };
    let header = root_header("Open".into(), caption(shared, on, cx), palette);
    // the whole app, over the list (debug mode)
    let app = readout.map(|readout| {
        div()
            .id("open/app")
            .debug_selector(|| "open/app".to_owned())
            .px(px(look::GUTTER))
            .py(px(look::gap::SM))
            .border_b_1()
            .border_color(palette.border())
            .child(mono(readout.app()))
    });
    if opened.is_empty() {
        return div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .children(app)
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
        let when = opened
            .left
            .map_or_else(|| "now".to_owned(), |left| ago(now - left));
        let caption = match titles.get(&opened.instance) {
            Some(title) if !title.is_empty() => format!("{title} · {when}"),
            _ => when,
        };
        let icon = row
            .as_ref()
            .map(|row| row.icon.clone())
            .filter(|icon| !icon.is_empty());
        let cost = readout.map(|readout| {
            let seat = shared.seats.read(cx).seat(opened.instance);
            let memory = seat.and_then(|seat| seat.read(cx).memory_bytes());
            let id = format!("open/{program}/cost");
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .pt(px(look::gap::XXS))
                .children(
                    readout
                        .cost(program, opened.instance, memory)
                        .lines()
                        .map(mono),
                )
        });
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
            .active(|style| style.opacity(look::PRESSED))
            .child(match icon {
                Some(icon) => svg()
                    .path(SharedString::from(icon))
                    .size(px(look::ICON_HIT))
                    .flex_none()
                    .text_color(palette.foreground())
                    .into_any_element(),
                None => div()
                    .size(px(look::ICON_HIT))
                    .flex_none()
                    .into_any_element(),
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
                    )
                    .children(cost),
            )
            .on_click(cx.listener(move |root, _, window, cx| {
                window.blur(cx);
                root.open(program, cx);
            }));
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
                .children(app)
                .children(rows)
                .child(
                    look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted())
                        .px(px(look::GUTTER + 4.))
                        .py(px(look::gap::LG))
                        .child("A closed program stays on Home."),
                ),
        )
}

/// Notifications: the notices the programs posted, as the host's centre
/// logs them (the desktop bell's list), newest first, each under its
/// program's name and how long ago it came, an unread one in medium with a
/// dot; a press reads it and opens what it is about as a tapped system
/// notice does ([`Root::open_notice`]). "Mark all read" at the header's
/// right while any is unread, "Clear read" under the rows while any is
/// read. Which notices there are, their words, their grouping and their
/// links are each program's own; this list carries them and decides none
/// of it.
pub fn notifications(shared: &Shared, palette: Palette, cx: &mut Context<Root>) -> Div {
    let (entries, unread) = {
        let notifications = shared.notifications.read(cx);
        (notifications.entries(), notifications.unread())
    };
    let on = {
        let network = shared.session.read(cx).get().network.clone();
        match unread {
            0 => network,
            unread => format!("{network} · {unread} unread"),
        }
    };
    let mut header = root_header("Notifications".into(), caption(shared, on, cx), palette);
    if unread > 0 {
        // Expo `HitText`: 44 tall, the words in medium
        header = header.child(
            div()
                .id("notifications/mark-all")
                .control(Role::Button, "Mark all read")
                .debug_selector(|| "notifications/mark-all".to_owned())
                .h(px(look::HIT))
                .flex_none()
                .px(px(look::gap::SM))
                .flex()
                .items_center()
                .active(|style| style.opacity(look::PRESSED))
                .child(
                    look::text(look::BODY, FontWeight::MEDIUM, palette.foreground())
                        .child("Mark all read"),
                )
                .on_click(cx.listener(|root, _, _, cx| root.mark_all_read(cx))),
        );
    }
    let screen = div().flex_1().min_h_0().flex().flex_col().child(header);
    if entries.is_empty() {
        return screen.child(empty(
            "icons/bell.svg",
            "You\u{2019}re all caught up",
            "Notices from your programs land here.",
            palette,
        ));
    }
    let now = wall();
    // the log keeps every row until it is cleared: read ones go here, as
    // the desktop bell's footer clears them
    let clear = entries.iter().any(|entry| entry.read).then(|| {
        div()
            .id("notifications/clear-read")
            .control(Role::Button, "Clear read")
            .debug_selector(|| "notifications/clear-read".to_owned())
            .h(px(look::HIT))
            .px(px(look::GUTTER))
            .flex()
            .items_center()
            .active(|style| style.opacity(look::PRESSED))
            .child(
                look::text(look::SECONDARY, FontWeight::MEDIUM, palette.muted())
                    .child("Clear read"),
            )
            .on_click(cx.listener(|root, _, _, cx| root.clear_read(cx)))
    });
    let rows: Vec<_> = entries
        .into_iter()
        .map(|entry| notice(shared, entry, now, palette, cx))
        .collect();
    screen.child(
        div()
            .id("notifications")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .children(clear),
    )
}

/// One notice's row (Expo `Row`, 56 and up): its program's glyph, the
/// program's name and how long ago over the notice's title and its words
/// (two lines at most); unread, the title in medium and a dot at the right.
fn notice(
    shared: &Shared,
    entry: Entry,
    now: i64,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Stateful<Div> {
    let row = row_of(shared, &entry.program, cx);
    let name = row.as_ref().map_or_else(|| entry.program.clone(), name_of);
    let lead = match &row {
        Some(row) => glyph(row, 32., look::ICON_ROW, palette),
        None => avatar(&name, 32., palette),
    };
    let when = ago(Duration::from_secs(
        u64::try_from(now - entry.at).unwrap_or(0),
    ));
    let (weight, ink) = match entry.read {
        true => (FontWeight::NORMAL, palette.muted()),
        false => (FontWeight::MEDIUM, palette.foreground()),
    };
    let said = format!(
        "{}{name}: {}",
        if entry.read { "" } else { "Unread. " },
        entry.title
    );
    let body = entry.body.replace('\n', " ");
    let id = entry.id;
    let selector = format!("notifications/{id}");
    let words = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(1.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(look::gap::SM))
                .child(
                    look::text(look::CAPTION, FontWeight::MEDIUM, palette.muted())
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(name),
                )
                .child(
                    look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                        .flex_none()
                        .child(when),
                ),
        )
        .child(
            look::text(look::BODY, weight, ink)
                .truncate()
                .child(entry.title),
        )
        .children((!body.is_empty()).then(|| {
            look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted())
                .line_clamp(2)
                .child(body)
        }));
    div()
        .id(SharedString::from(selector.clone()))
        .control(Role::Button, said)
        .debug_selector(move || selector.clone())
        .min_h(px(look::ROW_TALL))
        .flex()
        .items_center()
        .gap(px(look::gap::LG))
        .py(px(look::gap::MD))
        .px(px(look::GUTTER))
        .border_b_1()
        .border_color(palette.border())
        .active(|style| style.opacity(look::PRESSED))
        .child(lead)
        .child(words)
        .children((!entry.read).then(|| dot(palette)))
        .on_click(cx.listener(move |root, _, window, cx| {
            window.blur(cx);
            root.open_notice(id, cx);
        }))
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
        .child(
            look::text(look::BODY, FontWeight::MEDIUM, palette.foreground()).child(name_of(row)),
        );
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
/// label; the chosen tab in the foreground with a 2 rule over it. Each item
/// picks its tab's root. Notifications carries a dot while a notice is
/// unread (Expo's tab dot: 8 in a 2 ring of the ground, never a count), and
/// says how many to a screen reader as the desktop's bell does.
pub fn tab_bar(
    on: Tab,
    unread: usize,
    inset: Pixels,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Div {
    let tabs = [
        (Tab::Home, "tab/home", "Home", "icons/house.svg"),
        (Tab::Open, "tab/open", "Open", "icons/archive.svg"),
        (
            Tab::Notifications,
            "tab/notifications",
            "Notifications",
            "icons/bell.svg",
        ),
        (
            Tab::Account,
            "tab/account",
            "Account",
            "icons/circle-user.svg",
        ),
    ];
    let items = tabs.map(|(tab, id, label, icon)| {
        let chosen = on == tab;
        let (tone, weight) = match chosen {
            true => (palette.foreground(), FontWeight::MEDIUM),
            false => (palette.muted(), FontWeight::NORMAL),
        };
        let mut glyph = div()
            .relative()
            .child(svg().path(icon).size(px(look::ICON_HIT)).text_color(tone));
        let name = match (tab, unread) {
            (Tab::Notifications, 1..) => SharedString::from(format!("{label}, {unread} unread")),
            _ => SharedString::from(label),
        };
        if unread > 0 && tab == Tab::Notifications {
            glyph = glyph.child(
                dot(palette)
                    .debug_selector(|| "tab/notifications/dot".to_owned())
                    .size(px(12.))
                    .border_2()
                    .border_color(palette.background())
                    .absolute()
                    .top(px(-2.))
                    .right(px(-6.)),
            );
        }
        let mut item = div()
            .id(id)
            .control(Role::Tab, name)
            .aria_selected(chosen)
            .debug_selector(move || id.to_owned())
            .flex_1()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(3.))
            .child(glyph)
            .child(look::text(look::TINY, weight, tone).child(label))
            .on_click(cx.listener(move |root, _, window, cx| {
                window.blur(cx);
                root.pick(tab, cx);
            }));
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
    });
    div()
        .flex_none()
        .h(px(look::TAB_BAR) + inset)
        .pb(inset)
        .flex()
        .border_t_1()
        .border_color(palette.border())
        .bg(palette.background())
        .children(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use view_wire::manifest::Tint;

    fn row(label: &str, note: Option<&'static str>) -> RailRow {
        RailRow {
            program: "chat",
            label: label.into(),
            icon: String::new(),
            tint: Tint::None,
            description: String::new(),
            note,
            empty: false,
            pending: false,
            prep: Prep::Settled,
        }
    }

    /// A tile names its program by its manifest once the view is in, by its
    /// id with a capital until then.
    #[test]
    fn a_tile_names_its_program_by_manifest_else_by_id() {
        assert_eq!(name_of(&row("Chat rooms", None)), "Chat rooms");
        assert_eq!(name_of(&row("forge-view", Some("Loading"))), "Forge view");
    }

    /// New holds the programs with news, in the roster's order, and only
    /// those: none while no program has a dot (a count of 0 is none). The
    /// grid is every program as it was: New repeats a tile, never moves it.
    #[test]
    fn new_holds_the_programs_with_news_and_the_grid_keeps_its_order() {
        let rows: Vec<RailRow> = ["arcade", "chat", "files", "forge"]
            .into_iter()
            .map(|program| RailRow {
                program,
                ..row(program, None)
            })
            .collect();
        let programs = |rows: Vec<&RailRow>| rows.iter().map(|row| row.program).collect::<Vec<_>>();
        assert!(sections(&rows, &BTreeMap::new()).0.is_empty());
        assert!(sections(&rows, &BTreeMap::from([("chat", 0)])).0.is_empty());
        let badges = BTreeMap::from([("forge", 2), ("chat", 1)]);
        let (new, grid) = sections(&rows, &badges);
        assert_eq!(programs(new), ["chat", "forge"]);
        assert_eq!(programs(grid), ["arcade", "chat", "files", "forge"]);
    }

    /// A run counts the programs on its way when it begins: those read while
    /// some are still being read or not shown yet, then the ones with a view
    /// as they are prepared, the one a loader is at by name; it never grows
    /// (one that starts meanwhile waits for the next run, which begins as
    /// this one ends); new code over a view held is read, then prepared; none
    /// is on once every one is settled.
    #[test]
    fn a_run_counts_them_read_then_prepared_and_ends_when_all_are_settled() {
        // R its bytes coming, r read and not shown yet, W waiting for a
        // loader, K a loader at it, S settled, N settled with no view, H a
        // view held with new code coming
        let rows = |states: &[(&'static str, char)]| -> Vec<RailRow> {
            (states.iter())
                .map(|&(program, state)| {
                    let (prep, pending) = match state {
                        'R' => (Prep::Reading, true),
                        'r' => (Prep::Waiting, true),
                        'W' => (Prep::Waiting, false),
                        'K' => (Prep::Working, false),
                        'H' => (Prep::Reading, false),
                        _ => (Prep::Settled, false),
                    };
                    RailRow {
                        program,
                        prep,
                        pending,
                        empty: state == 'N',
                        ..row(program, (prep != Prep::Settled).then_some("Loading"))
                    }
                })
                .collect()
        };
        let mut run = Run::default();
        let mut step = |states: &[(&'static str, char)]| {
            let rows = rows(states);
            run.follow(&rows);
            run.caption(&rows)
        };
        let at = |caption: &str| Some(caption.to_owned());
        assert_eq!(step(&[("chat", 'S'), ("forge", 'S')]), None, "all settled");
        assert_eq!(
            step(&[("chat", 'R'), ("forge", 'R'), ("none", 'R')]),
            at("Loading 1/3")
        );
        assert_eq!(
            step(&[("chat", 'r'), ("forge", 'R'), ("none", 'r')]),
            at("Loading 3/3")
        );
        assert_eq!(
            step(&[("chat", 'r'), ("forge", 'r'), ("none", 'r')]),
            at("Loading 3/3"),
            "prepared before the tiles came"
        );
        assert_eq!(
            step(&[("chat", 'W'), ("forge", 'W'), ("none", 'N')]),
            at("Preparing 1/2")
        );
        assert_eq!(
            step(&[("chat", 'K'), ("forge", 'W'), ("none", 'N'), ("trade", 'R')]),
            at("Preparing 1/2 · Chat"),
            "grew with a program that started meanwhile"
        );
        assert_eq!(
            step(&[("chat", 'S'), ("forge", 'K'), ("none", 'N'), ("trade", 'R')]),
            at("Preparing 2/2 · Forge")
        );
        assert_eq!(
            step(&[("chat", 'S'), ("forge", 'S'), ("none", 'N'), ("trade", 'R')]),
            at("Loading 1/1"),
            "the next run did not begin as this one ended"
        );
        assert_eq!(step(&[("trade", 'S'), ("chat", 'H')]), at("Loading 1/1"));
        assert_eq!(
            step(&[("trade", 'S'), ("chat", 'K')]),
            at("Preparing 1/1 · Chat")
        );
        assert_eq!(step(&[("trade", 'S'), ("chat", 'S')]), None);
    }

    /// The ring's running part is one side long wherever it is, on the top
    /// side at the start, cut in two at a corner.
    #[test]
    fn the_rings_part_is_one_side_long_and_cut_at_a_corner() {
        let side = 64.;
        let length = |at| -> f32 { segment(at, side).map(|[_, _, w, h]| w.max(h)).sum() };
        for at in [0., 0.1, 0.25, 0.4, 0.8, 0.95] {
            assert!((length(at) - side).abs() < 1e-3, "{at}: {}", length(at));
        }
        assert_eq!(
            segment(0., side).collect::<Vec<_>>(),
            [[0., 0., side, RING]]
        );
        // half way along the top: the top's second half, the right's first
        assert_eq!(
            segment(0.125, side).collect::<Vec<_>>(),
            [[32., 0., 32., RING], [side - RING, 0., RING, 32.]]
        );
        // past the end: the left side's top half and the top's first half
        assert_eq!(
            segment(0.875, side).collect::<Vec<_>>(),
            [[0., 0., 32., RING], [0., 0., RING, 32.]]
        );
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
