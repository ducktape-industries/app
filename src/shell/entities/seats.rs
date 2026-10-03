//! Every pane's `Seat`, by the pane's layout `instance`: one per view pane
//! any window holds, placed in that window, told when it left every desk,
//! and handed the session as its props (`Session`, `Account` and the
//! appearance, observed here; a seat turns only when the bytes moved).
//! It follows `Windows` and every window's `Desk` (`reconcile`), and
//! routes what a seat asks for: a badge to `Rail`, a notice to
//! `Notifications`, a view seated to the desks holding it, a link to
//! `Windows`.
use super::{Account, Notifications, Rail, Session, Windows};
use crate::runtime::{Intent, Seat, WindowKey};
use crate::shell::layers::view_body;
use crate::ui::layout::Layout;
use gpui_kit::{AnyWindowHandle, App, AppContext as _, Context, Entity, Subscription, WeakEntity};
use std::collections::BTreeMap;

/// A pane's seat and the routes out of it.
struct Placed {
    seat: Entity<Seat>,
    /// Its intents, routed here; dropping it unsubscribes.
    _intents: Subscription,
}

pub(crate) struct Seats {
    map: BTreeMap<u64, Placed>,
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
            props: Vec::new(),
            _observing: vec![
                cx.observe(session, |this, _, cx| this.props_moved(cx)),
                cx.observe(account, |this, _, cx| this.props_moved(cx)),
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
        for placed in self.map.values() {
            let props = self.props.clone();
            placed.seat.update(cx, |seat, cx| seat.set_props(props, cx));
        }
    }

    /// The seat of the pane with this layout `instance`.
    pub(crate) fn seat(&self, instance: u64) -> Option<Entity<Seat>> {
        self.map.get(&instance).map(|placed| placed.seat.clone())
    }

    /// A seat for every view pane the windows' desks hold, placed in the
    /// window whose desk holds it, and none for a pane they no longer
    /// hold: that one is told it is hidden first, its pictures leave the
    /// windows' atlases, and the intents its last update produced are routed (its subscription is dropped here, so an
    /// emit would be lost). Notifies when a seat came or went, never when
    /// one moved: a pane's view observes its own seat.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let Some(windows) = self.windows.upgrade() else {
            return;
        };
        let (layouts, handles, console): (
            BTreeMap<WindowKey, Layout>,
            BTreeMap<WindowKey, AnyWindowHandle>,
            Option<WindowKey>,
        ) = {
            let windows = windows.read(cx);
            (
                windows
                    .by_window()
                    .iter()
                    .map(|(key, own)| (*key, own.desk.read(cx).get().clone()))
                    .collect(),
                windows.handles().clone(),
                windows.console(),
            )
        };
        type Place = (&'static str, Option<(AnyWindowHandle, Option<(f32, f32)>)>);
        let wanted: BTreeMap<u64, Place> = layouts
            .iter()
            .flat_map(|(key, layout)| {
                let window = handles.get(key).copied();
                let console = console == Some(*key);
                layout
                    .panes
                    .iter()
                    .filter(|pane| pane.is_view())
                    .map(move |pane| {
                        let body = view_body(console, pane, layout.desk);
                        (
                            pane.instance,
                            (pane.module, window.map(|window| (window, body))),
                        )
                    })
            })
            .collect();
        let gone: Vec<u64> = self
            .map
            .keys()
            .filter(|instance| !wanted.contains_key(instance))
            .copied()
            .collect();
        let mut moved = !gone.is_empty();
        for (instance, (module, window)) in wanted {
            let props = &self.props;
            let placed = self.map.entry(instance).or_insert_with(|| {
                moved = true;
                let seat = cx.new(|cx| Seat::new(module, cx));
                let _intents = cx.subscribe(&seat, move |this, _, intent: &Intent, cx| {
                    this.route(module, intent.clone(), cx)
                });
                // the session as it stands, before its first turn
                seat.update(cx, |seat, cx| seat.set_props(props.clone(), cx));
                Placed { seat, _intents }
            });
            if let Some((window, body)) = window {
                placed
                    .seat
                    .update(cx, |seat, cx| seat.place(window, body, cx));
            }
        }
        let mut hidden = Vec::new();
        for instance in gone {
            let Some(placed) = self.map.remove(&instance) else {
                continue;
            };
            let module = placed.seat.read(cx).module();
            let intents = placed.seat.update(cx, |seat, _| seat.hide());
            // the seat drops with this entry: its images leave the atlas
            if let Some(tree) = placed.seat.read(cx).tree() {
                tree.update(cx, |tree, cx| tree.release(cx));
            }
            hidden.extend(intents.into_iter().map(|intent| (module, intent)));
        }
        if moved {
            cx.notify();
        }
        for (module, intent) in hidden {
            self.route(module, intent, cx);
        }
    }

    /// What a view of `module` asked for, to the entity it moves.
    fn route(&mut self, module: &'static str, intent: Intent, cx: &mut Context<Self>) {
        match intent {
            Intent::Badge(count) => self
                .rail
                .update(cx, |rail, cx| rail.set_badge(module, count, cx)),
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
        for placed in self.map.values() {
            placed.seat.update(cx, |seat, cx| seat.turn(cx));
        }
    }
}
