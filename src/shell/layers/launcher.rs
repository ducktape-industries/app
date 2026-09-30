//! The launcher: everything before the desk, in a smaller window of its
//! own size — reaching a node, this device's key, its recovery phrase, and
//! the account on the network. A drawing on the left, one column to read
//! on the right, the way a game client signs in before its main window.
//!
//! Two different things happen here, one after the other. The KEY is this
//! device's: kept by the system and opened on its own; it never leaves the
//! device. The ACCOUNT is the network's: created for that key, or one this
//! key joins (another device, a passkey or a recovery key consents).
//!
//! Every screen is ported from its board on the design canvas, element by
//! element, out of `ink`'s pieces.
//!
//! `LauncherLayer` is the console's view of it, over the entities it reads
//! (`Screen`, `Session`, `Account`, `Prefs`). Its fields own what is typed;
//! its figure is written from its observers and, a frame after a screen
//! change, from a next-frame callback; never from a draw. What a
//! screen asks of the app is one call on `Session` or `Account`
//! (`on_session`, `on_account`), a typed secret going in the call.

use super::super::entities::{Account, Entities, Observed, Prefs, Screen, Secret, Session, Slice};
use super::super::figure::Figure;
use super::super::ink::{self, *};
use super::super::spin::{self, Spin};
use super::super::{WindowKey, theme};
use super::fields::NativeInput;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

mod account;
mod connect;
mod key;
mod recovery;

/// The launcher window's size; it does not change.
pub(in crate::shell) const LAUNCHER_SIZE: (f32, f32) = (960., 640.);

/// The narrowest the reading column goes beside the drawing: its 40px sides
/// around a 360px field, the address and its button on one line. Narrower,
/// the drawing steps aside and the column takes the window.
const COLUMN_MIN: f32 = 440.;

/// The drawing's panel, `width: 380px`.
const FIGURE_W: f32 = 380.;

// the launcher's own window keeps its drawing
const _: () = assert!(LAUNCHER_SIZE.0 >= FIGURE_W + COLUMN_MIN);

/// A quiet link above a screen: its element id, its words, what it does.
type Back = (&'static str, &'static str, Box<dyn Fn(&mut App)>);

/// What a launcher screen puts in the frame (`LauncherLayer::frame`).
struct LauncherScreen {
    /// The reading column's element id: the screen's name to the AX tree.
    id: &'static str,
    /// A tighter column (`padding-top: 24px; gap: 16px`) for a long body
    /// (the phrase's 24 words).
    tight: bool,
    /// The mono line under the drawing.
    caption: String,
    /// The small link above the column, if the screen has a way back.
    back: Option<Back>,
    /// The step tag over the headline: `[02 / 03] Key`.
    label: String,
    /// The `<h1>`.
    headline: String,
    /// The paragraph under it.
    lead: Option<String>,
    /// The screen's own controls, after the lead.
    body: Vec<AnyElement>,
}

/// The drawing on the left of `screen`.
fn figure(screen: Screen) -> Figure {
    match screen {
        Screen::Connect | Screen::Desk => Figure::Roll,
        Screen::Unlock { .. } => Figure::Ring,
        Screen::Phrase { .. } | Screen::Recover => Figure::Card,
        Screen::Account { .. } => Figure::Pair,
    }
}

/// The launcher's text fields: each owns what is typed into it.
pub(in crate::shell) struct Fields {
    pub(in crate::shell) endpoint: NativeInput,
    pub(in crate::shell) password: NativeInput,
    pub(in crate::shell) restore: NativeInput,
    /// The three words the phrase check asks back.
    pub(in crate::shell) words: [NativeInput; 3],
    pub(in crate::shell) name: NativeInput,
}

/// The console's launcher screens.
pub(in crate::shell) struct LauncherLayer {
    key: WindowKey,
    screen: Observed<Slice<Screen>>,
    session: Observed<Session>,
    account: Observed<Account>,
    prefs: Observed<Slice<Prefs>>,
    /// The figure on the left, written from the observers and a frame
    /// after a screen change (`render`).
    pub(in crate::shell) spin: Entity<Spin>,
    pub(in crate::shell) fields: Fields,
    /// The address the endpoint field and `Session` last agreed on: what it
    /// sent on its last change, or what the session last put in it.
    endpoint: String,
    /// A new recovery key's words while its screens show: `Account`'s,
    /// copied as the screen comes and wiped as it goes.
    phrase: Option<Secret>,
    /// The screen the observers last saw.
    shown: Screen,
    _subscriptions: [Subscription; 3],
}

impl LauncherLayer {
    pub(in crate::shell) fn new(
        entities: &Entities,
        key: WindowKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (screen, session, account, prefs) = (
            entities.screen.clone(),
            entities.session.clone(),
            entities.account.clone(),
            entities.prefs.clone(),
        );
        let shown = *screen.read(cx).get();
        let spin = {
            let prefs = prefs.read(cx).get();
            let (motion, ink) = (prefs.motion, Ink::of(prefs.dark()).figure);
            cx.new(|cx| Spin::new(figure(shown), motion, ink, cx))
        };
        // a keystroke in a sign-in field clears the step's failure
        let typed = |this: &mut Self, _: String, cx: &mut Context<Self>| {
            this.account
                .entity()
                .update(cx, |account, cx| account.clear_error(cx));
        };
        // Enter in a field: what its button does, the field's text in the call
        let enter = |field: fn(&Fields) -> &NativeInput,
                     submit: fn(&mut Account, String, &mut Context<Account>)| {
            move |this: &mut Self, cx: &mut Context<Self>| {
                let text = field(&this.fields).state.read(cx).value().to_string();
                this.account
                    .entity()
                    .update(cx, |account, cx| submit(account, text, cx));
            }
        };
        let word = |window: &mut Window, cx: &mut Context<Self>| {
            NativeInput::new(
                "",
                false,
                typed,
                |this: &mut Self, cx| {
                    let answers = this.answers(cx);
                    this.account
                        .entity()
                        .update(cx, |account, cx| account.phrase_check(answers, cx));
                },
                window,
                cx,
            )
        };
        let fields = Fields {
            endpoint: NativeInput::new(
                "127.0.0.1:8844",
                false,
                |this: &mut Self, text: String, cx| {
                    this.endpoint = text.clone();
                    this.session
                        .entity()
                        .update(cx, |session, cx| session.set_endpoint(text, cx));
                },
                |this: &mut Self, cx| {
                    this.session
                        .entity()
                        .update(cx, |session, cx| session.submit(cx));
                },
                window,
                cx,
            ),
            password: NativeInput::new(
                "",
                true,
                typed,
                enter(|fields| &fields.password, Account::unlock),
                window,
                cx,
            ),
            restore: NativeInput::new(
                "24 words, separated by spaces",
                false,
                typed,
                enter(|fields| &fields.restore, Account::recover_submit),
                window,
                cx,
            ),
            words: [word(window, cx), word(window, cx), word(window, cx)],
            name: NativeInput::new(
                "",
                false,
                typed,
                enter(|fields| &fields.name, Account::create_submit),
                window,
                cx,
            ),
        };
        let endpoint = session.read(cx).get().endpoint.clone();
        fields.endpoint.set(endpoint.clone(), window, cx);
        let subscriptions = [
            cx.observe_in(&screen, window, |this, _, window, cx| {
                this.screen_moved(window, cx)
            }),
            cx.observe(&prefs, |this, _, cx| this.draw_figure(cx)),
            cx.observe_in(&session, window, |this, _, window, cx| {
                this.session_moved(window, cx)
            }),
        ];
        let mut this = Self {
            screen: Observed::new(&screen, cx),
            session: Observed::new(&session, cx),
            account: Observed::new(&account, cx),
            prefs: Observed::new(&prefs, cx),
            key,
            spin,
            fields,
            endpoint,
            phrase: None,
            shown,
            _subscriptions: subscriptions,
        };
        this.phrase = this.phrase_words(cx);
        this
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

    /// What a press does: `call` on the account, with `field`'s text (a
    /// typed secret travels in the call, never through an entity).
    fn with_field(
        &self,
        field: &NativeInput,
        call: impl Fn(&mut Account, String, &mut Context<Account>) + 'static,
    ) -> impl Fn(&mut App) + 'static {
        let (account, field) = (self.account.entity().clone(), field.state.clone());
        move |cx| {
            let text = field.read(cx).value().to_string();
            account.update(cx, |account, cx| call(account, text, cx))
        }
    }

    /// The three words typed back for the phrase check.
    fn answers(&self, cx: &App) -> [String; 3] {
        std::array::from_fn(|nth| self.fields.words[nth].state.read(cx).value().to_string())
    }

    /// "Confirm": the three words typed back, checked against the phrase.
    fn check_phrase(&self) -> impl Fn(&mut App) + 'static {
        let account = self.account.entity().clone();
        let words: [_; 3] = std::array::from_fn(|nth| self.fields.words[nth].state.clone());
        move |cx| {
            let answers = std::array::from_fn(|nth| words[nth].read(cx).value().to_string());
            account.update(cx, |account, cx| account.phrase_check(answers, cx))
        }
    }

    /// The figure as the screen and the preferences say: compared, so an
    /// equal write draws nothing.
    fn draw_figure(&mut self, cx: &mut Context<Self>) {
        let prefs = self.prefs.read(cx).get();
        let (motion, ink) = (prefs.motion, Ink::of(prefs.dark()).figure);
        let figure = figure(*self.screen.read(cx).get());
        self.spin
            .update(cx, |spin, cx| spin.set(figure, motion, ink, cx));
    }

    /// Another screen: its figure, and the fields emptied as the last one
    /// went (a step's secrets go with it; the account's name goes only when
    /// the account steps do, "← Back" from the recovery key brings it
    /// back).
    fn screen_moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = *self.screen.read(cx).get();
        let was = std::mem::replace(&mut self.shown, now);
        if was == now {
            return;
        }
        // from the desk the figure is drawn afresh (no last frame of it to
        // keep), so it comes with its screen; between launcher screens it
        // follows a frame late (`render`)
        if was == Screen::Desk {
            self.draw_figure(cx);
        }
        let fields = &self.fields;
        for field in [&fields.password, &fields.restore]
            .into_iter()
            .chain(&fields.words)
        {
            field.wipe(window, cx);
        }
        if !matches!(now, Screen::Account { .. } | Screen::Recover) {
            fields.name.wipe(window, cx);
        }
        self.phrase = self.phrase_words(cx);
    }

    /// The address moved in `Session` other than by typing here (the node
    /// reached, a switch that did not land): the field shows it.
    fn session_moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let endpoint = &self.session.read(cx).get().endpoint;
        if *endpoint != self.endpoint {
            self.endpoint = endpoint.clone();
            self.fields.endpoint.set(endpoint.clone(), window, cx);
        }
    }

    /// A new recovery key's words, while its screens show.
    fn phrase_words(&self, cx: &App) -> Option<Secret> {
        self.account.read(cx).phrase().cloned()
    }

    /// `<button>` ([`ink::button`]): pressed, it runs `run`.
    fn button(
        &self,
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        kind: Kind,
        run: impl Fn(&mut App) + 'static,
        press: impl Into<Press>,
        ink: &Ink,
    ) -> AnyElement {
        ink::button(id, text, kind, run, press, ink)
    }

    /// `<a>` ([`ink::link_running`]): pressed, it runs `run`.
    fn link(
        &self,
        id: &'static str,
        text: impl Into<SharedString>,
        run: impl Fn(&mut App) + 'static,
        small: bool,
        ink: &Ink,
    ) -> AnyElement {
        link_running(id, text, run, small, ink)
    }

    /// One launcher screen in the canvas's frame: on the left a 380px panel,
    /// `padding: 20px; gap: 12px`, the drawing on `surface` and its mono
    /// caption; on the right the reading column, `padding: 32px 40px 0;
    /// gap: 22px`.
    fn frame(&self, screen: LauncherScreen, window: &mut Window, cx: &App) -> AnyElement {
        let LauncherScreen {
            id,
            tight,
            caption,
            back,
            label,
            headline,
            lead,
            body,
        } = screen;
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        // The canvas draws a title bar. macOS lends the window's own
        // (transparent, the traffic lights in it); elsewhere the system's
        // title bar is that bar. It is the desk's menu bar height: one
        // window serves both, and its traffic lights sit where they were
        // put when it opened.
        let titlebar = theme::traffic_lights(window).map(|lights| {
            div()
                .id("launcher-titlebar")
                .h(px(super::BAR))
                .flex_shrink_0()
                .flex()
                .items_center()
                .pl(px(lights))
                .border_b_1()
                .border_color(ink.line)
                .on_mouse_down(MouseButton::Left, |event, window, _| {
                    match event.click_count {
                        2 => window.titlebar_double_click(),
                        _ => window.start_window_move(),
                    }
                })
                .child(sans(500, 13.).text_color(ink.ink).child("Ducktape"))
        });
        let picture = div()
            .id("launcher-figure")
            .w(px(FIGURE_W))
            .flex_shrink_0()
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .border_r_1()
            .border_color(ink.line)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(ink.surface)
                    .child(spin::drawing(&self.spin)),
            )
            .child(tag("caption", caption, &ink));
        let reading = div()
            .id(id)
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .pt(px(if tight { 24. } else { 32. }))
            .px(px(40.))
            .pb(px(32.))
            .flex()
            .flex_col()
            .gap(px(if tight { 16. } else { 22. }))
            .children(
                back.map(|(key, text, run)| div().child(self.link(key, text, run, true, &ink))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .child(tag("step", label, &ink))
                    .child(h1("headline", headline, &ink))
                    .children(lead.map(|text| ink::lead("lead", text, &ink))),
            )
            .children(body);
        div()
            .id("launcher")
            .size_full()
            .flex()
            .flex_col()
            .bg(ink.bg)
            .text_color(ink.ink)
            .children(titlebar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(
                        f32::from(window.viewport_size().width) >= FIGURE_W + COLUMN_MIN,
                        |row| row.child(picture),
                    )
                    .child(reading),
            )
            .into_any_element()
    }

    /// The node reached, as the drawing's caption: "testkit · 127.0.0.1:8844".
    fn node_caption(&self, cx: &App) -> String {
        let session = self.session.read(cx).get();
        let host = crate::backend::host_of(&session.connected_rpc);
        format!("{} · {host}", session.network)
    }
}

impl Render for LauncherLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.launcher", 1);
        let shown = *self.screen.read(cx).get();
        let screen = match shown {
            Screen::Connect => self.connect(window, cx),
            Screen::Unlock { .. } => self.unlock(window, cx),
            Screen::Phrase { quiz: None } => self.phrase(window, cx),
            Screen::Phrase { quiz: Some(asked) } => self.phrase_check(asked, window, cx),
            Screen::Recover => self.recover(window, cx),
            Screen::Account { .. } => self.account_step(window, cx),
            // the root draws the desk instead
            Screen::Desk => return div(),
        };
        // A new screen's first frame keeps the last screen's figure; the new
        // one is written after that frame. Its glyphs then enter the atlas
        // after the screen's text, as they always have: within one draw
        // order gpui sorts sprites by atlas tile, so a figure written with
        // its screen moves the unlock screen's pixels by 1 LSB (the look
        // rule). Asked for here, the callback runs after this frame (asked
        // from an observer, before it); it writes, the draw does not.
        if self.spin.read(cx).figure() != figure(shown) {
            let this = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = this.update(cx, |this, cx| this.draw_figure(cx));
            });
        }
        div().size_full().child(screen)
    }
}

/// The canvas's button row: `display: flex; gap: 12px; margin-top: 4px`.
fn buttons(children: impl IntoIterator<Item = AnyElement>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap(px(12.))
        .mt(px(4.))
        .children(children)
}

/// The canvas's closing links: `gap: 10px; padding-top: 20px;
/// border-top: 1px solid line`.
fn closing(children: impl IntoIterator<Item = AnyElement>, ink: &Ink) -> Div {
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(10.))
        .pt(px(20.))
        .border_t_1()
        .border_color(ink.line)
        .children(children)
}
