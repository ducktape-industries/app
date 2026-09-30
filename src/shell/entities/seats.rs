//! Every pane's `Seat`, by the pane's layout `instance`: one per view pane
//! any window holds, placed in that window, told when it left every desk.
use crate::runtime::{Intent, Seat, WindowKey};
use crate::ui::layout::Layout;
use gpui_kit::{AnyWindowHandle, AppContext as _, Context, Entity, EventEmitter, Subscription};
use std::collections::BTreeMap;

/// A pane's seat and the routes out of it.
struct Placed {
    module: &'static str,
    seat: Entity<Seat>,
    /// Its intents, re-emitted as `(module, intent)`; dropping it unsubscribes.
    _intents: Subscription,
}

pub(crate) struct Seats {
    map: BTreeMap<u64, Placed>,
}

impl EventEmitter<(&'static str, Intent)> for Seats {}

impl Seats {
    pub(crate) fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    /// The seat of the pane with this layout `instance`.
    pub(crate) fn seat(&self, instance: u64) -> Option<Entity<Seat>> {
        self.map.get(&instance).map(|placed| placed.seat.clone())
    }

    /// A seat for every view pane the layouts hold, placed in the window
    /// whose layout holds it, and none for a pane they no longer hold: that
    /// one is told it is hidden first, and the intents its last update
    /// produced come back for the caller to route (its subscription is
    /// dropped here, so an emit would be lost). Notifies when a seat came
    /// or went, never when one moved: a pane's view observes its own seat.
    #[must_use]
    pub(crate) fn reconcile(
        &mut self,
        layouts: &BTreeMap<WindowKey, Layout>,
        windows: &BTreeMap<WindowKey, AnyWindowHandle>,
        cx: &mut Context<Self>,
    ) -> Vec<(&'static str, Intent)> {
        let wanted: BTreeMap<u64, (&'static str, Option<AnyWindowHandle>)> = layouts
            .iter()
            .flat_map(|(key, layout)| {
                let window = windows.get(key).copied();
                layout
                    .panes
                    .iter()
                    .filter(|pane| pane.is_view())
                    .map(move |pane| (pane.instance, (pane.module, window)))
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
            let placed = self.map.entry(instance).or_insert_with(|| {
                moved = true;
                let seat = cx.new(|cx| Seat::new(module, cx));
                let _intents = cx.subscribe(&seat, move |_, _, intent: &Intent, cx| {
                    cx.emit((module, intent.clone()))
                });
                Placed {
                    module,
                    seat,
                    _intents,
                }
            });
            if let Some(window) = window {
                placed.seat.update(cx, |seat, cx| seat.place(window, cx));
            }
        }
        let mut hidden = Vec::new();
        for instance in gone {
            let Some(placed) = self.map.remove(&instance) else {
                continue;
            };
            let intents = placed.seat.update(cx, |seat, _| seat.hide());
            hidden.extend(intents.into_iter().map(|intent| (placed.module, intent)));
        }
        if moved {
            cx.notify();
        }
        hidden
    }

    /// Every seat turned now. The AX door calls this before a read: a reply
    /// the guest has answered is then in the tree the read draws (or, with
    /// a tick still to draw, in the next one), as the draw itself took
    /// it in before the seat left the draw path. The door's task yields to
    /// no other between a press and its read, so the seats' own reply wakes
    /// have not run yet.
    pub(crate) fn settle(&self, cx: &mut Context<Self>) {
        for placed in self.map.values() {
            placed.seat.update(cx, |seat, cx| seat.turn(cx));
        }
    }

    /// The session's props, to every seat; a seat turns only when its
    /// bytes moved. s10 deletes this: `Seats` observes `Session`, `Account`
    /// and the theme itself.
    pub(crate) fn set_props(&self, props: Vec<u8>, cx: &mut Context<Self>) {
        for placed in self.map.values() {
            placed
                .seat
                .update(cx, |seat, cx| seat.set_props(props.clone(), cx));
        }
    }
}
