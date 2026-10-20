//! Debug mode's Memory window: the app's memory by owner
//! (`ducktape_host::memory`), read on each frame the window draws, and the
//! OS's regions as it opens and on "Read regions again".

use super::super::WindowKey;
use super::super::entities::{Entities, Prefs, Seats, Slice, Windows};
use super::super::ink::{Ink, Kind, button, h1, mono, sans, words};
use ducktape_host::memory::{self, Figure, Group, OsMemory, Owned, Region};
use gpui_kit::*;

const GROUPS: [(Group, &str, &str); 7] = [
    (Group::View, "views", "VIEWS · WASM MEMORY"),
    (Group::Notifier, "notifiers", "NOTIFIERS · WASM MEMORY"),
    (Group::Pictures, "pictures", "PICTURES THE VIEWS SENT"),
    (Group::Rasters, "rasters", "DECODED PICTURES"),
    (Group::SvgRasters, "svg", "SVG RASTERS"),
    (
        Group::Code,
        "code",
        "COMPILED VIEW CODE · NOT IN THE FOOTPRINT",
    ),
    (
        Group::Fonts,
        "fonts",
        "BUNDLED FONTS · NOT IN THE FOOTPRINT",
    ),
];

pub(in crate::shell) struct MemoryPane {
    key: WindowKey,
    windows: Entity<Windows>,
    seats: Entity<Seats>,
    prefs: Entity<Slice<Prefs>>,
    regions: Option<Vec<Region>>,
}

impl MemoryPane {
    pub(in crate::shell) fn new(app: &Entities, key: WindowKey) -> Self {
        Self {
            key,
            windows: app.windows.clone(),
            seats: app.seats.clone(),
            prefs: app.prefs.clone(),
            regions: regions(),
        }
    }

    fn read_regions(&mut self, cx: &mut Context<Self>) {
        self.regions = regions();
        cx.notify();
    }
}

fn regions() -> Option<Vec<Region>> {
    ducktape_host::contract::os()
        .memory(true)
        .and_then(|os| os.regions)
}

impl Render for MemoryPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.memory", 1);
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let windows: Vec<_> = (self.windows.read(cx).handles().iter())
            .map(|(key, handle)| (format!("window {}", key.0), *handle))
            .collect();
        let seated = self.seats.read(cx).drawn(cx);
        let mut reading = memory::read(false);
        memory::drawn(&mut reading, &seated, &windows, cx);
        let pane = cx.entity().downgrade();
        let again = move |cx: &mut App| {
            let _ = pane.update(cx, |pane, cx| pane.read_regions(cx));
        };
        page(
            reading.os.as_ref(),
            &reading.owners,
            self.regions.as_deref(),
            again,
            &ink,
        )
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024. * 1024.))
}

fn page(
    os: Option<&OsMemory>,
    owners: &[Owned],
    regions: Option<&[Region]>,
    again: impl Fn(&mut App) + 'static,
    ink: &Ink,
) -> AnyElement {
    let section = |id: &str, title: String| {
        mono(400, 12.)
            .text_color(ink.muted)
            .pt(px(24.))
            .pb(px(8.))
            .child(words(SharedString::from(id.to_owned()), title))
    };
    let said = |id: &str, text: &str| {
        sans(400, 13.)
            .text_color(ink.muted)
            .py(px(5.))
            .child(words(SharedString::from(id.to_owned()), text.to_owned()))
    };
    let row = |id: String, name: String, value: String| {
        div()
            .flex()
            .items_baseline()
            .gap(px(16.))
            .py(px(5.))
            .border_b_1()
            .border_color(ink.line)
            .child(
                sans(400, 13.)
                    .flex_1()
                    .min_w_0()
                    .text_color(ink.ink)
                    .child(words(SharedString::from(format!("{id}/name")), name)),
            )
            .child(
                mono(400, 12.)
                    .flex_shrink_0()
                    .text_color(ink.ink)
                    .child(words(SharedString::from(format!("{id}/value")), value)),
            )
    };
    let mut body = div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .child(h1("memory/heading", "Memory", ink))
                .child(button(
                    "memory/regions",
                    "Read regions again",
                    Kind::Small,
                    again,
                    false,
                    ink,
                )),
        )
        .child(said(
            "memory/lead",
            "Read on each frame this window draws, and the OS's regions when it opened and on Read regions again.",
        ))
        .child(section("memory/os", "AS THE OS COUNTS IT".to_owned()));
    match os {
        Some(os) => {
            let figure = match os.figure {
                Figure::Footprint => "Footprint (Activity Monitor's figure)",
                Figure::Resident => "Resident pages",
            };
            let figures = [
                ("total", figure, Some(os.bytes)),
                ("peak", "Peak", os.peak),
                ("heap", "Heap in use (the allocator's figure)", os.heap),
                ("graphics", "Graphics (the OS's ledger)", os.graphics),
            ];
            for (id, name, bytes) in figures {
                if let Some(bytes) = bytes {
                    body = body.child(row(format!("memory/os/{id}"), name.to_owned(), mib(bytes)));
                }
            }
        }
        None => {
            body = body.child(said(
                "memory/os/none",
                "This system reports no figure for the app.",
            ))
        }
    }
    for (group, id, title) in GROUPS {
        let mut held: Vec<&Owned> = owners.iter().filter(|owned| owned.group == group).collect();
        if held.is_empty() {
            continue;
        }
        held.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
        let total = held.iter().map(|owned| owned.bytes).sum();
        body = body.child(section(
            &format!("memory/{id}"),
            format!("{title} · {}", mib(total)),
        ));
        for (n, owned) in held.into_iter().enumerate() {
            let value = match owned.resident {
                Some(resident) if resident != owned.bytes => {
                    format!("{} · {} resident", mib(owned.bytes), mib(resident))
                }
                _ => mib(owned.bytes),
            };
            body = body.child(row(format!("memory/{id}/{n}"), owned.name.clone(), value));
        }
    }
    body = body.child(section(
        "memory/regions/title",
        "OS REGIONS · DIRTY · RESIDENT".to_owned(),
    ));
    match regions {
        Some(regions) if !regions.is_empty() => {
            for (n, region) in regions.iter().enumerate() {
                body = body.child(row(
                    format!("memory/region/{n}"),
                    region.kind.clone(),
                    format!("{} · {}", mib(region.dirty), mib(region.resident)),
                ));
            }
        }
        _ => {
            body = body.child(said(
                "memory/region/none",
                "This system reports no regions.",
            ))
        }
    }
    div()
        .id("memory")
        .role(Role::Document)
        .aria_label("Memory")
        .size_full()
        .overflow_scroll()
        .child(
            div()
                .mx_auto()
                .w_full()
                .min_w(px(360.))
                .max_w(px(640.))
                .px(px(32.))
                .py(px(32.))
                .child(body),
        )
        .into_any_element()
}
