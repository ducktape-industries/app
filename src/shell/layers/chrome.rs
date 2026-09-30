//! The console's chrome: the menu bar across the window's top (the
//! network, the programs as tabs, then Search (⌘K), the bell, the node's
//! breath, who is signed in, and Settings), and the menus hanging from its
//! buttons (`menus.rs`, `bell.rs`). A cached view of its own over the
//! slices it reads: a beat that moves none of them leaves it alone, and a
//! pane's tick never reaches it. It writes one thing, `DotSlot`: where the
//! node button's 12px well was laid out, read at prepaint and committed
//! after the frame when it moved.
//!
//! Each open menu hangs beside its bar button (`footed`), deferred and
//! anchored to the button's corner 4px under the bar; the backdrop under it (a click on it
//! closes the menu, and presses stay off the desk) is a deferred child of
//! the bar drawn first. A menu's keys: the arrows step its rows; Tab past
//! its ends, or anything else that takes the keys out of it, closes it and
//! leaves them where they went (`keys_left`; owner, 2026-09-28).

use super::super::entities::{
    Account, Chain, DotSlot, Front, Notifications, Observed, Overlay, Overlays, Popover, Prefs,
    Rail, Session, Slice, WindowEntities,
};
use super::super::ink::{Ink, mono, sans};
use super::super::{
    Desktop, Message, PaneMessage, WindowKey, WindowRoot, chord_label, keys, pane_hold, screens,
    theme,
};
use crate::a11y::Control as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

mod bell;
mod menus;

/// The menu bar's height.
pub(in crate::shell) const BAR: f32 = 36.;

/// The menu bar and its menus, one per console window.
pub(in crate::shell) struct Chrome {
    /// The reducer, until the arms the rows dispatch move (s9-s11).
    model: Entity<Desktop>,
    key: WindowKey,
    /// Its window: a tab's click opens the program through it.
    window: WeakEntity<WindowRoot>,
    session: Observed<Session>,
    chain: Observed<Chain>,
    account: Observed<Account>,
    rail: Observed<Rail>,
    notifications: Observed<Notifications>,
    overlays: Observed<Overlays>,
    front: Observed<Slice<Front>>,
    prefs: Observed<Slice<Prefs>>,
    dot: Observed<DotSlot>,
    /// The box the open menu's card hangs from holds the keys by this; the
    /// overlay layer puts them in when the menu opens (`OverlayLayer::moved`).
    pub(in crate::shell) menu: FocusHandle,
    /// The Programs rail's one Tab stop (`a11y::roving`).
    stop: FocusHandle,
    /// The program tab the arrows moved to while the rail has the keys:
    /// Return opens it. Gone when the keys leave the rail.
    rail_cursor: Option<&'static str>,
    /// The bar's program tabs: how far past their strip they ran.
    rail_scroll: ScrollHandle,
    /// The window width the bar's full words need; narrower, it folds.
    bar_needs: f32,
    /// What `bar_needs` was measured over: the network, the tabs, who is
    /// signed in. Changed, the bar is measured again.
    bar_made: u64,
    /// The width the bar was last drawn at unfolded; `None` while folded.
    bar_drawn: Option<f32>,
    /// One second at a time while a menu that shows ages (the node's, the
    /// bell's) is open: each tick draws the menu again.
    ages: Option<Task<()>>,
    _subscriptions: [Subscription; 3],
}

impl Chrome {
    pub(in crate::shell) fn new(
        model: Entity<Desktop>,
        key: WindowKey,
        own: &WindowEntities,
        desk_window: WeakEntity<WindowRoot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let entities = &model.read(cx).entities;
        let (session, chain, account, rail, notifications, prefs) = (
            entities.session.clone(),
            entities.chain.clone(),
            entities.account.clone(),
            entities.rail.clone(),
            entities.notifications.clone(),
            entities.prefs.clone(),
        );
        let (stop, menu) = (cx.focus_handle().tab_stop(true), cx.focus_handle());
        let subscriptions = [
            // the rail's cursor lasts as long as the rail has the keys
            cx.on_blur(&stop, window, |this, _, cx| {
                if this.rail_cursor.take().is_some() {
                    cx.notify();
                }
            }),
            // the belt: the keys left the menu's subtree, however they did
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
            prefs: Observed::new(&prefs, cx),
            dot: Observed::new(&own.dot, cx),
            model,
            menu,
            stop,
            rail_cursor: None,
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

    /// The keys are outside the open menu: it closes, and they stay where
    /// they went (`Overlays::close_by_keys`, now: the audit's walk and the
    /// door read the window in the same update, and an event would land
    /// after them). A no-op while they are in it (a row that went while the
    /// menu stays put them back at its first control, `focus_lost`), and
    /// when nothing hanging from the bar is open (a normal close moved them
    /// too; the bell's Settings row opened Settings, which stays).
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

    /// The ages clock runs while the node menu or the bell is open, and
    /// only then.
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

    fn dispatching(&self, message: fn() -> Message) -> impl Fn(&mut App) + 'static {
        let model = self.model.clone();
        move |cx| model.update(cx, |model, cx| model.dispatch(message(), cx))
    }

    /// A row of the menu `menu` that is done with it: the menu closes,
    /// then `run` runs.
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

    /// What a press does: `call` on the session.
    fn on_session(
        &self,
        call: impl Fn(&mut Session, &mut Context<Session>) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let session = self.session.entity().clone();
        move |cx| session.update(cx, |session, cx| call(session, cx))
    }

    /// What a press does: `call` on the account.
    fn on_account(
        &self,
        call: impl Fn(&mut Account, &mut Context<Account>) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let account = self.account.entity().clone();
        move |cx| account.update(cx, |account, cx| call(account, cx))
    }

    /// What is open over the desk, moved by `change`.
    fn overlaying(
        &self,
        change: impl Fn(&mut Overlays, &mut Context<Overlays>) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let overlays = self.overlays.entity().clone();
        move |cx| overlays.update(cx, &change)
    }

    /// A menu item: `height: 36px; padding: 0 16px; font: 400 14px`.
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

    /// What is under an open menu: it keeps presses off the desk, and a
    /// click on it closes the menu (as a click on the bar itself does not:
    /// the backdrop starts under the bar). Drawn first among the deferred
    /// children, so the menu's card lies over it. Invisible, and nothing
    /// about focus or keys: the box its card hangs from holds those.
    fn backdrop(&self, id: &'static str, closes: Overlay, window: &Window) -> AnyElement {
        let close = self.overlaying(move |overlays, cx| overlays.close(closes, cx));
        let viewport = window.viewport_size();
        deferred(
            div()
                .id(SharedString::from(format!("{id}-backdrop")))
                .absolute()
                .top(px(BAR))
                .left_0()
                .w(viewport.width)
                .h((viewport.height - px(BAR)).max(px(0.)))
                .occlude()
                .on_click(move |_, _, cx| close(cx)),
        )
        .with_priority(0)
        .into_any_element()
    }

    /// A menu hanging beside the bar button that opened it (`footed`): its
    /// card (the canvas's `border: 1.5px solid ink` and a soft shadow,
    /// `width` wide), `anchor` (a top corner) at the same corner of the
    /// button, 4px below the bar (`top: 40px`), shifted back inside the
    /// window when it would run off it. Deferred over the backdrop. The box
    /// the card hangs from holds the keys by `menu`: its rows step with the
    /// arrows and stop at its ends; Tab and Shift+Tab move on, and past its
    /// ends they close it (`keys_left`; `keys::MenuTab`, bound over the
    /// kit's Tab under the `menu` context). Escape closes it through
    /// `keys::CloseOverlay` (bound under the `overlay` context). A press
    /// inside stays inside: occluded, the backdrop is not under the pointer.
    /// No `on_click` on the card: that would offer a press on the dialog.
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
        // the corner: a point-sized box 5px under the button's foot
        // (`Chrome::footed`, whose bottom is the bar's inside edge at 35)
        // that the anchored card hangs from, so its top lands on 40 (the
        // canvas's menus: `top: 40px`). It holds the menu handle and hears
        // the rows' keys (no id, no role: the card is the node assistive
        // technology sees, and it offers no focus of its own)
        let menu = self.menu.clone();
        let corner = div()
            .absolute()
            .bottom(px(-5.))
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
            // the kit's Tab would move the keys and leave the menu open
            // behind them: the menu's own moves them and then asks
            .key_context(keys::MENU)
            .on_action(cx.listener(|this, _: &keys::MenuTab, window, cx| {
                window.focus_next(cx);
                this.keys_left(window, cx);
            }))
            .on_action(cx.listener(|this, _: &keys::MenuTabBack, window, cx| {
                window.focus_prev(cx);
                this.keys_left(window, cx);
            }));
        let corner = match anchor {
            Anchor::TopLeft => corner.left_0(),
            _ => corner.right_0(),
        };
        deferred(
            corner.child(
                anchored()
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// A bar button with its open menu beside it, in a box the bar's inside
    /// height (35px above the border line). The 36px button is centred in
    /// the box as it was in the bar, half a pixel up, so it is drawn where
    /// it was; the menu's corner hangs from the box, whose edges lie on
    /// whole pixels. Layout snaps each edge to the device grid where it lies
    /// (gpui's `layout_bounds`), so a card laid out half a pixel off has
    /// some of its rows land a pixel from where the same card on a whole
    /// pixel has them. The corner hangs 5px under the box, so the card is
    /// laid out on a whole pixel.
    fn footed(button: Stateful<Div>, menu: AnyElement) -> AnyElement {
        div()
            .relative()
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .child(button)
            .child(menu)
            .into_any_element()
    }

    /// A menu hanging below the bar, its right edge under its button's
    /// (`hanging`), scrolling in a short window rather than being cut off.
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
        // (the card's 1.5px border above and below it)
        let room = f32::from(window.viewport_size().height) - BAR - 8. - 3.;
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

/// What the bar is drawn from, read off the slices at the top of a render.
struct Bar {
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
    on_desk: Vec<&'static str>,
    rows: Vec<crate::runtime::RailRow>,
}

impl Render for Chrome {
    /// The menu bar (the Menubar board): `height: 36px; padding: 0 8px;
    /// border-bottom: 1px solid line`; every item `height: 36px; padding: 0
    /// 10px; gap: 8px; font: 400 13px`, on `surface` while its menu is open.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.chrome", 1);
        let bar = {
            let (session, account, prefs, front) = (
                self.session.read(cx).get(),
                self.account.read(cx).get(),
                self.prefs.read(cx).get(),
                self.front.read(cx).get(),
            );
            Bar {
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
                on_desk: front.open.clone(),
                rows: self.rail.read(cx).rows().to_vec(),
            }
        };
        let ink = Ink::of(bar.dark);
        // Folding. The tabs show their full labels until they overflow their
        // strip; then every tab folds to its icon or initial, so none is cut.
        // `bar_needs` is the narrowest window the full labels are known to
        // need: the width the bar was last drawn unfolded at, plus how far
        // the strip overflowed there (`rail_scroll.max_offset()`). Layout
        // reports that overflow one frame late, so a bar drawn unfolded at a
        // new width asks for one more frame to be measured in. `bar_needs`
        // only grows while the bar shows the same words. New words (the
        // network, the tab list, the sign-in state: `bar_made` hashes them)
        // reset it to measure again, and the overflow the old words left is
        // skipped on that frame. Badge counts stay out of the hash: they
        // tick while folded, and a re-measure draws the bar whole for a frame.
        let made_of = {
            use std::hash::{Hash as _, Hasher as _};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bar.network.hash(&mut hasher);
            for row in &bar.rows {
                (row.module, &row.label, row.note, row.empty).hash(&mut hasher);
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
        // last frame's overflow: the old words', on the frame that remade the bar
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
        // drawn whole at a new width or of new words: the next frame measures
        // it, so ask for one that draws this view again (a frame alone would
        // find it cached)
        if drawn.is_some() && (remade || drawn != self.bar_drawn) {
            let this = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = this.update(cx, |_, cx| cx.notify());
            });
        }
        self.bar_drawn = drawn;

        // one Tab stop: the tab the arrows moved to while the rail has the
        // keys, else the focused window's program, else the first
        let stop = self.stop.clone();
        let rows = bar.rows.iter().filter(|row| !row.empty);
        let listed: Vec<&'static str> = rows.clone().map(|row| row.module).collect();
        let active = self
            .rail_cursor
            .into_iter()
            .chain(bar.focused)
            .find_map(|module| listed.iter().position(|it| *it == module))
            .unwrap_or_default();
        let desk_window = self.window.clone();
        let tabs: Vec<_> = rows
            .enumerate()
            .map(|(n, row)| {
                let module = row.module;
                let selected = bar.focused == Some(module);
                let badge = self.rail.read(cx).badge(module);
                let shown = tab_label(row);
                let name = match row.note {
                    Some(note) => format!("{shown} · {note}"),
                    None => shown.clone(),
                };
                // the count the tab shows is in what it is called
                let name = match badge {
                    ..=0 => name,
                    count => format!("{name}, {count} unread"),
                };
                // folded: the program's icon, its initial when it has none,
                // and its whole name on hover
                let shown: AnyElement = match (narrow, tab_icon(module)) {
                    (false, _) => shown.into_any_element(),
                    (true, Some(icon)) => gpui_kit::component::Icon::new(icon)
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
                let (click, keys) = (desk_window.clone(), desk_window.clone());
                crate::a11y::roving_item(
                    sans(400, 13.).id(SharedString::from(format!("rail/{module}"))),
                    (n == active).then_some(&stop),
                    ink.ink,
                )
                .control(Role::Tab, SharedString::from(name))
                .aria_selected(selected)
                .aria_description(format!(
                    "Shift+{} shows it in this window",
                    pane_hold::keep_key()
                ))
                .h(px(BAR))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .cursor_pointer()
                .text_color(match selected || bar.on_desk.contains(&module) {
                    true => ink.ink,
                    false => ink.muted,
                })
                .hover(move |style| style.text_color(hover))
                // a click opens it (into an empty focused window, or its
                // own); shift-click shows it in the focused window instead
                .on_click(move |event: &ClickEvent, window, cx| {
                    let _ = click.update(cx, |desk, cx| match event.modifiers().shift {
                        true => desk.pane_message(PaneMessage::Select(module), window, cx),
                        false => desk.open_view(module, window, cx),
                    });
                })
                // the keyboard's shift-click: a press with Shift is no click
                .on_key_down(move |event: &KeyDownEvent, window, cx| {
                    let stroke = &event.keystroke;
                    if stroke.modifiers == Modifiers::shift()
                        && matches!(stroke.key.as_str(), "enter" | "space")
                    {
                        cx.stop_propagation();
                        let _ = keys.update(cx, |desk, cx| {
                            desk.pane_message(PaneMessage::Select(module), window, cx)
                        });
                    }
                })
                .child(shown)
                .when_some(tip, |tab, tip| {
                    tab.tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
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
        // `popup`: what its press opens, which it says (AX-113)
        let item = |id: &'static str,
                    name: SharedString,
                    open: bool,
                    popup: Option<accesskit::HasPopup>,
                    run: Box<dyn Fn(&mut App)>| {
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
        };
        let toggle = |menu: Overlay| -> Box<dyn Fn(&mut App)> {
            Box::new(self.overlaying(move |overlays, cx| overlays.toggle(menu, cx)))
        };
        use accesskit::HasPopup::{Dialog, Menu};
        let network_open = bar.open == Some(Overlay::Network);
        let network = item(
            "network-switcher",
            SharedString::from(format!("Network: {}", bar.network)),
            network_open,
            Some(Menu),
            toggle(Overlay::Network),
        )
        .aria_expanded(network_open)
        .child(sans(500, 13.).child(bar.network.clone()))
        .child(div().text_color(ink.muted).child("⌄"));
        let network = match network_open {
            true => Self::footed(network, self.network_menu(narrow, &ink, cx)),
            false => network.into_any_element(),
        };
        let chord = chord_label("K");
        let search = item(
            "rail-search",
            "Search".into(),
            false,
            Some(Dialog),
            Box::new(self.overlaying(|overlays, cx| overlays.open(Overlay::Spotlight, cx))),
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
        );
        let unread = bar.unread;
        let bell_open = bar.open == Some(Overlay::Menu(Popover::Notifications));
        let bell = item(
            "rail-notifications",
            SharedString::from(match unread {
                0 => "Notifications".to_owned(),
                count => format!("Notifications, {count} unread"),
            }),
            bell_open,
            Some(Dialog),
            toggle(Overlay::Menu(Popover::Notifications)),
        )
        .aria_expanded(bell_open)
        .child(
            div()
                .relative()
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Bell)
                        .size(px(16.))
                        .text_color(ink.muted),
                )
                // the count, announced as notices land
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
        let bell = match bell_open {
            true => Self::footed(bell, self.bell_menu(&ink, window, cx)),
            false => bell.into_any_element(),
        };
        // the height moves every block: it is the description, so the
        // name holds still
        let said = match (bar.connecting, bar.reconnecting) {
            (true, _) => "Node: switching",
            (_, true) => "Node: not answering",
            (false, false) => "Node: in sync",
        };
        let node_open = bar.open == Some(Overlay::Menu(Popover::Node));
        // the 12px well the breath sits in, empty: `layers::StatusDot`
        // draws the breath over it. Where it is laid out is the dot's
        // slot, read at prepaint and, when it moved, committed after the
        // frame (a settled bar asks for nothing)
        let well = {
            let dot = self.dot.entity().clone();
            div().size(px(12.)).flex_shrink_0().relative().child(
                canvas(
                    move |bounds, window, cx| {
                        if *dot.read(cx).get() != Some(bounds) {
                            let dot = dot.clone();
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
        let node = item(
            "rail-connection",
            said.into(),
            node_open,
            Some(Dialog),
            toggle(Overlay::Menu(Popover::Node)),
        )
        .aria_description(format!("Block {}", bar.height))
        .aria_expanded(node_open)
        .px(px(12.))
        .child(well);
        let node = match node_open {
            true => Self::footed(node, self.node_menu(&ink, window, cx)),
            false => node.into_any_element(),
        };
        let account_open = bar.open == Some(Overlay::Menu(Popover::Account));
        let who = match (&bar.account, bar.unlocked) {
            (_, false) => item(
                "sign-in",
                "Sign in".into(),
                false,
                None,
                Box::new(self.on_account(Account::sign_in)),
            )
            .child(div().underline().child("Sign in"))
            .into_any_element(),
            // named by what it says
            (Some(None), true) => item(
                "rail-account",
                "Create account".into(),
                false,
                None,
                Box::new(self.on_account(Account::create_account)),
            )
            .child(div().underline().child("Create account"))
            .into_any_element(),
            (account, true) => {
                let name = match account {
                    Some(Some((_, name))) => name.clone(),
                    _ => "Signed in".to_owned(),
                };
                let shown = match narrow {
                    true => screens::initials(&name),
                    false => name.clone(),
                };
                let who = item(
                    "rail-account",
                    SharedString::from(format!("Account: {name}")),
                    account_open,
                    Some(Dialog),
                    toggle(Overlay::Menu(Popover::Account)),
                )
                .aria_expanded(account_open)
                .child(shown);
                match account_open {
                    true => Self::footed(who, self.account_menu(&ink, window, cx)),
                    false => who.into_any_element(),
                }
            }
        };
        // Settings are the app's, not the account's: their own spot at the
        // edge.
        let gear = item(
            "settings",
            "Ducktape settings".into(),
            false,
            Some(Dialog),
            Box::new(self.overlaying(|overlays, cx| overlays.open_settings(cx))),
        )
        .px(px(8.))
        .child(
            gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Settings)
                .size(px(16.))
                .text_color(ink.muted),
        );
        // macOS draws the traffic lights over the bar's left end, and the
        // bar is the window's handle: its empty middle moves the window.
        let titlebar = theme::traffic_lights(window);
        let handle = div()
            .id("menubar-handle")
            .flex_1()
            .min_w(px(8.))
            .h_full()
            .when(titlebar.is_some(), |strip| {
                strip.on_mouse_down(MouseButton::Left, |event, window, _| {
                    match event.click_count {
                        2 => window.titlebar_double_click(),
                        _ => window.start_window_move(),
                    }
                })
            });
        // the backdrop under whichever menu is open, drawn before its card
        let backdrop = match bar.open {
            Some(Overlay::Network) => Some(self.backdrop("network-menu", Overlay::Network, window)),
            Some(Overlay::Menu(which)) => {
                let id = match which {
                    Popover::Node => "node-status",
                    Popover::Account => "account-menu",
                    Popover::Notifications => "notifications",
                };
                Some(self.backdrop(id, Overlay::Menu(which), window))
            }
            _ => None,
        };
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
            .children(backdrop)
            .child(network)
            .child(div().w(px(1.)).h(px(16.)).mx(px(6.)).bg(ink.line))
            .child(
                crate::a11y::roving(
                    div().id("rail-rows").control(Role::TabList, "Programs"),
                    &stop,
                    accesskit::Orientation::Horizontal,
                    [active, listed.len()],
                    // the arrows move the keys; Return opens the program
                    move |to, _, cx| {
                        let module = listed[to];
                        let _ = moved.update(cx, |this, cx| {
                            this.rail_cursor = Some(module);
                            cx.notify();
                        });
                    },
                )
                .min_w_0()
                .flex()
                // folded and still too many: they scroll
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
            .child(handle)
            .child(search)
            .child(bell)
            .child(node)
            .child(who)
            .child(gear)
    }
}

/// A program's icon on a folded bar, by its id (Members is `identity`,
/// Account `module-registry`, Nodes `valset`); `None` folds to its initial.
fn tab_icon(module: &str) -> Option<gpui_kit::assets::IconName> {
    use gpui_kit::assets::IconName;
    Some(match module {
        "chat" => IconName::MessagesSquare,
        "forge" => IconName::Hammer,
        "identity" => IconName::Users,
        "module-registry" => IconName::CircleUser,
        "valset" => IconName::Server,
        "explorer" => IconName::Compass,
        _ => return None,
    })
}

/// A program's name on the bar. Before the manifest lands (or if it never
/// does) the label is the program's own id, prettified.
pub(in crate::shell) fn tab_label(row: &crate::runtime::RailRow) -> String {
    match row.note {
        Some(_) => screens::prettify(&row.label),
        None => row.label.clone(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_folded_tab_is_its_programs_icon_or_its_initial() {
        for module in [
            "chat",
            "forge",
            "identity",
            "module-registry",
            "valset",
            "explorer",
        ] {
            assert!(super::tab_icon(module).is_some(), "{module}");
        }
        assert!(super::tab_icon("ledger").is_none());
    }
}
