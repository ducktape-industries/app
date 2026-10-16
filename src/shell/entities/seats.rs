//! The desktop's half of the seats: which view panes exist, and in which
//! window, is the windows' desks' to say. `DeskSeats` follows `Windows`
//! and every window's `Desk`, tells the shared `Seats` where each pane's
//! seat is placed (`Seats::place`), and answers what a seat asks of the
//! windows (`SeatsEvent`): a title to the desk holding its pane, a view
//! seated to the desks holding it, a link to `Windows`.
use super::{Seats, Windows};
use crate::runtime::WindowKey;
use crate::shell::WindowKind;
use crate::shell::layers::view_body;
use crate::ui::layout::Layout;
use ducktape_host::session::{Place, SeatsEvent};
use gpui_kit::{Context, Entity, Subscription, WeakEntity};
use std::collections::BTreeMap;

pub(crate) struct DeskSeats {
    seats: Entity<Seats>,
    /// The windows the seats are placed in. Weak: `Windows` holds the
    /// roots it opens.
    windows: WeakEntity<Windows>,
    /// One observer per window's desk, dropped with the window.
    desks: BTreeMap<WindowKey, Subscription>,
    _observing: Vec<Subscription>,
}

impl DeskSeats {
    /// The seats follow `windows` from now on: a window coming or going,
    /// and every desk's move, reconciles them.
    pub(crate) fn new(
        seats: &Entity<Seats>,
        windows: &Entity<Windows>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            seats: seats.clone(),
            windows: windows.downgrade(),
            desks: BTreeMap::new(),
            _observing: vec![
                cx.observe(windows, |this, windows, cx| {
                    this.windows_moved(&windows, cx)
                }),
                cx.subscribe(seats, |this, _, event: &SeatsEvent, cx| {
                    this.answer(event, cx)
                }),
            ],
        };
        this.windows_moved(windows, cx);
        this
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

    /// A seat for every view pane the windows' desks hold, placed in the
    /// window whose desk holds it, and none for a pane they no longer hold.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let Some(windows) = self.windows.upgrade() else {
            return;
        };
        let (layouts, handles, front) = {
            let windows = windows.read(cx);
            // the body is the frame's on a console's desk, where the pane
            // is drawn framed; a window one view fills (a pop-out, `--live`)
            // lays its pane out edge to edge, whatever frame the layout keeps
            let layouts: BTreeMap<WindowKey, (Layout, bool)> = windows
                .by_window()
                .iter()
                .map(|(key, own)| {
                    let on_desk = own.kind == WindowKind::Console;
                    (*key, (own.desk.read(cx).get().clone(), on_desk))
                })
                .collect();
            (layouts, windows.handles().clone(), windows.front())
        };
        let wanted: BTreeMap<u64, Place> = layouts
            .iter()
            .flat_map(|(key, (layout, on_desk))| {
                let window = handles.get(key).copied();
                let on_desk = *on_desk;
                let in_front = front == Some(*key);
                layout
                    .panes
                    .iter()
                    .enumerate()
                    .filter(|(_, pane)| pane.is_view())
                    .map(move |(index, pane)| {
                        let body = view_body(on_desk, layout, index);
                        (
                            pane.instance,
                            (pane.program, window.map(|window| (window, body, in_front))),
                        )
                    })
            })
            .collect();
        self.seats.update(cx, |seats, cx| seats.place(wanted, cx));
    }

    /// What a seat asked of the windows.
    fn answer(&mut self, event: &SeatsEvent, cx: &mut Context<Self>) {
        let Some(windows) = self.windows.upgrade() else {
            return;
        };
        match event {
            // its own window's name, wherever that pane is now (a pane
            // gone from every desk names nothing)
            SeatsEvent::Title(instance, title) => {
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
                            .any(|pane| pane.instance == *instance)
                    });
                if let Some(desk) = desk {
                    desk.update(cx, |desk, cx| desk.set_title(*instance, title.clone(), cx));
                }
            }
            // a window placed before its view came is widened to it
            SeatsEvent::Seated(program) => {
                let desks: Vec<_> = windows
                    .read(cx)
                    .by_window()
                    .values()
                    .map(|own| own.desk.clone())
                    .filter(|desk| desk.read(cx).holds(program))
                    .collect();
                for desk in desks {
                    desk.update(cx, |desk, cx| desk.settle(cx));
                }
            }
            // from this pane: a link into its own program stays in it
            SeatsEvent::OpenLink(program, instance, link) => {
                windows.update(cx, |windows, cx| {
                    windows.open_link(link, Some((program, *instance)), cx)
                });
            }
            // the reader's account: the console bar's account menu
            SeatsEvent::Show(_, _, view_wire::methods::Place::Account) => {
                windows.update(cx, |windows, cx| windows.show_account(cx));
            }
        }
    }
}
