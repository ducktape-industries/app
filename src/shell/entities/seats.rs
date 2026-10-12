//! Every pane's `Seat`, by the pane's layout `instance`: one per view pane
//! any window holds, placed in that window (and told whether that window
//! is the one in front), told when it left every desk,
//! and handed the session as its props (`Session`, `Account` and the
//! appearance, observed here; a seat turns only when the bytes moved).
//! It follows `Windows` and every window's `Desk` (`reconcile`), and
//! routes what a seat asks for: a badge to `Rail`, a title to the desk
//! holding its pane, a notice to `Notifications`, a view seated to the
//! desks holding it, a link to `Windows`.
//!
//! A view that posts notices (`notify` in its manifest) runs on while no
//! pane holds it and a network is in hand: a seat in no window, laid out
//! unseen, so a mention that lands with its window closed is still
//! noticed. The pane that opens on
//! it takes that very seat over, and the last of its panes to close hands
//! it back.
use super::{Account, Notifications, Rail, Session, Windows};
use crate::runtime::{Intent, Seat, WindowKey};
use crate::shell::layers::view_body;
use crate::ui::layout::Layout;
use gpui_kit::{AnyWindowHandle, App, AppContext as _, Context, Entity, Subscription, WeakEntity};
use std::collections::{BTreeMap, BTreeSet};

/// The pane instance a seat no pane holds routes its intents under: none,
/// since a layout's instances start at 1.
const UNSEEN: u64 = 0;

/// The body a view no pane holds is laid out in, never drawn: the size a
/// new window opens at.
const UNSEEN_BODY: (f32, f32) = crate::shell::windows::WINDOW_SIZE;

/// A pane's seat and the routes out of it.
struct Placed {
    seat: Entity<Seat>,
    /// Its intents, routed here; dropping it unsubscribes.
    _intents: Subscription,
}

pub(crate) struct Seats {
    map: BTreeMap<u64, Placed>,
    /// The seats of views that post notices and no pane holds, by module,
    /// run on unseen (`Seat::run_unseen`).
    unseen: BTreeMap<&'static str, Placed>,
    session: Entity<Session>,
    account: Entity<Account>,
    rail: Entity<Rail>,
    notifications: Entity<Notifications>,
    /// The windows the seats are placed in (`follow`). Weak: `Windows`
    /// holds this entity for the roots it opens.
    windows: WeakEntity<Windows>,
    /// One observer per window's desk, dropped with the window.
    desks: BTreeMap<WindowKey, Subscription>,
    /// The session as every seat last heard it, encoded.
    props: Vec<u8>,
    _observing: Vec<Subscription>,
}

impl Seats {
    pub(crate) fn new(
        session: &Entity<Session>,
        account: &Entity<Account>,
        rail: &Entity<Rail>,
        notifications: &Entity<Notifications>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            map: BTreeMap::new(),
            unseen: BTreeMap::new(),
            props: Vec::new(),
            _observing: vec![
                // a network left takes its unseen seats with it
                cx.observe(session, |this, _, cx| {
                    this.props_moved(cx);
                    this.reconcile(cx);
                }),
                cx.observe(account, |this, _, cx| this.props_moved(cx)),
                // a program listed, or its view read far enough to say it
                // posts notices
                cx.observe(rail, |this, _, cx| this.reconcile(cx)),
            ],
            session: session.clone(),
            account: account.clone(),
            rail: rail.clone(),
            notifications: notifications.clone(),
            windows: WeakEntity::new_invalid(),
            desks: BTreeMap::new(),
        };
        this.props = this.encode(cx);
        this
    }

    /// The seats follow `windows` from now on: a window coming or going,
    /// and every desk's move, reconciles them.
    pub(crate) fn follow(&mut self, windows: &Entity<Windows>, cx: &mut Context<Self>) {
        self.windows = windows.downgrade();
        self._observing
            .push(cx.observe(windows, |this, windows, cx| {
                this.windows_moved(&windows, cx)
            }));
        self.windows_moved(windows, cx);
    }

    /// The window list moved: a desk observer for every window that has
    /// none yet, none for a window that went, and the seats reconciled.
    fn windows_moved(&mut self, windows: &Entity<Windows>, cx: &mut Context<Self>) {
        let desks: Vec<_> = windows
            .read(cx)
            .by_window()
            .iter()
            .map(|(key, own)| (*key, own.desk.clone()))
            .collect();
        self.desks
            .retain(|key, _| desks.iter().any(|(there, _)| there == key));
        for (key, desk) in desks {
            self.desks
                .entry(key)
                .or_insert_with(|| cx.observe(&desk, |this, _, cx| this.reconcile(cx)));
        }
        self.reconcile(cx);
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
        for placed in self.map.values().chain(self.unseen.values()) {
            let props = self.props.clone();
            placed.seat.update(cx, |seat, cx| seat.set_props(props, cx));
        }
    }

    /// The seat of the pane with this layout `instance`.
    pub(crate) fn seat(&self, instance: u64) -> Option<Entity<Seat>> {
        self.map.get(&instance).map(|placed| placed.seat.clone())
    }

    /// The seat `module`'s view runs unseen in, while no pane holds it.
    #[cfg(test)]
    pub(crate) fn unseen(&self, module: &str) -> Option<Entity<Seat>> {
        self.unseen.get(module).map(|placed| placed.seat.clone())
    }

    /// A seat for every view pane the windows' desks hold, placed in the
    /// window whose desk holds it, and none for a pane they no longer
    /// hold: that one is told it is hidden first, its pictures leave the
    /// windows' atlases, and the intents its last update produced are
    /// routed (its subscription is dropped here, so an emit would be lost).
    /// A view that posts notices and that no pane holds keeps one seat,
    /// run on unseen: the last of its panes to close hands its seat over,
    /// and the next pane to open on it takes that seat back. Notifies when
    /// a seat came or went, never when one moved: a pane's view observes
    /// its own seat.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let Some(windows) = self.windows.upgrade() else {
            return;
        };
        let (layouts, handles, console, front) = {
            let windows = windows.read(cx);
            let layouts: BTreeMap<WindowKey, Layout> = windows
                .by_window()
                .iter()
                .map(|(key, own)| (*key, own.desk.read(cx).get().clone()))
                .collect();
            (
                layouts,
                windows.handles().clone(),
                windows.console(),
                windows.front(),
            )
        };
        type Place = (
            &'static str,
            Option<(AnyWindowHandle, Option<(f32, f32)>, bool)>,
        );
        let wanted: BTreeMap<u64, Place> = layouts
            .iter()
            .flat_map(|(key, layout)| {
                let window = handles.get(key).copied();
                let console = console == Some(*key);
                let in_front = front == Some(*key);
                layout
                    .panes
                    .iter()
                    .enumerate()
                    .filter(|(_, pane)| pane.is_view())
                    .map(move |(index, pane)| {
                        let body = view_body(console, layout, index);
                        (
                            pane.instance,
                            (pane.module, window.map(|window| (window, body, in_front))),
                        )
                    })
            })
            .collect();
        // the listed views that no pane holds and that post notices, while
        // a network is in hand (the rail lists the last one's still): one
        // that runs unseen already stays while its load is on its way
        let held: BTreeSet<&'static str> = wanted.values().map(|(module, _)| *module).collect();
        let connected = self.session.read(cx).get().connected;
        let unseen: BTreeSet<&'static str> = self
            .rail
            .read(cx)
            .rows()
            .iter()
            .filter(|_| connected)
            .filter(|row| !row.empty && !held.contains(row.module))
            .map(|row| row.module)
            .filter(|module| match crate::runtime::notifies(module) {
                Some(notifies) => notifies,
                None => self.unseen.contains_key(module),
            })
            .collect();
        let gone: Vec<u64> = self
            .map
            .keys()
            .filter(|instance| !wanted.contains_key(instance))
            .copied()
            .collect();
        let mut moved = !gone.is_empty();
        let mut titled = Vec::new();
        for (instance, (module, window)) in wanted {
            if !self.map.contains_key(&instance) {
                moved = true;
                // the seat that ran unseen, its view as it stands, or a new one
                let seat = match self.unseen.remove(module) {
                    Some(placed) => {
                        let title = placed.seat.read(cx).title().map(str::to_owned);
                        titled.extend(title.map(|title| (module, instance, title)));
                        placed.seat
                    }
                    None => self.new_seat(module, cx),
                };
                let placed = self.routed(module, instance, seat, cx);
                self.map.insert(instance, placed);
            }
            if let Some((window, body, in_front)) = window {
                self.map[&instance]
                    .seat
                    .update(cx, |seat, cx| seat.place(window, body, in_front, cx));
            }
        }
        let mut hidden = Vec::new();
        for instance in gone {
            let Some(placed) = self.map.remove(&instance) else {
                continue;
            };
            let module = placed.seat.read(cx).module();
            // the last pane on a view that posts notices: it runs on unseen
            if unseen.contains(module) && !self.unseen.contains_key(module) {
                let body = placed.seat.read(cx).body().unwrap_or(UNSEEN_BODY);
                placed.seat.update(cx, |seat, cx| seat.run_unseen(body, cx));
                // no window draws it now: its images leave the atlases until
                // a pane draws it again
                if let Some(tree) = placed.seat.read(cx).tree() {
                    tree.update(cx, |tree, cx| tree.release(cx));
                }
                let placed = self.routed(module, UNSEEN, placed.seat, cx);
                self.unseen.insert(module, placed);
                continue;
            }
            hidden.extend(Self::hide(placed, cx).map(|intent| (module, instance, intent)));
        }
        // one seat for each view to run unseen, none for any other
        let stale: Vec<&'static str> = self
            .unseen
            .keys()
            .filter(|module| !unseen.contains(*module))
            .copied()
            .collect();
        for module in stale {
            if let Some(placed) = self.unseen.remove(module) {
                moved = true;
                hidden.extend(Self::hide(placed, cx).map(|intent| (module, UNSEEN, intent)));
            }
        }
        for module in unseen {
            if !self.unseen.contains_key(module) {
                moved = true;
                let seat = self.new_seat(module, cx);
                seat.update(cx, |seat, cx| seat.run_unseen(UNSEEN_BODY, cx));
                let placed = self.routed(module, UNSEEN, seat, cx);
                self.unseen.insert(module, placed);
            }
        }
        if moved {
            cx.notify();
        }
        for (module, instance, intent) in hidden {
            self.route(module, instance, intent, cx);
        }
        // a pane that took over a seat no pane held names its window as
        // that view last asked
        for (module, instance, title) in titled {
            self.route(module, instance, Intent::Title(title), cx);
        }
    }

    /// A new seat of `module`, handed the session as it stands before its
    /// first turn.
    fn new_seat(&self, module: &'static str, cx: &mut Context<Self>) -> Entity<Seat> {
        let seat = cx.new(|cx| Seat::new(module, cx));
        let props = self.props.clone();
        seat.update(cx, |seat, cx| seat.set_props(props, cx));
        seat
    }

    /// `seat`, its intents routed as the pane `instance`'s ([`UNSEEN`]:
    /// none's).
    fn routed(
        &self,
        module: &'static str,
        instance: u64,
        seat: Entity<Seat>,
        cx: &mut Context<Self>,
    ) -> Placed {
        let _intents = cx.subscribe(&seat, move |this, _, intent: &Intent, cx| {
            this.route(module, instance, intent.clone(), cx)
        });
        Placed { seat, _intents }
    }

    /// `placed` let go: its view told it is hidden, its last update's
    /// intents handed back for the caller to route, its pictures out of the
    /// windows' atlases.
    fn hide(placed: Placed, cx: &mut Context<Self>) -> impl Iterator<Item = Intent> {
        let intents = placed.seat.update(cx, |seat, _| seat.hide());
        // the seat drops with this entry: its images leave the atlas
        if let Some(tree) = placed.seat.read(cx).tree() {
            tree.update(cx, |tree, cx| tree.release(cx));
        }
        intents.into_iter()
    }

    /// What the view of `module` in the pane with this layout `instance`
    /// asked for, to the entity it moves.
    fn route(
        &mut self,
        module: &'static str,
        instance: u64,
        intent: Intent,
        cx: &mut Context<Self>,
    ) {
        match intent {
            Intent::Badge(count) => self
                .rail
                .update(cx, |rail, cx| rail.set_badge(module, count, cx)),
            // its own window's name, wherever that pane is now (a pane
            // gone from every desk names nothing)
            Intent::Title(title) => {
                let Some(windows) = self.windows.upgrade() else {
                    return;
                };
                let desk = windows
                    .read(cx)
                    .by_window()
                    .values()
                    .map(|own| own.desk.clone())
                    .find(|desk| {
                        desk.read(cx)
                            .get()
                            .panes
                            .iter()
                            .any(|pane| pane.instance == instance)
                    });
                if let Some(desk) = desk {
                    desk.update(cx, |desk, cx| desk.set_title(instance, title, cx));
                }
            }
            Intent::Notified => self
                .notifications
                .update(cx, |notifications, cx| _ = notifications.refresh(cx)),
            // a window placed before its view came is widened to it
            Intent::Seated => {
                let Some(windows) = self.windows.upgrade() else {
                    return;
                };
                let desks: Vec<_> = windows
                    .read(cx)
                    .by_window()
                    .values()
                    .map(|own| own.desk.clone())
                    .filter(|desk| desk.read(cx).holds(module))
                    .collect();
                for desk in desks {
                    desk.update(cx, |desk, cx| desk.settle(cx));
                }
            }
            Intent::OpenLink(link) => {
                if let Some(windows) = self.windows.upgrade() {
                    windows.update(cx, |windows, cx| windows.open_link(&link, cx));
                }
            }
            // the person is asked on the console, whichever window the
            // view is in
            Intent::Consent => match self.windows.upgrade() {
                Some(windows) => windows.update(cx, |windows, cx| windows.sync_consent(cx)),
                None => crate::runtime::consent::refuse_all(),
            },
        }
    }

    /// Every seat turned now. The AX door calls this before a read: a reply
    /// the guest has answered is then in the tree the read draws (or, with
    /// a tick still to draw, in the next one), as the draw itself took
    /// it in before the seat left the draw path. The door's task yields to
    /// no other between a press and its read, so the seats' own reply wakes
    /// have not run yet.
    #[cfg(any(test, feature = "ax-door"))]
    pub(crate) fn settle(&self, cx: &mut Context<Self>) {
        for placed in self.map.values().chain(self.unseen.values()) {
            placed.seat.update(cx, |seat, cx| seat.turn(cx));
        }
    }
}
