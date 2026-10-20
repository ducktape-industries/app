//! What opens over the console's desk on a scrim: ⌘K's Spotlight
//! (`spotlight.rs`), Settings (`settings.rs`) and "Add a device…"
//! (`approve.rs`), a cached view of its own over the entities it reads. A
//! keystroke in Spotlight's field moves `Spotlight` and draws this layer,
//! and nothing else in the window.
//!
//! Here the keys go where something opening or closing over the desk
//! sends them, whoever draws it (the bar's menus are `Chrome`'s): whatever
//! opens takes them, unless something in it already did (Spotlight's
//! field); one giving way to the next hands them on; the last to close
//! gives them back to what had them before the first opened, unless they
//! left a menu by the keys and closed it. The handoff follows `Overlays`
//! (`moved`); the keys enter what opened after the draw that shows it
//! (`entered`), when its first control is in the frame `focus_next` walks.
//! Also the scrim and card every dialog on it is dressed in (`scrim`).

use super::super::entities::{
    Account, Entities, Notifications, Observed, Overlay, Overlays, Prefs, Rail, Screen, Session,
    Slice, Spot, Spotlight, WindowEntities,
};
use super::super::{WindowKey, theme};
use super::fields::NativeInput;
use super::{BAR, Inset, chrome_inset};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div,
};
use std::collections::HashMap;

mod approve;
mod consent;
mod settings;
mod spotlight;

#[cfg(any(test, feature = "ax-door"))]
pub(crate) use settings::Kept;

struct Shown;

pub(in crate::shell) struct OverlayLayer {
    app: Entities,
    key: WindowKey,
    overlays: Observed<Overlays>,
    spotlight: Observed<Spotlight>,
    rail: Observed<Rail>,
    notifications: Observed<Notifications>,
    session: Observed<Session>,
    account: Observed<Account>,
    prefs: Observed<Slice<Prefs>>,
    screen: Observed<Slice<Screen>>,
    framed: bool,
    field: Entity<InputState>,
    approve_code: NativeInput,
    spotlight_rows: gpui_kit::ScrollHandle,
    settings_rows: settings::Page,
    settings_moved: settings::Moved,
    stops: HashMap<gpui_kit::SharedString, FocusHandle>,
    covered: Option<Overlay>,
    pub(in crate::shell) refocus: Option<FocusHandle>,
    entering: Option<FocusHandle>,
    pub(in crate::shell) modal: FocusHandle,
    menu: FocusHandle,
    card: FocusHandle,
    _subscriptions: [Subscription; 4],
}

impl EventEmitter<Shown> for OverlayLayer {}

impl OverlayLayer {
    pub(in crate::shell) fn new(
        app: Entities,
        key: WindowKey,
        own: &WindowEntities,
        menu: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (rail, notifications, session, account, prefs) = (
            app.rail.clone(),
            app.notifications.clone(),
            app.session.clone(),
            app.account.clone(),
            app.prefs.clone(),
        );
        let field = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search programs, networks, actions")
        });
        let approve_code = NativeInput::new(
            "XXXX-XXXX",
            false,
            |this: &mut Self, _, cx| {
                this.account
                    .entity()
                    .update(cx, |account, cx| account.clear_error(cx));
            },
            |this: &mut Self, cx| {
                let code = this.approve_code.state.read(cx).value().to_string();
                this.account
                    .entity()
                    .update(cx, |account, cx| account.approve_find(code, cx));
            },
            window,
            cx,
        );
        let subscriptions = [
            cx.observe_in(&own.overlays, window, |this, _, window, cx| {
                this.moved(window, cx)
            }),
            cx.observe(&own.desk, |this, desk, cx| {
                let framed = framed(desk.read(cx).get());
                if framed != this.framed {
                    this.framed = framed;
                    cx.notify();
                }
            }),
            cx.subscribe(&field, |this, field, event: &InputEvent, cx| match event {
                InputEvent::PressEnter { .. } => this.submit(cx),
                InputEvent::Change => {
                    let query = field.read(cx).value().to_string();
                    this.spotlight
                        .entity()
                        .update(cx, |spotlight, cx| spotlight.set_query(query, cx));
                }
                _ => {}
            }),
            cx.subscribe_in(&cx.entity(), window, |this, _, _: &Shown, window, cx| {
                this.entered(window, cx)
            }),
        ];
        Self {
            overlays: Observed::new(&own.overlays, cx),
            spotlight: Observed::new(&own.spotlight, cx),
            rail: Observed::new(&rail, cx),
            notifications: Observed::new(&notifications, cx),
            session: Observed::new(&session, cx),
            account: Observed::new(&account, cx),
            prefs: Observed::new(&prefs, cx),
            screen: Observed::new(&app.screen, cx),
            framed: framed(own.desk.read(cx).get()),
            app,
            key,
            field,
            approve_code,
            spotlight_rows: Default::default(),
            settings_rows: Default::default(),
            settings_moved: Default::default(),
            stops: HashMap::new(),
            covered: None,
            refocus: None,
            entering: None,
            modal: cx.focus_handle(),
            menu,
            card: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn inset(&self, window: &Window, cx: &gpui_kit::App) -> Inset {
        match *self.screen.read(cx).get() == Screen::Desk {
            true => chrome_inset(self.prefs.read(cx).get().layout_or_default()),
            false => Inset {
                top: theme::traffic_lights(window).map_or(0., |_| BAR),
                left: 0.,
                menus: 0.,
            },
        }
    }

    fn stop(&mut self, id: &str, cx: &gpui_kit::App) -> FocusHandle {
        self.stops
            .entry(id.to_owned().into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    fn moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = *self.overlays.read(cx).get();
        let was = std::mem::replace(&mut self.covered, open);
        if kind(was) == kind(open) {
            return;
        }
        match (was, open) {
            (None, Some(_)) => self.refocus = window.focused(cx),
            (Some(_), None) => {
                let left = self
                    .overlays
                    .entity()
                    .update(cx, |overlays, _| overlays.take_left_by_keys());
                self.entering = None;
                if let Some(back) = self.refocus.take().filter(|_| !left) {
                    back.focus(window, cx);
                }
            }
            _ => {}
        }
        let Some(open) = open else {
            return;
        };
        if open == Overlay::Approve {
            self.approve_code.wipe(window, cx);
        }
        if open == Overlay::Spotlight {
            self.field.update(cx, |field, cx| {
                field.set_value("", window, cx);
                field.focus(window, cx);
            });
            self.spotlight
                .entity()
                .update(cx, |spotlight, cx| spotlight.set_query(String::new(), cx));
        }
        self.entering = Some(match open {
            Overlay::Network | Overlay::Menu(_) => self.menu.clone(),
            Overlay::Consent(_) => self.card.clone(),
            _ => self.modal.clone(),
        });
    }

    fn entered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(into) = self.entering.take() else {
            return;
        };
        if !into.contains_focused(window, cx) {
            into.focus(window, cx);
            if into != self.card {
                window.focus_next(cx);
            }
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let picked = {
            let spotlight = self.spotlight.read(cx).get();
            spotlight.picked(&self.rows(cx))
        };
        run(self.overlays.entity(), &self.app, picked, cx);
    }

    fn rows(&self, cx: &gpui_kit::App) -> Vec<super::super::entities::SpotRow> {
        let windows = self.app.windows.read(cx);
        let on_top = super::super::windows::can_keep_above(cx)
            .then(|| windows.console().is_some_and(|key| windows.on_top(key)));
        self.spotlight.read(cx).get().rows(
            self.rail.read(cx),
            self.session.read(cx).get(),
            self.account.read(cx).get(),
            self.framed,
            on_top,
        )
    }
}

fn kind(open: Option<Overlay>) -> Option<Overlay> {
    open.map(|open| match open {
        Overlay::Settings(_) => Overlay::Settings(Default::default()),
        open => open,
    })
}

fn framed(layout: &crate::ui::layout::Layout) -> bool {
    layout
        .panes
        .get(layout.focused)
        .is_some_and(|pane| pane.frame.is_some())
}

pub(super) fn run(
    overlays: &Entity<Overlays>,
    app: &Entities,
    picked: Option<Spot>,
    cx: &mut gpui_kit::App,
) {
    let framed_pane = |cx: &gpui_kit::App| {
        let windows = app.windows.read(cx);
        let desk = windows
            .console()
            .and_then(|key| windows.own(key))?
            .desk
            .clone();
        let layout = desk.read(cx).get();
        let index = layout.focused;
        layout.panes.get(index)?.frame.map(|_| (desk, index))
    };
    match overlays.update(cx, |overlays, cx| overlays.submit(picked, cx)) {
        Some(Spot::Settings) => overlays.update(cx, |overlays, cx| overlays.open_settings(cx)),
        Some(Spot::Switch(url)) => app
            .session
            .update(cx, |session, cx| session.switch(url, cx)),
        Some(Spot::OtherNetwork) => app.session.update(cx, |session, cx| session.disconnect(cx)),
        Some(Spot::CreateAccount) => app
            .account
            .update(cx, |account, cx| account.create_account(cx)),
        Some(Spot::Lock) => app.account.update(cx, |account, cx| account.lock(cx)),
        Some(Spot::Open(program)) => app
            .windows
            .update(cx, |windows, cx| windows.select_view(program, cx)),
        Some(Spot::Appearance(mode)) => app
            .prefs
            .update(cx, |prefs, cx| prefs.set_appearance(mode, cx)),
        Some(Spot::Help) => app.windows.update(cx, |windows, cx| windows.help_asked(cx)),
        Some(Spot::FillWindow) => {
            if let Some((desk, _)) = framed_pane(cx) {
                desk.update(cx, |desk, cx| desk.fill(cx));
            }
        }
        Some(Spot::HoldWindow) => {
            if let Some((desk, index)) = framed_pane(cx) {
                desk.update(cx, |desk, cx| desk.hold(index, cx));
            }
        }
        Some(Spot::KeepOnTop) => app.windows.update(cx, |windows, cx| {
            if let Some(key) = windows.console() {
                windows.toggle_on_top(key, cx);
            }
        }),
        None => {}
    }
}

impl Render for OverlayLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.overlays", 1);
        if self.entering.is_some() {
            cx.emit(Shown);
        }
        let dialog = match *self.overlays.read(cx).get() {
            Some(Overlay::Spotlight) => Some(self.spotlight(window, cx)),
            Some(Overlay::Settings(page)) => Some(self.settings(page, window, cx)),
            Some(Overlay::Approve) => Some(self.approve(window, cx)),
            Some(Overlay::Consent(ask)) => Some(self.consent(ask, window, cx)),
            _ => None,
        };
        div().absolute().inset_0().size_full().children(dialog)
    }
}

const DIALOG_EDGE: f32 = 12.;

pub(in crate::shell) fn dialog_fit(high: f32, inset: Inset, tall: f32, top: f32) -> (f32, f32) {
    let room = high - inset.top;
    let top = (room - tall - DIALOG_EDGE).clamp(DIALOG_EDGE, top);
    (top, (room - top - DIALOG_EDGE).max(0.))
}

#[allow(clippy::too_many_arguments, reason = "one scrim, every dialog")]
pub(in crate::shell) fn scrim(
    id: impl Into<gpui_kit::SharedString>,
    role: gpui_kit::Role,
    name: &'static str,
    closes: Overlay,
    overlays: &Entity<Overlays>,
    modal: &FocusHandle,
    inset: Inset,
    ink: &super::super::ink::Ink,
    dress: impl FnOnce(gpui_kit::Stateful<gpui_kit::Div>) -> gpui_kit::AnyElement,
) -> gpui_kit::AnyElement {
    use crate::a11y::Control as _;
    use gpui_kit::component::FocusTrapElement as _;
    use gpui_kit::*;
    let id = id.into();
    let backdrop_id = SharedString::from(format!("{id}-backdrop"));
    let overlays = overlays.clone();
    let backdrop = div()
        .id(backdrop_id.clone())
        .absolute()
        .top(px(inset.top))
        .left(px(inset.left))
        .right_0()
        .bottom_0()
        .occlude()
        .bg(ink.bg.opacity(0.6))
        .flex()
        .justify_center()
        .px(px(DIALOG_EDGE))
        .on_click(move |_, _, cx| {
            overlays.update(cx, |overlays, cx| overlays.close(closes, cx));
        });
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
        .shadow_lg();
    backdrop
        .child(dress(crate::a11y::modal(card)))
        .focus_trap(backdrop_id, modal)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dialog_rises_then_shrinks_to_stay_inside_a_short_window() {
        use crate::backend::Layout;
        let bar = chrome_inset(Layout::MenuBar);
        assert_eq!(dialog_fit(800., bar, 680., 74.), (72., 680.));
        assert_eq!(dialog_fit(1000., bar, 680., 74.), (74., 878.));
        let (top, tall) = dialog_fit(480., bar, 680., 74.);
        assert_eq!(top, 12.);
        assert_eq!(bar.top + top + tall + 12., 480.);
        let side = chrome_inset(Layout::Sidebar);
        assert_eq!(dialog_fit(800., side, 680., 74.), (74., 714.));
        let (top, tall) = dialog_fit(480., side, 680., 74.);
        assert_eq!(top + tall + 12., 480.);
    }
}
