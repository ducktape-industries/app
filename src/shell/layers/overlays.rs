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

use super::super::WindowKey;
use super::super::entities::{
    Account, Entities, Notifications, Observed, Overlay, Overlays, Prefs, Rail, Session, Slice,
    Spot, Spotlight, WindowEntities,
};
use super::BAR;
use super::fields::NativeInput;
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

/// The layer drew what opened: the keys go into it now.
struct Shown;

/// The dialogs on a scrim over one console's desk.
pub(in crate::shell) struct OverlayLayer {
    /// The app's entities: what a Spotlight row or a Settings switch calls.
    app: Entities,
    key: WindowKey,
    overlays: Observed<Overlays>,
    spotlight: Observed<Slice<Spotlight>>,
    rail: Observed<Rail>,
    notifications: Observed<Notifications>,
    session: Observed<Session>,
    account: Observed<Account>,
    prefs: Observed<Slice<Prefs>>,
    /// The window in front on the desk has a frame: Spotlight offers to fill
    /// or move it. Read off the window's `Desk` by an observer that tells
    /// this layer only when it turns, never on a drag's frames.
    framed: bool,
    /// Spotlight's field: it owns what is typed, and each change writes
    /// `Spotlight.query`.
    field: Entity<InputState>,
    /// "Add a device…"'s code: the field owns it, and it goes in the
    /// `Account::approve_find` call; emptied as the dialog opens.
    approve_code: NativeInput,
    /// Spotlight's list: ↑↓ scroll the picked row into it.
    spotlight_rows: gpui_kit::ScrollHandle,
    /// Settings' page: the row holding the keys scrolls into view.
    settings_rows: settings::Page,
    /// The one Tab stop of each tab list and radio group drawn here, by
    /// the composite's id: its active item tracks it (`a11y::roving`).
    stops: HashMap<gpui_kit::SharedString, FocusHandle>,
    /// What was open over the desk when the handoff last looked.
    covered: Option<Overlay>,
    /// What had the keys when something opened over the desk: they go back
    /// to it when it closes, so typing carries on where it was. Not when
    /// they left a menu, which closed it: they stay where they went.
    pub(in crate::shell) refocus: Option<FocusHandle>,
    /// What the keys go into once the draw that shows it is done.
    entering: Option<FocusHandle>,
    /// A dialog on a scrim (Spotlight, Settings, Approve): the keys go into
    /// it when it opens, and Tab and Shift+Tab stay in it.
    pub(in crate::shell) modal: FocusHandle,
    /// A menu hanging from the bar (`Chrome`'s): the keys go into it when
    /// it opens.
    menu: FocusHandle,
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
            // the field owns the text: each change is Spotlight's query
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
            // every draw that showed what opened hands it the keys
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
            framed: framed(own.desk.read(cx).get()),
            app,
            key,
            field,
            approve_code,
            spotlight_rows: Default::default(),
            settings_rows: Default::default(),
            stops: HashMap::new(),
            covered: None,
            refocus: None,
            entering: None,
            modal: cx.focus_handle(),
            menu,
            _subscriptions: subscriptions,
        }
    }

    /// The one Tab stop of the tab list or radio group `id`
    /// (`a11y::roving`), made the first time it is drawn.
    fn stop(&mut self, id: &str, cx: &gpui_kit::App) -> FocusHandle {
        self.stops
            .entry(id.to_owned().into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    /// Something opened, closed, or gave way to the next over the desk (a
    /// page turned in Settings is none of these): the keys follow. What had
    /// them is kept as the first thing opens and gets them back as the last
    /// closes, unless they left a menu and closed it (`left_by_keys`).
    /// Spotlight opens empty, its field holding the keys; anything else
    /// takes them after the draw that shows it (`entered`).
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
            // what the last one typed went as it opened (`Account::approve_open`)
            self.approve_code.wipe(window, cx);
        }
        if open == Overlay::Spotlight {
            self.field.update(cx, |field, cx| {
                // no Change: the query is set here
                field.set_value("", window, cx);
                field.focus(window, cx);
            });
            self.spotlight
                .entity()
                .update(cx, |spotlight, cx| spotlight.set_query(String::new(), cx));
        }
        self.entering = Some(match open {
            Overlay::Network | Overlay::Menu(_) => self.menu.clone(),
            _ => self.modal.clone(),
        });
    }

    /// The draw that shows what opened is done: the keys go to its first
    /// control, unless something in it has them already.
    fn entered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(into) = self.entering.take() else {
            return;
        };
        if !into.contains_focused(window, cx) {
            into.focus(window, cx);
            window.focus_next(cx);
        }
    }

    /// Enter in Spotlight's field: the picked row runs.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let picked = {
            let spotlight = self.spotlight.read(cx).get();
            spotlight.picked(&self.rows(cx))
        };
        run(self.overlays.entity(), &self.app, picked, cx);
    }

    /// Spotlight's rows for the text typed.
    fn rows(&self, cx: &gpui_kit::App) -> Vec<super::super::entities::SpotRow> {
        self.spotlight.read(cx).get().rows(
            self.rail.read(cx),
            self.session.read(cx).get(),
            self.account.read(cx).get(),
            self.framed,
        )
    }
}

/// What is open, a Settings page not told apart: turning one is no handoff.
fn kind(open: Option<Overlay>) -> Option<Overlay> {
    open.map(|open| match open {
        Overlay::Settings(_) => Overlay::Settings(Default::default()),
        open => open,
    })
}

/// The window in front on `layout` has a frame to fill or move.
fn framed(layout: &crate::ui::layout::Layout) -> bool {
    layout
        .panes
        .get(layout.focused)
        .is_some_and(|pane| pane.frame.is_some())
}

/// A Spotlight row run (Enter, a click): Spotlight closes, and what the row
/// means happens, one call on the entity it moves.
pub(super) fn run(
    overlays: &Entity<Overlays>,
    app: &Entities,
    picked: Option<Spot>,
    cx: &mut gpui_kit::App,
) {
    // the console's window in front and its place on the desk: what a
    // window command (Fill, Move or size) acts on, once it has a frame
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
        // the session's and the account's rows: one call each
        Some(Spot::Switch(url)) => app
            .session
            .update(cx, |session, cx| session.switch(url, cx)),
        Some(Spot::OtherNetwork) => app.session.update(cx, |session, cx| session.disconnect(cx)),
        Some(Spot::CreateAccount) => app
            .account
            .update(cx, |account, cx| account.create_account(cx)),
        Some(Spot::Lock) => app.account.update(cx, |account, cx| account.lock(cx)),
        Some(Spot::Open(module)) => app
            .windows
            .update(cx, |windows, cx| windows.select_view(module, cx)),
        Some(Spot::Appearance(mode)) => app
            .prefs
            .update(cx, |prefs, cx| prefs.set_appearance(mode, cx)),
        Some(Spot::Help) => app.windows.update(cx, |windows, cx| windows.help_asked(cx)),
        Some(Spot::FillWindow) => {
            if let Some((desk, index)) = framed_pane(cx) {
                desk.update(cx, |desk, cx| desk.fill(index, cx));
            }
        }
        Some(Spot::HoldWindow) => {
            if let Some((desk, index)) = framed_pane(cx) {
                desk.update(cx, |desk, cx| desk.hold(index, cx));
            }
        }
        None => {}
    }
}

impl Render for OverlayLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.overlays", 1);
        // an effect, run once this draw is done: the keys go into what opened
        if self.entering.is_some() {
            cx.emit(Shown);
        }
        let dialog = match *self.overlays.read(cx).get() {
            Some(Overlay::Spotlight) => Some(self.spotlight(window, cx)),
            Some(Overlay::Settings(page)) => Some(self.settings(page, window, cx)),
            Some(Overlay::Approve) => Some(self.approve(window, cx)),
            Some(Overlay::Consent(ask)) => Some(self.consent(ask, cx)),
            _ => None,
        };
        // `size_full` too: cached, this view is a layout root of its own,
        // where a block's height is its content's (taffy), and the scrim is
        // absolute. Sized by its insets alone the root would be 0px high and
        // the scrim culled
        div().absolute().inset_0().size_full().children(dialog)
    }
}

/// The least a dialog keeps from the window's edges, at any size.
const DIALOG_EDGE: f32 = 12.;

/// A dialog `tall` high in a window `high` high, hung `top` below the bar
/// when the window has room for it, higher (not under 12px) when it hasn't:
/// where its top goes, and the height it may take so its bottom stays 12px
/// inside the window.
pub(in crate::shell) fn dialog_fit(high: f32, tall: f32, top: f32) -> (f32, f32) {
    let room = high - BAR;
    let top = (room - tall - DIALOG_EDGE).clamp(DIALOG_EDGE, top);
    (top, (room - top - DIALOG_EDGE).max(0.))
}

/// A dialog open over the desk, below the bar: a dimmed backdrop that
/// closes `closes` on a click, and on it the card the canvas dresses its
/// dialogs in, `border: 1.5px solid ink` and a soft shadow. `dress` places
/// and fills the card. Escape closes it through `keys::CloseOverlay`
/// (bound under the `overlay` context). The backdrop holds `modal`, the
/// handle the keys enter it by (`OverlayLayer::moved`): Tab and Shift+Tab
/// go round its controls, never out to the bar. (The bar's menus are
/// `Chrome`'s.) `id` is the dialog's identity: another id is another
/// dialog, whose controls hold no press, focus or hover from the last.
#[allow(clippy::too_many_arguments, reason = "one scrim, every dialog")]
pub(in crate::shell) fn scrim(
    id: impl Into<gpui_kit::SharedString>,
    role: gpui_kit::Role,
    name: &'static str,
    closes: Overlay,
    overlays: &Entity<Overlays>,
    modal: &FocusHandle,
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
        .top(px(BAR))
        .left_0()
        .right_0()
        .bottom_0()
        .occlude()
        // a dialog keeps its margin from the window's sides, as
        // `dialog_fit` keeps it from the bottom
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
        // a press inside the card stays inside: occluded, the backdrop
        // is not under the pointer and its click never fires. No
        // `on_click` here: that would offer a press on the dialog.
        .occlude()
        .flex()
        .flex_col()
        .bg(ink.bg)
        .text_color(ink.ink)
        .border(px(1.5))
        .border_color(ink.ink)
        .shadow_lg();
    // modal to assistive technology as to the keyboard: what is
    // behind the scrim is not reachable
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
        // room for it: where the design hangs it, whole
        assert_eq!(dialog_fit(800., 680., 74.), (72., 680.));
        assert_eq!(dialog_fit(1000., 680., 74.), (74., 878.));
        // the smallest window: as high as it goes, and no taller than what is left
        let (top, tall) = dialog_fit(480., 680., 74.);
        assert_eq!(top, 12.);
        assert_eq!(BAR + top + tall + 12., 480.);
    }
}
