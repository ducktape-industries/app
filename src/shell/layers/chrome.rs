//! The console's chrome, in the layout the prefs pick: the menu bar across
//! the window's top (the network, the programs as tabs, then Search (⌘K),
//! the bell, the node's breath, who is signed in, and Settings), or the
//! sidebar down its left (`sidebar.rs`: the same buttons, and under each
//! program the windows it has open, its pop-outs too); and the menus
//! hanging from its buttons (`menus.rs`, `bell.rs`). A cached view of its
//! own over the slices it reads: a write that moves none of them leaves it
//! alone, and a pane's tick never reaches it. It writes one thing,
//! `DotSlot`: where the node button's 12px well was laid out, read at
//! prepaint and committed after the frame when it moved. What is placed
//! against the chrome reads where it sits off `chrome_inset`.
//!
//! Each open menu hangs beside its button (`footed`), deferred and
//! anchored to the button's corner 4px past the chrome; the backdrop under
//! it (a click on it closes the menu, and presses stay off the desk) is a
//! deferred child of the chrome drawn first. A menu's keys: the arrows step
//! its rows; Tab past its ends, or anything else that takes the keys out of
//! it, closes it and leaves them where they went (`keys_left`; owner,
//! 2026-09-28).

use super::super::entities::{
    Account, Chain, DotSlot, Entities, Front, Listed, Notifications, Observed, Overlay, Overlays,
    Popover, Popped, Prefs, Rail, Session, Slice, WindowEntities, Windows,
};
use super::super::ink::{Ink, mono, sans};
use super::super::{
    PaneMessage, WindowKey, WindowRoot, chord_label, keys, pane_hold, screens, theme,
};
use crate::a11y::Control as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

mod bell;
mod menus;
mod sidebar;

pub(in crate::shell) const BAR: f32 = 36.;
pub(in crate::shell) const SIDEBAR: f32 = 220.;
const SIDEBAR_FOOTER_HEIGHT: f32 = 40.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::shell) struct Inset {
    pub(in crate::shell) top: f32,
    pub(in crate::shell) left: f32,
    pub(in crate::shell) menus: f32,
}

pub(in crate::shell) fn chrome_inset(layout: crate::backend::Layout) -> Inset {
    match layout {
        crate::backend::Layout::MenuBar => Inset {
            top: BAR,
            left: 0.,
            menus: BAR,
        },
        crate::backend::Layout::Sidebar => Inset {
            top: 0.,
            left: SIDEBAR,
            menus: SIDEBAR_FOOTER_HEIGHT,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    Program(&'static str),
    Window(u64),
}

pub(in crate::shell) struct Chrome {
    windows: Entity<Windows>,
    key: WindowKey,
    window: WeakEntity<WindowRoot>,
    session: Observed<Session>,
    chain: Observed<Chain>,
    account: Observed<Account>,
    rail: Observed<Rail>,
    notifications: Observed<Notifications>,
    overlays: Observed<Overlays>,
    front: Observed<Slice<Front>>,
    popped: Observed<Slice<Popped>>,
    prefs: Observed<Slice<Prefs>>,
    dot: Observed<DotSlot>,
    pub(in crate::shell) menu: FocusHandle,
    stop: FocusHandle,
    rail_cursor: Option<Row>,
    hovered: Option<&'static str>,
    rail_scroll: ScrollHandle,
    bar_needs: f32,
    bar_made: u64,
    bar_drawn: Option<f32>,
    ages: Option<Task<()>>,
    _subscriptions: [Subscription; 3],
}

impl Chrome {
    pub(in crate::shell) fn new(
        app: Entities,
        key: WindowKey,
        own: &WindowEntities,
        desk_window: WeakEntity<WindowRoot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (session, chain, account, rail, notifications, prefs) = (
            app.session.clone(),
            app.chain.clone(),
            app.account.clone(),
            app.rail.clone(),
            app.notifications.clone(),
            app.prefs.clone(),
        );
        let popped = app.windows.read(cx).popped().clone();
        let (stop, menu) = (cx.focus_handle().tab_stop(true), cx.focus_handle());
        let subscriptions = [
            cx.on_blur(&stop, window, |this, _, cx| {
                if this.rail_cursor.take().is_some() {
                    cx.notify();
                }
            }),
            cx.on_focus_out(&menu, window, |this, _, window, cx| {
                this.keys_left(window, cx)
            }),
            cx.observe(&own.overlays, |this, overlays, cx| {
                let open = *overlays.read(cx).get();
                this.ages_follow(&open, cx)
            }),
        ];
        let mut this = Self {
            key,
            window: desk_window,
            session: Observed::new(&session, cx),
            chain: Observed::new(&chain, cx),
            account: Observed::new(&account, cx),
            rail: Observed::new(&rail, cx),
            notifications: Observed::new(&notifications, cx),
            overlays: Observed::new(&own.overlays, cx),
            front: Observed::new(&own.front, cx),
            popped: Observed::new(&popped, cx),
            prefs: Observed::new(&prefs, cx),
            dot: Observed::new(&own.dot, cx),
            windows: app.windows,
            menu,
            stop,
            rail_cursor: None,
            hovered: None,
            rail_scroll: ScrollHandle::default(),
            bar_needs: 0.,
            bar_made: 0,
            bar_drawn: None,
            ages: None,
            _subscriptions: subscriptions,
        };
        let open = *own.overlays.read(cx).get();
        this.ages_follow(&open, cx);
        this
    }

    fn keys_left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.contains_focused(window, cx) {
            return;
        }
        let open = match *self.overlays.read(cx).get() {
            Some(open @ (Overlay::Network | Overlay::Menu(_))) => open,
            _ => return,
        };
        self.overlays
            .entity()
            .update(cx, |overlays, cx| overlays.close_by_keys(open, cx));
    }

    fn ages_follow(&mut self, open: &Option<Overlay>, cx: &mut Context<Self>) {
        let counting = matches!(
            open,
            Some(Overlay::Menu(Popover::Node | Popover::Notifications))
        );
        match (counting, self.ages.is_some()) {
            (true, false) => {
                self.ages = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(1))
                            .await;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            break;
                        }
                    }
                }));
            }
            (false, true) => self.ages = None,
            _ => {}
        }
    }

    fn on_notifications(
        &self,
        call: impl Fn(&mut Notifications, &mut Context<Notifications>) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let notifications = self.notifications.entity().clone();
        move |cx| notifications.update(cx, |notifications, cx| call(notifications, cx))
    }

    fn on_windows(
        &self,
        call: impl Fn(&mut Windows, &mut Context<Windows>) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let windows = self.windows.clone();
        move |cx| windows.update(cx, |windows, cx| call(windows, cx))
    }

    fn closing(
        &self,
        menu: Overlay,
        run: impl Fn(&mut App) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let overlays = self.overlays.entity().clone();
        move |cx| {
            overlays.update(cx, |overlays, cx| overlays.close(menu, cx));
            run(cx)
        }
    }

    fn on_session(
        &self,
        call: impl Fn(&mut Session, &mut Context<Session>) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let session = self.session.entity().clone();
        move |cx| session.update(cx, |session, cx| call(session, cx))
    }

    fn on_account(
        &self,
        call: impl Fn(&mut Account, &mut Context<Account>) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let account = self.account.entity().clone();
        move |cx| account.update(cx, |account, cx| call(account, cx))
    }

    fn overlaying(
        &self,
        change: impl Fn(&mut Overlays, &mut Context<Overlays>) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let overlays = self.overlays.entity().clone();
        move |cx| overlays.update(cx, &change)
    }

    fn menu_row(
        &self,
        id: &'static str,
        label: &'static str,
        run: impl Fn(&mut App) + 'static,
        ink: &Ink,
    ) -> Stateful<Div> {
        let surface = ink.surface;
        crate::a11y::keyboard(
            sans(400, 14.)
                .id(id)
                .control(Role::MenuItem, label)
                .h(px(36.))
                .px(px(16.))
                .flex()
                .items_center()
                .justify_between()
                .cursor_pointer()
                .hover(move |style| style.bg(surface))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    run(cx)
                })
                .child(label),
            ink.ink,
        )
    }

    fn inset(&self, cx: &App) -> Inset {
        chrome_inset(self.prefs.read(cx).get().layout_or_default())
    }

    fn menu_room(&self, window: &Window, cx: &App) -> f32 {
        let clear = match self.prefs.read(cx).get().layout_or_default() {
            crate::backend::Layout::MenuBar => 8.,
            crate::backend::Layout::Sidebar => 4. + BAR + 4.,
        };
        f32::from(window.viewport_size().height) - self.inset(cx).menus - clear - 3.
    }

    fn backdrop(&self, id: &'static str, closes: Overlay, window: &Window, cx: &App) -> AnyElement {
        let close = self.overlaying(move |overlays, cx| overlays.close(closes, cx));
        let viewport = window.viewport_size();
        let inset = self.inset(cx);
        deferred(
            div()
                .id(SharedString::from(format!("{id}-backdrop")))
                .absolute()
                .top(px(inset.top))
                .left(px(inset.left))
                .w((viewport.width - px(inset.left)).max(px(0.)))
                .h((viewport.height - px(inset.top)).max(px(0.)))
                .occlude()
                .on_click(move |_, _, cx| close(cx)),
        )
        .with_priority(crate::render::deferred::HOST_BAND + 1)
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments, reason = "one shape, four menus")]
    fn hanging(
        &self,
        id: &'static str,
        role: Role,
        name: &'static str,
        anchor: Anchor,
        width: f32,
        body: impl IntoElement,
        ink: &Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card = div()
            .id(id)
            .control(role, name)
            .occlude()
            .flex()
            .flex_col()
            .bg(ink.bg)
            .text_color(ink.ink)
            .border(px(1.5))
            .border_color(ink.ink)
            .shadow_lg()
            .w(px(width))
            .child(body);
        let menu = self.menu.clone();
        let corner = div()
            .absolute()
            .size_0()
            .track_focus(&self.menu)
            .capture_key_down(move |event: &KeyDownEvent, window, cx| {
                let down = match event.keystroke.key.as_str() {
                    "down" => true,
                    "up" => false,
                    _ => return,
                };
                let step = |window: &mut Window, cx: &mut App, down| match down {
                    true => window.focus_next(cx),
                    false => window.focus_prev(cx),
                };
                cx.stop_propagation();
                step(window, cx, down);
                if !menu.contains_focused(window, cx) {
                    step(window, cx, !down);
                }
            })
            .key_context(keys::MENU)
            .on_action(cx.listener(|this, _: &keys::MenuTab, window, cx| {
                window.focus_next(cx);
                this.keys_left(window, cx);
            }))
            .on_action(cx.listener(|this, _: &keys::MenuTabBack, window, cx| {
                window.focus_prev(cx);
                this.keys_left(window, cx);
            }));
        use crate::backend::Layout;
        let (anchor, corner) = match (self.prefs.read(cx).get().layout_or_default(), anchor) {
            (Layout::MenuBar, Anchor::TopLeft) => (anchor, corner.bottom(px(-5.)).left_0()),
            (Layout::MenuBar, _) => (anchor, corner.bottom(px(-5.)).right_0()),
            (Layout::Sidebar, Anchor::TopLeft) => (anchor, corner.top_0().right(px(-5.))),
            (Layout::Sidebar, _) => (Anchor::BottomLeft, corner.top(px(-5.)).left_0()),
        };
        deferred(
            corner.child(
                anchored()
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            ),
        )
        .with_priority(crate::render::deferred::HOST_BAND + 2)
        .into_any_element()
    }

    fn footed(button: Stateful<Div>, menu: Option<AnyElement>) -> Div {
        div()
            .relative()
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .child(button)
            .children(menu)
    }

    fn hung((button, menu): (Stateful<Div>, Option<AnyElement>)) -> AnyElement {
        match menu {
            Some(menu) => Self::footed(button, Some(menu)).into_any_element(),
            None => button.into_any_element(),
        }
    }

    fn popover(
        &self,
        which: Popover,
        width: f32,
        body: impl IntoElement,
        ink: &Ink,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (id, name) = match which {
            Popover::Node => ("node-status", "Node status"),
            Popover::Account => ("account-menu", "Account"),
            Popover::Notifications => ("notifications", "Notifications"),
        };
        let room = self.menu_room(window, cx);
        let body = div()
            .id(SharedString::from(format!("{id}-body")))
            .max_h(px(room.max(0.)))
            .overflow_y_scroll()
            .child(body);
        self.hanging(
            id,
            Role::Dialog,
            name,
            Anchor::TopRight,
            width,
            body,
            ink,
            cx,
        )
    }
}

struct Bar {
    layout: crate::backend::Layout,
    dark: bool,
    network: String,
    connecting: bool,
    reconnecting: bool,
    height: i64,
    unread: usize,
    unlocked: bool,
    account: Option<Option<(u64, String)>>,
    open: Option<Overlay>,
    focused: Option<&'static str>,
    front: Option<u64>,
    listed: Vec<Listed>,
    rows: Vec<crate::runtime::RailRow>,
}

fn item(
    id: &'static str,
    name: SharedString,
    open: bool,
    popup: Option<accesskit::HasPopup>,
    run: Box<dyn Fn(&mut App)>,
    ink: &Ink,
) -> Stateful<Div> {
    let surface = ink.surface;
    let patch = crate::a11y::Patch::default();
    let patch = match popup {
        Some(popup) => patch.has_popup(popup),
        None => patch,
    };
    patch.on(crate::a11y::keyboard(
        sans(400, 13.)
            .id(id)
            .control(Role::Button, name)
            .h(px(BAR))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .cursor_pointer()
            .text_color(ink.ink)
            .when(open, |item| item.bg(surface))
            .hover(move |style| style.bg(surface))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                run(cx);
            }),
        ink.ink,
    ))
}

fn drag_handle(id: &'static str, titlebar: Option<f32>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .h_full()
        .when(titlebar.is_some(), |strip| {
            strip.on_mouse_down(MouseButton::Left, |event, window, _| {
                match event.click_count {
                    2 => window.titlebar_double_click(),
                    _ => window.start_window_move(),
                }
            })
        })
}

fn accessible_program_name_with_status_and_unread(
    row: &crate::runtime::RailRow,
    badge: i64,
) -> String {
    let shown = tab_label(row);
    let name = match row.note {
        Some(note) => format!("{shown} · {note}"),
        None => shown,
    };
    match badge {
        ..=0 => name,
        count => format!("{name}, {count} unread"),
    }
}

impl Chrome {
    fn toggle(&self, menu: Overlay) -> Box<dyn Fn(&mut App)> {
        Box::new(self.overlaying(move |overlays, cx| overlays.toggle(menu, cx)))
    }

    fn network(
        &self,
        bar: &Bar,
        narrow: bool,
        ink: &Ink,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, Option<AnyElement>) {
        let open = bar.open == Some(Overlay::Network);
        let button = item(
            "network-switcher",
            SharedString::from(format!("Network: {}", bar.network)),
            open,
            Some(accesskit::HasPopup::Menu),
            self.toggle(Overlay::Network),
            ink,
        )
        .aria_expanded(open)
        .child(sans(500, 13.).child(bar.network.clone()))
        .child(div().text_color(ink.muted).child("⌄"));
        (button, open.then(|| self.network_menu(narrow, ink, cx)))
    }

    fn search(&self, narrow: bool, ink: &Ink) -> Stateful<Div> {
        let chord = chord_label("K");
        item(
            "rail-search",
            "Search".into(),
            false,
            Some(accesskit::HasPopup::Dialog),
            Box::new(self.overlaying(|overlays, cx| overlays.open(Overlay::Spotlight, cx))),
            ink,
        )
        .aria_keyshortcuts(chord.clone())
        .when(!narrow, |item| {
            item.child(div().text_color(ink.muted).child("Search"))
        })
        .child(
            mono(400, 12.)
                .text_color(ink.muted)
                .px(px(5.))
                .py(px(1.))
                .border_1()
                .border_color(ink.line)
                .child(chord),
        )
    }

    fn bell(
        &self,
        bar: &Bar,
        ink: &Ink,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, Option<AnyElement>) {
        let unread = bar.unread;
        let open = bar.open == Some(Overlay::Menu(Popover::Notifications));
        let button = item(
            "rail-notifications",
            SharedString::from(match unread {
                0 => "Notifications".to_owned(),
                count => format!("Notifications, {count} unread"),
            }),
            open,
            Some(accesskit::HasPopup::Dialog),
            self.toggle(Overlay::Menu(Popover::Notifications)),
            ink,
        )
        .aria_expanded(open)
        .child(
            div()
                .relative()
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Bell)
                        .size(px(16.))
                        .text_color(ink.muted),
                )
                .when(unread > 0, |bell| {
                    bell.child(
                        crate::a11y::live(
                            mono(500, 9.).id("rail-unread").role(Role::Status),
                            accesskit::Live::Polite,
                            format!("{unread} unread notifications"),
                        )
                        .absolute()
                        .top(px(-5.))
                        .left(px(8.))
                        .min_w(px(14.))
                        .h(px(14.))
                        .px(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(ink.ink)
                        .text_color(ink.bg)
                        .child(match unread {
                            ..=99 => unread.to_string(),
                            _ => "99+".to_owned(),
                        }),
                    )
                }),
        );
        (button, open.then(|| self.bell_menu(ink, window, cx)))
    }

    fn node(
        &self,
        bar: &Bar,
        ink: &Ink,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, Option<AnyElement>) {
        let said = match (bar.connecting, bar.reconnecting) {
            (true, _) => "Node: switching",
            (_, true) => "Node: not answering",
            (false, false) => "Node: in sync",
        };
        let open = bar.open == Some(Overlay::Menu(Popover::Node));
        let well = {
            let dot = self.dot.entity().clone();
            div().size(px(12.)).flex_shrink_0().relative().child(
                canvas(
                    move |bounds, window, cx| {
                        if *dot.read(cx).get() != Some(bounds) {
                            window.on_next_frame(move |_, cx| {
                                dot.update(cx, |slot, cx| {
                                    slot.set(Some(bounds), cx);
                                });
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
        };
        let button = item(
            "rail-connection",
            said.into(),
            open,
            Some(accesskit::HasPopup::Dialog),
            self.toggle(Overlay::Menu(Popover::Node)),
            ink,
        )
        .aria_description(format!("Block {}", bar.height))
        .aria_expanded(open)
        .px(px(12.))
        .child(well);
        (button, open.then(|| self.node_menu(ink, window, cx)))
    }

    fn who(
        &self,
        bar: &Bar,
        narrow: bool,
        ink: &Ink,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, Option<AnyElement>) {
        match (&bar.account, bar.unlocked) {
            (_, false) => (
                item(
                    "sign-in",
                    "Sign in".into(),
                    false,
                    None,
                    Box::new(self.on_account(Account::sign_in)),
                    ink,
                )
                .child(div().underline().child("Sign in")),
                None,
            ),
            (Some(None), true) => (
                item(
                    "rail-account",
                    "Create account".into(),
                    false,
                    None,
                    Box::new(self.on_account(Account::create_account)),
                    ink,
                )
                .child(div().underline().child("Create account")),
                None,
            ),
            (account, true) => {
                let name = match account {
                    Some(Some((_, name))) => name.clone(),
                    _ => "Signed in".to_owned(),
                };
                let shown = match narrow {
                    true => screens::initials(&name),
                    false => name.clone(),
                };
                let open = bar.open == Some(Overlay::Menu(Popover::Account));
                let button = item(
                    "rail-account",
                    SharedString::from(format!("Account: {name}")),
                    open,
                    Some(accesskit::HasPopup::Dialog),
                    self.toggle(Overlay::Menu(Popover::Account)),
                    ink,
                )
                .aria_expanded(open)
                .child(shown);
                (button, open.then(|| self.account_menu(ink, window, cx)))
            }
        }
    }

    fn gear(&self, ink: &Ink) -> Stateful<Div> {
        item(
            "settings",
            "Ducktape settings".into(),
            false,
            Some(accesskit::HasPopup::Dialog),
            Box::new(self.overlaying(|overlays, cx| overlays.open_settings(cx))),
            ink,
        )
        .px(px(8.))
        .child(
            gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Settings)
                .size(px(16.))
                .text_color(ink.muted),
        )
    }

    fn menu_backdrop(&self, bar: &Bar, window: &Window, cx: &App) -> Option<AnyElement> {
        match bar.open {
            Some(Overlay::Network) => {
                Some(self.backdrop("network-menu", Overlay::Network, window, cx))
            }
            Some(Overlay::Menu(which)) => {
                let id = match which {
                    Popover::Node => "node-status",
                    Popover::Account => "account-menu",
                    Popover::Notifications => "notifications",
                };
                Some(self.backdrop(id, Overlay::Menu(which), window, cx))
            }
            _ => None,
        }
    }

    fn opening(&self, row: Stateful<Div>, program: &'static str) -> Stateful<Div> {
        let (click, keys) = (self.window.clone(), self.window.clone());
        row.aria_description(format!(
            "Shift+{} shows it in this window",
            pane_hold::keep_key()
        ))
        .on_click(move |event: &ClickEvent, window, cx| {
            let _ = click.update(cx, |desk, cx| match event.modifiers().shift {
                true => desk.pane_message(PaneMessage::Select(program), window, cx),
                false => desk.open_view(program, window, cx),
            });
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            let stroke = &event.keystroke;
            if stroke.modifiers == Modifiers::shift()
                && matches!(stroke.key.as_str(), "enter" | "space")
            {
                cx.stop_propagation();
                let _ = keys.update(cx, |desk, cx| {
                    desk.pane_message(PaneMessage::Select(program), window, cx)
                });
            }
        })
    }
}

impl Render for Chrome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.chrome", 1);
        let bar = {
            let (session, account, prefs, front, popped) = (
                self.session.read(cx).get(),
                self.account.read(cx).get(),
                self.prefs.read(cx).get(),
                self.front.read(cx).get(),
                self.popped.read(cx).get(),
            );
            Bar {
                layout: prefs.layout_or_default(),
                dark: prefs.dark(),
                network: session.network.clone(),
                connecting: session.connecting,
                reconnecting: session.reconnecting,
                height: self.chain.read(cx).height,
                unread: self.notifications.read(cx).unread(),
                unlocked: !account.signer_key.is_empty(),
                account: account.account.clone(),
                open: *self.overlays.read(cx).get(),
                focused: front.focused,
                front: popped.front.or(front.front),
                listed: [&front.open[..], &popped.open[..]].concat(),
                rows: self.rail.read(cx).rows().to_vec(),
            }
        };
        let ink = Ink::of(bar.dark);
        match bar.layout {
            crate::backend::Layout::MenuBar => self.bar(bar, &ink, window, cx),
            crate::backend::Layout::Sidebar => self.sidebar(bar, &ink, window, cx),
        }
    }
}

impl Chrome {
    fn bar(
        &mut self,
        bar: Bar,
        ink: &Ink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.hovered = None;
        let made_of = {
            use std::hash::{Hash as _, Hasher as _};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bar.network.hash(&mut hasher);
            for row in &bar.rows {
                (row.program, &row.label, row.note, row.empty).hash(&mut hasher);
            }
            (bar.unlocked, &bar.account).hash(&mut hasher);
            hasher.finish()
        };
        let remade = self.bar_made != made_of;
        if remade {
            self.bar_made = made_of;
            self.bar_needs = 0.;
        }
        let width = f32::from(window.viewport_size().width);
        let over = f32::from(self.rail_scroll.max_offset().x);
        if let Some(drawn) = self.bar_drawn
            && !remade
            && over > 0.
            && drawn + over > self.bar_needs
        {
            self.bar_needs = drawn + over;
        }
        let narrow = width < self.bar_needs;
        let drawn = (!narrow).then_some(width);
        if drawn.is_some() && (remade || drawn != self.bar_drawn) {
            let this = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = this.update(cx, |_, cx| cx.notify());
            });
        }
        self.bar_drawn = drawn;

        let stop = self.stop.clone();
        let rows = bar.rows.iter().filter(|row| !row.empty);
        let listed: Vec<&'static str> = rows.clone().map(|row| row.program).collect();
        let active = self
            .rail_cursor
            .into_iter()
            .chain(bar.focused.map(Row::Program))
            .find_map(|at| listed.iter().position(|it| Row::Program(it) == at))
            .unwrap_or_default();
        let tabs: Vec<_> = rows
            .enumerate()
            .map(|(n, row)| {
                let program = row.program;
                let selected = bar.focused == Some(program);
                let badge = self.rail.read(cx).badge(program);
                let shown = tab_label(row);
                let name = accessible_program_name_with_status_and_unread(row, badge);
                let icon = narrow
                    .then(|| folded_icon(row, cx.asset_source().as_ref()))
                    .flatten();
                let shown: AnyElement = match (narrow, icon) {
                    (false, _) => shown.into_any_element(),
                    (true, Some(icon)) => gpui_kit::component::Icon::empty()
                        .path(icon)
                        .size(px(16.))
                        .into_any_element(),
                    (true, None) => shown
                        .chars()
                        .next()
                        .map(String::from)
                        .unwrap_or_default()
                        .into_any_element(),
                };
                let tip = narrow.then(|| SharedString::from(name.clone()));
                let hover = ink.ink;
                let tab = crate::a11y::roving_item(
                    sans(400, 13.).id(SharedString::from(format!("rail/{program}"))),
                    (n == active).then_some(&stop),
                    ink.ink,
                )
                .control(Role::Tab, SharedString::from(name))
                .aria_selected(selected)
                .h(px(BAR))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .cursor_pointer()
                .text_color(
                    match selected || bar.listed.iter().any(|it| it.program == program) {
                        true => ink.ink,
                        false => ink.muted,
                    },
                )
                .hover(move |style| style.text_color(hover));
                self.opening(tab, program)
                    .child(shown)
                    .when_some(tip, |tab, tip| {
                        tab.tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                .build(window, cx)
                        })
                    })
                    .when(row.note == Some("Failed"), |tab| {
                        tab.child(div().size(px(5.)).rounded_full().bg(ink.danger))
                    })
                    .when(badge > 0, |tab| {
                        tab.child(
                            mono(400, 12.)
                                .text_color(ink.muted)
                                .child(badge.to_string()),
                        )
                    })
            })
            .collect();
        let titlebar = theme::traffic_lights(window);
        let moved = cx.entity().downgrade();
        div()
            .id("menubar")
            .role(Role::MenuBar)
            .aria_label("Ducktape")
            .h(px(BAR))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .pl(px(titlebar.unwrap_or(8.)))
            .pr(px(8.))
            .border_b_1()
            .border_color(ink.line)
            .bg(ink.bg)
            .children(self.menu_backdrop(&bar, window, cx))
            .child(Self::hung(self.network(&bar, narrow, ink, cx)))
            .child(div().w(px(1.)).h(px(16.)).mx(px(6.)).bg(ink.line))
            .child(
                crate::a11y::roving(
                    div().id("rail-rows").control(Role::TabList, "Programs"),
                    &stop,
                    accesskit::Orientation::Horizontal,
                    [active, listed.len()],
                    move |to, _, cx| {
                        let program = listed[to];
                        let _ = moved.update(cx, |this, cx| {
                            this.rail_cursor = Some(Row::Program(program));
                            cx.notify();
                        });
                    },
                )
                .min_w_0()
                .flex()
                .overflow_x_scroll()
                .track_scroll(&self.rail_scroll)
                .children(tabs)
                .when(bar.rows.is_empty(), |list| {
                    list.child(
                        sans(400, 13.)
                            .px(px(10.))
                            .text_color(ink.muted)
                            .child("No programs listed yet"),
                    )
                }),
            )
            .child(drag_handle("menubar-handle", titlebar).min_w(px(8.)))
            .child(self.search(narrow, ink))
            .child(Self::hung(self.bell(&bar, ink, window, cx)))
            .child(Self::hung(self.node(&bar, ink, window, cx)))
            .child(Self::hung(self.who(&bar, narrow, ink, window, cx)))
            .child(self.gear(ink))
            .into_any_element()
    }
}

fn folded_icon(row: &crate::runtime::RailRow, assets: &dyn AssetSource) -> Option<SharedString> {
    let bundled = !row.icon.is_empty() && matches!(assets.load(&row.icon), Ok(Some(_)));
    bundled.then(|| row.icon.clone().into())
}

pub(in crate::shell) fn tab_label(row: &crate::runtime::RailRow) -> String {
    match row.note {
        Some(_) => screens::prettify(&row.label),
        None => row.label.clone(),
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::RailRow;

    #[test]
    fn a_folded_tab_is_the_icon_its_view_declares_or_its_initial() {
        let drawn = |icon: &str| {
            let row = RailRow {
                program: "folded-tab",
                label: "Folded".into(),
                icon: icon.into(),
                tint: view_wire::manifest::Tint::None,
                description: String::new(),
                note: None,
                empty: false,
                pending: false,
                prep: crate::runtime::Prep::Settled,
            };
            super::folded_icon(&row, &gpui_kit::assets::AllAssets)
        };
        for icon in [
            "icons/messages-square.svg",
            "icons/hammer.svg",
            "icons/users.svg",
            "icons/circle-user.svg",
            "icons/server.svg",
            "icons/compass.svg",
        ] {
            assert_eq!(drawn(icon), Some(icon.into()), "declared and bundled");
        }
        assert_eq!(drawn(""), None, "none declared");
        assert_eq!(drawn("icons/no-such-icon.svg"), None, "not bundled");
    }
}
