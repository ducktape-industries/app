//! Every placed view pane's `Seat`, by the pane's layout `instance`: one
//! per view pane the shell's windows hold, placed where the shell says
//! (`place`), told when it left every window, and handed the session as
//! its props (`Session`, `Account` and the appearance, observed here; a
//! seat turns only when the bytes moved). It routes what a seat asks for:
//! a badge to `Rail`, a notice to `Notifications`; what concerns the
//! shell's windows (a title, a view seated, a link) is a `SeatsEvent`.
//! Which panes exist, and in which window, is the shell's
//! (the desktop's `shell::entities::DeskSeats` follows its desks).
use super::{Account, Notifications, Rail, Session};
use crate::runtime::{Intent, Seat};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, EventEmitter, Subscription,
};
use std::collections::BTreeMap;

/// A pane's seat and the routes out of it.
struct Placed {
    seat: Entity<Seat>,
    /// Its intents, routed here; dropping it unsubscribes.
    _intents: Subscription,
}

/// Where the shell wants one pane's seat: its program, and the window it
/// is drawn in with the body it is laid out in and whether that window is
/// the one in front (`None` while the window is not open yet).
pub type Place = (
    &'static str,
    Option<(AnyWindowHandle, Option<(f32, f32)>, bool)>,
);

/// What a seat asked for that the shell's windows answer.
#[derive(Clone, Debug)]
pub enum SeatsEvent {
    /// The pane with this layout `instance` named its window.
    Title(u64, String),
    /// `program`'s view came: a window placed before it is widened to it.
    Seated(&'static str),
    /// From the pane (`program`, `instance`): a link to open.
    OpenLink(&'static str, u64, String),
}

pub struct Seats {
    map: BTreeMap<u64, Placed>,
    session: Entity<Session>,
    account: Entity<Account>,
    rail: Entity<Rail>,
    notifications: Entity<Notifications>,
    /// The session as every seat last heard it, encoded.
    props: Vec<u8>,
    _observing: Vec<Subscription>,
}

impl EventEmitter<SeatsEvent> for Seats {}

impl Seats {
    pub fn new(
        session: &Entity<Session>,
        account: &Entity<Account>,
        rail: &Entity<Rail>,
        notifications: &Entity<Notifications>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            map: BTreeMap::new(),
            props: Vec::new(),
            _observing: vec![
                cx.observe(session, |this, _, cx| this.props_moved(cx)),
                cx.observe(account, |this, _, cx| this.props_moved(cx)),
            ],
            session: session.clone(),
            account: account.clone(),
            rail: rail.clone(),
            notifications: notifications.clone(),
        };
        this.props = this.encode(cx);
        this
    }

    /// What every view is handed as its props: the node the views are on
    /// (not the address being typed or tried, a switch in flight), the
    /// chain, the seated key and its account.
    fn encode(&self, cx: &App) -> Vec<u8> {
        let session = self.session.read(cx).get();
        let account = self.account.read(cx).get();
        crate::runtime::props(
            session.connected,
            &session.chain,
            &account.signer_key,
            account.account.clone().flatten().map(|(number, _)| number),
            &session.connected_rpc,
        )
    }

    /// The session moved: the props to every seat, which turns only when
    /// its bytes did.
    fn props_moved(&mut self, cx: &mut Context<Self>) {
        let props = self.encode(cx);
        if props == self.props {
            return;
        }
        self.props = props;
        for placed in self.map.values() {
            let props = self.props.clone();
            placed.seat.update(cx, |seat, cx| seat.set_props(props, cx));
        }
    }

    /// The seat of the pane with this layout `instance`.
    pub fn seat(&self, instance: u64) -> Option<Entity<Seat>> {
        self.map.get(&instance).map(|placed| placed.seat.clone())
    }

    /// A seat for every pane in `wanted`, placed where it says, and none
    /// for a pane no longer there: that one is told it is hidden first,
    /// its pictures leave the windows' atlases, and the intents its last
    /// update produced are routed (its subscription is dropped here, so an
    /// emit would be lost). Notifies when a seat came or went, never when
    /// one moved: a pane's view observes its own seat.
    pub fn place(&mut self, wanted: BTreeMap<u64, Place>, cx: &mut Context<Self>) {
        let gone: Vec<u64> = self
            .map
            .keys()
            .filter(|instance| !wanted.contains_key(instance))
            .copied()
            .collect();
        let mut moved = !gone.is_empty();
        for (instance, (program, window)) in wanted {
            let props = &self.props;
            let placed = self.map.entry(instance).or_insert_with(|| {
                moved = true;
                let seat = cx.new(|cx| Seat::new(program, instance, cx));
                let _intents = cx.subscribe(&seat, move |this, _, intent: &Intent, cx| {
                    this.route(program, instance, intent.clone(), cx)
                });
                // the session as it stands, before its first turn
                seat.update(cx, |seat, cx| seat.set_props(props.clone(), cx));
                Placed { seat, _intents }
            });
            if let Some((window, body, in_front)) = window {
                placed
                    .seat
                    .update(cx, |seat, cx| seat.place(window, body, in_front, cx));
            }
        }
        let mut hidden = Vec::new();
        for instance in gone {
            let Some(placed) = self.map.remove(&instance) else {
                continue;
            };
            let program = placed.seat.read(cx).program();
            let intents = placed.seat.update(cx, |seat, _| seat.hide());
            // the seat drops with this entry: its images leave the atlas
            if let Some(tree) = placed.seat.read(cx).tree() {
                tree.update(cx, |tree, cx| tree.release(cx));
            }
            hidden.extend(
                intents
                    .into_iter()
                    .map(|intent| (program, instance, intent)),
            );
        }
        if moved {
            cx.notify();
        }
        for (program, instance, intent) in hidden {
            self.route(program, instance, intent, cx);
        }
    }

    /// What the view of `program` in the pane with this layout `instance`
    /// asked for, to the entity it moves, or to the shell.
    fn route(
        &mut self,
        program: &'static str,
        instance: u64,
        intent: Intent,
        cx: &mut Context<Self>,
    ) {
        match intent {
            Intent::Badge(count) => self
                .rail
                .update(cx, |rail, cx| rail.set_badge(program, count, cx)),
            Intent::Title(title) => cx.emit(SeatsEvent::Title(instance, title)),
            Intent::Notified => self
                .notifications
                .update(cx, |notifications, cx| _ = notifications.refresh(cx)),
            Intent::Seated => cx.emit(SeatsEvent::Seated(program)),
            Intent::OpenLink(link) => cx.emit(SeatsEvent::OpenLink(program, instance, link)),
        }
    }

    /// Every seat turned now, and whether every seat's tick has landed.
    /// The AX door calls this before a read, and parks the read while a
    /// tick is in flight: a reply the view has answered is then in the tree
    /// the read draws (or, with a tick still to draw, in the next one), as
    /// the draw itself took it in before the seat left the draw path. The
    /// door's task yields to no other between a press and its read, so the
    /// seats' own reply wakes have not run yet. On the wasmtime runner a
    /// tick lands inside its turn: this answers `true` at once.
    #[cfg(any(test, feature = "ax-door"))]
    pub fn settle(&self, cx: &mut Context<Self>) -> bool {
        let mut landed = true;
        for placed in self.map.values() {
            placed.seat.update(cx, |seat, cx| seat.turn(cx));
            landed &= !placed.seat.read(cx).in_flight();
        }
        landed
    }
}
