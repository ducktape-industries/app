//! The desktop's half of the seats: which view panes exist, and in which
//! window, is the windows' desks' to say. `DeskSeats` follows `Windows`
//! and every window's `Desk`, tells the shared `Seats` where each pane's
//! seat is placed (`Seats::place`: its window, floated by its view or
//! not, kept on top or not), and answers what a seat asks of the
//! windows (`SeatsEvent`): a title to the desk holding its pane, a view
//! seated to the desks holding it, a link or a float to `Windows`.
use super::{Desk, Seats, Windows};
use crate::runtime::{Placed, WindowKey};
use crate::shell::WindowKind;
use crate::shell::layers::view_body;
use crate::ui::layout::Layout;
use ducktape_host::session::{Place, SeatsEvent};
use gpui_kit::{App, Context, Entity, Subscription, WeakEntity, px, size};
use std::collections::BTreeMap;

pub(crate) struct DeskSeats {
    seats: Entity<Seats>,
    windows: WeakEntity<Windows>,
    desks: BTreeMap<WindowKey, Subscription>,
    _observing: Vec<Subscription>,
}

impl DeskSeats {
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

    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let Some(windows) = self.windows.upgrade() else {
            return;
        };
        let keeps_above = crate::shell::windows::can_keep_above(cx);
        let (layouts, handles, front) = {
            let windows = windows.read(cx);
            let layouts: BTreeMap<WindowKey, (Layout, bool, Placed)> = windows
                .by_window()
                .iter()
                .map(|(key, own)| {
                    let on_desk = own.kind == WindowKind::Console;
                    let placed = Placed {
                        float_min: windows.float_min(*key),
                        on_top: windows.on_top(*key),
                        keeps_above,
                    };
                    (*key, (own.desk.read(cx).get().clone(), on_desk, placed))
                })
                .collect();
            (layouts, windows.handles().clone(), windows.front())
        };
        let wanted: BTreeMap<u64, Place> = layouts
            .iter()
            .flat_map(|(key, (layout, on_desk, placed))| {
                let window = handles.get(key).copied();
                let (on_desk, placed) = (*on_desk, *placed);
                let in_front = front == Some(*key);
                layout
                    .panes
                    .iter()
                    .enumerate()
                    .filter(|(_, pane)| pane.is_view())
                    .map(move |(index, pane)| {
                        let body = view_body(on_desk, layout, index);
                        let window = window.map(|window| (window, body, in_front));
                        (pane.instance, (pane.program, window, placed))
                    })
            })
            .collect();
        self.seats.update(cx, |seats, cx| seats.place(wanted, cx));
    }

    fn answer(&mut self, event: &SeatsEvent, cx: &mut Context<Self>) {
        let Some(windows) = self.windows.upgrade() else {
            return;
        };
        match event {
            SeatsEvent::Title(instance, title) => {
                if let Some(desk) = holding(&windows, *instance, cx) {
                    desk.update(cx, |desk, cx| desk.set_title(*instance, title.clone(), cx));
                }
            }
            SeatsEvent::Showing(instance, route) => {
                if let Some(desk) = holding(&windows, *instance, cx) {
                    desk.update(cx, |desk, cx| {
                        desk.set_showing(*instance, route.clone(), cx)
                    });
                }
            }
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
            SeatsEvent::OpenLink(program, instance, link) => {
                windows.update(cx, |windows, cx| {
                    windows.open_link(link, Some((program, *instance)), cx)
                });
            }
            SeatsEvent::OpenWindow(program, instance, link) => {
                windows.update(cx, |windows, cx| {
                    windows.open_link_window(link, Some((program, *instance)), cx)
                });
            }
            SeatsEvent::Show(_, _, view_wire::methods::Place::Account) => {
                windows.update(cx, |windows, cx| windows.show_account(cx));
            }
            SeatsEvent::Show(
                _,
                _,
                place @ (view_wire::methods::Place::CameraSettings
                | view_wire::methods::Place::MicrophoneSettings),
            ) => {
                if let Some(page) = crate::render::av::settings_page(*place) {
                    cx.open_url(page);
                }
            }
            SeatsEvent::Float(instance, float) => {
                let body = size(px(float.width as f32), px(float.height as f32));
                let min = size(px(float.min_width as f32), px(float.min_height as f32));
                windows.update(cx, |windows, cx| {
                    windows.float(*instance, body, min, float.on_top, cx)
                });
            }
            SeatsEvent::Unfloat(instance) => {
                windows.update(cx, |windows, cx| windows.unfloat(*instance, cx));
            }
        }
    }
}

fn holding(windows: &Entity<Windows>, instance: u64, cx: &App) -> Option<Entity<Desk>> {
    windows
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
        })
}
