//! The programs the bar lists, each by the name its view gives itself, and
//! each view's unread count. The rows are read off the roster once per
//! change a loader thread, a seat coming or going, or a retry reports on the
//! roster's channel, never at a draw; the badges come from each view's
//! `Intent::Badge` (`Seats`), and go when the network is left.
use super::{Session, SessionEvent};
use crate::runtime::{Link, RailRow, Roster};
use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::{Context, Entity, Subscription, Task};
use std::collections::BTreeMap;

pub struct Rail {
    rows: Vec<RailRow>,
    badges: BTreeMap<&'static str, i64>,
    /// What the rows are read off; not compared.
    roster: Roster,
    _drain: Task<()>,
    _session: Subscription,
}

impl Rail {
    /// The rail of `roster`, read now and again on every message `changes`
    /// brings (a burst read once); its badges cleared as the network in
    /// hand is left.
    pub fn new(
        roster: Roster,
        mut changes: UnboundedReceiver<()>,
        session: &Entity<Session>,
        cx: &mut Context<Self>,
    ) -> Self {
        let leaving = cx.subscribe(session, |this, _, event: &SessionEvent, cx| {
            if let SessionEvent::LeftNetwork = event {
                this.set_badges(BTreeMap::new(), cx);
            }
        });
        let drain = cx.spawn(async move |this, cx| {
            while changes.next().await.is_some() {
                while changes.try_recv().is_ok() {}
                if this.update(cx, |rail, cx| rail.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            rows: roster.rail(),
            badges: BTreeMap::new(),
            roster,
            _drain: drain,
            _session: leaving,
        }
    }

    /// A link read against the programs the node lists (`Roster::parse_link`).
    pub fn parse_link(&self, link: &str) -> Link {
        self.roster.parse_link(link)
    }

    /// Where `link` goes, read against the programs the node lists and
    /// `chain`, the chain in hand. Each shell opens it its own way (the
    /// desktop's desk, the phone's stack).
    pub fn opening(&self, link: &str, chain: &str) -> Opening {
        opening(self.parse_link(link), |program| self.lists(program), chain)
    }

    /// Whether the node lists `program`.
    pub fn lists(&self, program: &str) -> bool {
        self.roster.lists(program)
    }

    pub fn rows(&self) -> &[RailRow] {
        &self.rows
    }

    /// Each view's unread count.
    pub fn badges(&self) -> &BTreeMap<&'static str, i64> {
        &self.badges
    }

    /// `program`'s unread count; 0 with none.
    pub fn badge(&self, program: &str) -> i64 {
        self.badges.get(program).copied().unwrap_or(0)
    }

    /// The rows read off the roster again; notifies only when they moved.
    pub fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let rows = self.roster.rail();
        if rows == self.rows {
            return false;
        }
        self.rows = rows;
        cx.notify();
        true
    }

    /// The rows of `roster` from now on: a test that swaps the model's.
    #[cfg(any(test, feature = "test-support"))]
    pub fn read_off(&mut self, roster: Roster, cx: &mut Context<Self>) {
        self.roster = roster;
        self.refresh(cx);
    }

    /// `program`'s unread count (`host.badge`); 0 or less takes it off.
    /// Notifies only when it moved.
    pub fn set_badge(&mut self, program: &'static str, count: i64, cx: &mut Context<Self>) {
        let was = match count > 0 {
            true => self.badges.insert(program, count),
            false => self.badges.remove(program),
        };
        if was != (count > 0).then_some(count) {
            cx.notify();
        }
    }

    /// Each view's unread count; notifies only when one moved.
    fn set_badges(&mut self, badges: BTreeMap<&'static str, i64>, cx: &mut Context<Self>) -> bool {
        if badges == self.badges {
            return false;
        }
        self.badges = badges;
        cx.notify();
        true
    }
}

/// Where a link goes ([`Rail::opening`]).
#[derive(Debug, PartialEq)]
pub enum Opening {
    /// `program`'s seat comes forward and its view is handed `route`;
    /// `notice` says what of the link it does not show.
    Seat {
        program: &'static str,
        route: Option<String>,
        notice: Option<String>,
    },
    /// An address for the system browser.
    Browser(String),
    /// Nothing opens; the notice says why.
    Nothing(String),
}

/// `duck://<chain>/<program>/<tail>` on this chain, or the short
/// `duck://<view>/<route>`: the seat opens and its view is handed the
/// route. The seat coming forward is the answer; a notice only says what
/// there is nothing to see of.
fn opening(parsed: Link, lists: impl Fn(&str) -> bool, chain: &str) -> Opening {
    let seat = |program, route, notice| Opening::Seat {
        program,
        route,
        notice,
    };
    match parsed {
        Link::View { program, route } => seat(program, route, None),
        Link::Chain(parsed) if !lists(&parsed.program) => {
            Opening::Nothing(format!("No view here opens {} links.", parsed.program))
        }
        Link::Chain(parsed) => {
            let program = crate::runtime::intern(&parsed.program);
            let route = parsed.tail.join("/");
            if parsed.chain.to_string() != chain {
                // another chain's page is not this chain's to show
                let said = format!(
                    "That link is to {}, not this network; opened {program}.",
                    parsed.chain.label
                );
                seat(program, None, Some(said))
            } else if route.is_empty() || crate::runtime::valid_route(&route) {
                seat(program, (!route.is_empty()).then_some(route), None)
            } else {
                let said = format!("{program} can't open that part of the link.");
                seat(program, None, Some(said))
            }
        }
        // to the view that declares `web`, if one is listed and this build
        // has a web view; else the system browser
        Link::Web(url) => match crate::render::web::view_for(&url).filter(|view| lists(view)) {
            Some(program) => seat(program, Some(url), None),
            None => Opening::Browser(url),
        },
        Link::Unknown => Opening::Nothing("This link is not one this app opens.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext as _, TestAppContext};

    /// A chain link opens its program on this chain with its route, on
    /// another chain without it and says so; one no view lists, or no
    /// link at all, opens nothing and says why.
    #[test]
    fn a_link_opens_its_program_on_this_chain_only() {
        let lists = |program: &str| program == "chat";
        let link = |chain: &str, program: &str, tail: &[&str]| {
            let minted = ducklink::mint(chain, program, tail).unwrap();
            ducklink::Link::parse(&minted).unwrap()
        };
        let here = "testkit#0a1b2c3d";
        assert_eq!(
            opening(Link::Chain(link(here, "chat", &["room", "42"])), lists, here),
            Opening::Seat {
                program: "chat",
                route: Some("room/42".into()),
                notice: None
            }
        );
        let Opening::Seat { route, notice, .. } = opening(
            Link::Chain(link("elsewhere#0a1b2c3d", "chat", &["room"])),
            lists,
            here,
        ) else {
            panic!("another chain's link still opens its program");
        };
        assert_eq!(route, None);
        assert_eq!(
            notice.as_deref(),
            Some("That link is to elsewhere, not this network; opened chat.")
        );
        assert_eq!(
            opening(Link::Chain(link(here, "forge", &[])), lists, here),
            Opening::Nothing("No view here opens forge links.".into())
        );
        assert_eq!(
            opening(Link::Unknown, lists, here),
            Opening::Nothing("This link is not one this app opens.".into())
        );
        assert_eq!(
            opening(Link::Web("https://ducktape.dev".into()), lists, here),
            Opening::Browser("https://ducktape.dev".into())
        );
    }

    #[gpui_kit::test]
    fn a_roster_change_recomputes_the_rail_once_and_notifies_only_on_change(
        cx: &mut TestAppContext,
    ) {
        const PROGRAM: &str = "rail-drain-view";
        let roster = Roster::listing(&[PROGRAM]);
        let (send, changes) = futures::channel::mpsc::unbounded();
        let session = cx.update(crate::session::tests::session);
        let rail = cx.new(|cx| Rail::new(roster.clone(), changes, &session, cx));
        let (seen, _observing) = crate::session::tests::notifies(&rail, cx);
        let note = |cx: &mut TestAppContext| rail.read_with(cx, |rail, _| rail.rows()[0].note);
        assert_eq!(note(cx), Some("Loading"));

        // the view landed on a loader thread, which says so on the channel
        crate::runtime::seat_for_test(PROGRAM, 400);
        send.unbounded_send(()).unwrap();
        cx.run_until_parked();
        assert_eq!(seen.get(), 1, "the rail moved and said nothing");
        assert_eq!(note(cx), None, "the rail still says the view loads");
        assert!(rail.read_with(cx, |rail, _| rail.rows() == roster.rail()));

        // a change that leaves the rows as they are, twice: read, not told
        send.unbounded_send(()).unwrap();
        send.unbounded_send(()).unwrap();
        cx.run_until_parked();
        assert_eq!(seen.get(), 1, "rows that did not move notified");
    }
}
