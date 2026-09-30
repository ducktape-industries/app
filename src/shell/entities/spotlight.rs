//! ⌘K in one window: the typed text and the picked row, and the rows the
//! text finds. The text is written from Spotlight's field as it changes
//! (the field owns what is typed); only the overlay layer reads it, so a
//! keystroke draws Spotlight and nothing else.
use super::{Account, Rail, Session, Slice, Spot, SpotRow};
use gpui_kit::Context;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Spotlight {
    pub(crate) query: String,
    pub(crate) pick: usize,
}

impl Slice<Spotlight> {
    /// The text as typed: the pick goes back to the first row.
    pub(crate) fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        let _timed = crate::perf::time(crate::perf::Key::Shell, "reducer.overlay");
        self.set(Spotlight { query, pick: 0 }, cx);
    }

    /// Up or down a row among `rows` shown, stopping at the ends.
    pub(crate) fn move_pick(&mut self, down: bool, rows: usize, cx: &mut Context<Self>) {
        let _timed = crate::perf::time(crate::perf::Key::Shell, "reducer.overlay");
        self.edit(
            |spotlight| {
                spotlight.pick = match (down, rows) {
                    (_, 0) => 0,
                    (true, rows) => (spotlight.pick + 1).min(rows - 1),
                    (false, _) => spotlight.pick.saturating_sub(1),
                }
            },
            cx,
        );
    }
}

impl Spotlight {
    /// What ⌘K offers for the text typed, in the order shown: the
    /// network's programs, the other networks this device reached, then
    /// things to do (the window rows while the desk's front window has a
    /// frame, `framed`). A row matches when its title or detail holds the
    /// text, ignoring case.
    pub(crate) fn rows(
        &self,
        rail: &Rail,
        session: &Session,
        account: &Account,
        framed: bool,
    ) -> Vec<SpotRow> {
        let row = |group, title: String, meta: String, spot| SpotRow {
            group,
            title,
            meta,
            spot,
        };
        let mut rows: Vec<SpotRow> = rail
            .rows()
            .iter()
            .filter(|program| !program.empty)
            .map(|program| {
                row(
                    "Programs",
                    program.label.clone(),
                    "Open".into(),
                    Spot::Open(program.module),
                )
            })
            .collect();
        rows.extend(
            session
                .recent_endpoints
                .iter()
                .filter(|entry| entry.url != session.connected_rpc)
                .map(|entry| {
                    row(
                        "Networks",
                        entry.name(),
                        entry.host().to_owned(),
                        Spot::Switch(entry.url.clone()),
                    )
                }),
        );
        rows.push(row(
            "Actions",
            "Help".into(),
            "keys and the desk".into(),
            Spot::Help,
        ));
        rows.push(row(
            "Actions",
            "Ducktape settings".into(),
            "theme, networks".into(),
            Spot::Settings,
        ));
        let keyed = !account.signer_key.is_empty();
        if matches!(account.account, Some(None)) && keyed {
            rows.push(row(
                "Actions",
                "Create account".into(),
                session.network.clone(),
                Spot::CreateAccount,
            ));
        }
        if keyed {
            rows.push(row(
                "Actions",
                "Lock".into(),
                "this device's key".into(),
                Spot::Lock,
            ));
        }
        if framed {
            for (title, spot) in [
                ("Fill window", Spot::FillWindow),
                ("Move or size window", Spot::HoldWindow),
            ] {
                rows.push(row("Actions", title.into(), "this desk".into(), spot));
            }
        }
        for (title, mode) in [
            ("Light appearance", crate::Appearance::Light),
            ("Dark appearance", crate::Appearance::Dark),
            ("Match the system's appearance", crate::Appearance::System),
        ] {
            rows.push(row(
                "Actions",
                title.into(),
                String::new(),
                Spot::Appearance(mode),
            ));
        }
        rows.push(row(
            "Actions",
            "Add a network…".into(),
            String::new(),
            Spot::OtherNetwork,
        ));
        let query = self.query.trim().to_lowercase();
        rows.retain(|row| {
            query.is_empty()
                || row.title.to_lowercase().contains(&query)
                || row.meta.to_lowercase().contains(&query)
        });
        rows
    }

    /// The row Enter runs: the picked one.
    pub(crate) fn picked(&self, rows: &[SpotRow]) -> Option<Spot> {
        rows.get(self.pick).map(|row| row.spot.clone())
    }
}
