//! Joining a network: the Expo build's join screens (boards 02 and 03,
//! mobile `app/join/{node,code}.tsx`; wasm-mobile SPEC §7.4) over the
//! host's own steps, as the desktop's launcher drives them: the address
//! reached (`Session::submit`), this phone's key opened or made for that
//! network (`Account::open_device_key`), and, with no account on the key
//! yet, the account step: a new account named (`Account::create_submit`;
//! the form of Expo's `CreateAccount`, mobile `app/account/index.tsx`), or,
//! from its "Add this phone from another device", a code shown for a
//! device on the account to approve (`Account::link_start`). The code stays
//! up, a failed join's included, until its Back.
//!
//! On a network, the address screen is Switch network (the same Expo
//! screen in its connected form), a sheet over the Account tab, which
//! draws its title: the node is reached through `Session::switch`, which
//! leaves the network in hand only once the other answers.

use super::look::{self, Palette};
use super::{about, account};
use ducktape_host::a11y::Control as _;
use ducktape_host::backend::join::JOIN_WAIT;
use ducktape_host::contract::SoftKeyboard;
use ducktape_host::session::{AccountStep, Screen, SessionState, Shared};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable as _, spinner::Spinner};
use gpui_kit::{
    AnyElement, App, AppContext as _, Autocapitalize, ClipboardItem, Context, Div, Entity,
    EventEmitter, Focusable as _, FontWeight, HighlightStyle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, Task,
    TextInputConfiguration, VirtualKeyboard, Window, div, px, svg,
};
use std::time::{Duration, SystemTime};

/// What the join shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Join a network: the node's address, and the nodes reached before.
    Node,
    /// The account step: a name for a new account, or the way to the code.
    Name,
    /// This phone's code, waiting for a device on the account.
    Code,
}

/// The join step `screen` shows on a phone; `None` for the desk. The
/// desktop's own steps (the layout question, which a phone's one layout
/// never asks, and the recovery key's) never come up on a phone and draw as
/// the address; the passkey's, which a phone never begins, as the name.
pub fn step(screen: Screen) -> Option<Step> {
    match screen {
        Screen::Desk => None,
        Screen::Account {
            step: AccountStep::Link,
        } => Some(Step::Code),
        Screen::Account { .. } => Some(Step::Name),
        Screen::Connect
        | Screen::Unlock { .. }
        | Screen::Phrase { .. }
        | Screen::Recover
        | Screen::Layout => Some(Step::Node),
    }
}

/// `origin` without its scheme, as the screens name a node.
pub(super) fn host_of(origin: &str) -> &str {
    origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or(origin)
}

/// Switch network asked for the node in hand: there is nothing to reach,
/// and the screen goes back where the reader was (Expo: the chain in hand
/// answered).
pub struct Stayed;

/// The join's screens.
pub struct Join {
    shared: Shared,
    /// The node's address, typed.
    pub(super) node: Entity<InputState>,
    /// Why the switch did not happen, under the field on Switch network:
    /// the address refused, or the host's words for a node that did not
    /// answer (which the window hands here, [`Join::switch_failed`]).
    pub(super) refused: Option<String>,
    /// The new account's name, on the name step.
    name: Entity<InputState>,
    /// Where the address screen is scrolled: on Switch network, a pull
    /// down drags the sheet only from its top.
    pub(super) scroll: ScrollHandle,
    /// The last code made and when (the wall clock, which runs on while the
    /// phone sleeps, as the relay's does): the countdown's start, and what a
    /// failed join shows faded. While it is set the code step shows: a
    /// failed join takes the host back to its name step, and the phone
    /// keeps the code up until its Back.
    code: Option<(String, SystemTime)>,
    /// The countdown's redraw, once a second while a code shows.
    tick: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Join {
    /// The join over the host's entities, its address field in `window`.
    pub fn new(shared: Shared, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // the last node reached, else nothing: the placeholder shows an address
        let last = shared
            .session
            .read(cx)
            .get()
            .recent_endpoints
            .first()
            .map(|entry| entry.url.clone());
        let node = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("duckpond.ducktape.industries")
                .default_value(last.clone().unwrap_or_default())
                .text_input(url_keyboard())
        });
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Name the new account")
                .text_input(name_keyboard())
        });
        let subscriptions = vec![
            cx.subscribe_in(&node, window, |this, _, event: &InputEvent, window, cx| {
                match event {
                    // a new address makes the last failure stale
                    InputEvent::Change => {
                        let typed = this.node.read(cx).value().to_string();
                        this.refused = None;
                        this.shared
                            .session
                            .update(cx, |session, cx| session.set_endpoint(typed, cx));
                    }
                    InputEvent::PressEnter { .. } => this.go(cx),
                    InputEvent::Focus => keyboard(window, Some(url_keyboard())),
                    InputEvent::Blur => keyboard(window, None),
                }
            }),
            cx.subscribe_in(&name, window, |this, _, event: &InputEvent, window, cx| {
                match event {
                    // the host's refusal goes with the next keystroke
                    InputEvent::Change => this
                        .shared
                        .account
                        .update(cx, |account, cx| account.clear_error(cx)),
                    InputEvent::PressEnter { .. } => this.create(cx),
                    InputEvent::Focus => keyboard(window, Some(name_keyboard())),
                    InputEvent::Blur => keyboard(window, None),
                }
            }),
            cx.observe_in(&shared.screen, window, |this, _, window, cx| {
                this.screen_moved(window, cx)
            }),
            cx.observe(&shared.account, |this, _, cx| this.account_moved(cx)),
            cx.observe(&shared.session, |_, _, cx| cx.notify()),
        ];
        let mut join = Self {
            shared,
            node,
            refused: None,
            name,
            scroll: ScrollHandle::new(),
            code: None,
            tick: None,
            _subscriptions: subscriptions,
        };
        join.screen_moved(window, cx);
        // a phone that has reached no node yet opens on the field, its
        // keyboard up (Expo `autoFocus` with no Recent)
        if last.is_none() && step(*join.shared.screen.read(cx).get()) == Some(Step::Node) {
            join.node.update(cx, |node, cx| node.focus(window, cx));
        }
        join
    }

    /// Continue: the typed address reached, or refused as no address; on a
    /// network, switched to.
    fn go(&mut self, cx: &mut Context<Self>) {
        let typed = self.node.read(cx).value().trim().to_owned();
        if self.switching(cx) {
            return self.switch_to(&typed, cx);
        }
        self.shared.session.update(cx, |session, cx| {
            session.set_endpoint(typed, cx);
            session.submit(cx);
        });
    }

    /// Whether the address screen is Switch network: drawn on a network
    /// (the desk under it), not before one.
    fn switching(&self, cx: &Context<Self>) -> bool {
        step(*self.shared.screen.read(cx).get()).is_none()
    }

    /// Switch network opened (Expo mounts it afresh): the last node reached
    /// in the field, the keys in it, nothing refused, scrolled to its top
    /// (the handle outlives the sheet, and the join's own screen shares it).
    pub fn open_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refused = None;
        self.scroll.set_offset(Default::default());
        let last = self
            .shared
            .session
            .read(cx)
            .get()
            .recent_endpoints
            .first()
            .map(|entry| entry.url.clone())
            .unwrap_or_default();
        self.node.update(cx, |node, cx| {
            node.set_value(last, window, cx);
            node.focus(window, cx);
        });
        cx.notify();
    }

    /// The node at `text` switched to: refused when it is no address; the
    /// node in hand needs no reach ([`Stayed`]); any other is reached first
    /// and taken only once it answers (`Session::switch`).
    fn switch_to(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(origin) = noded_client::endpoint::endpoint_origin(text.trim()) else {
            self.refused = Some(noded_client::endpoint::ENDPOINT_REFUSAL.to_owned());
            return cx.notify();
        };
        self.refused = None;
        let session = self.shared.session.read(cx).get();
        if origin == session.connected_rpc && !session.connecting {
            cx.emit(Stayed);
        } else {
            self.shared
                .session
                .update(cx, |session, cx| session.switch(origin, cx));
        }
        cx.notify();
    }

    /// The switch did not land: the host's words, under the field.
    pub fn switch_failed(&mut self, said: String, cx: &mut Context<Self>) {
        self.refused = Some(said);
        cx.notify();
    }

    /// The step drawn: the host's ([`step`]), but the code while one is up
    /// ([`Join::code`]).
    fn shown(&self, cx: &App) -> Option<Step> {
        match step(*self.shared.screen.read(cx).get()) {
            Some(Step::Name) if self.code.is_some() => Some(Step::Code),
            step => step,
        }
    }

    /// Back on the account step: the code goes back to the name step, its
    /// wait and any failure with it; the name step leaves the network
    /// ([`Join::leave`]).
    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.shown(cx) != Some(Step::Code) {
            return self.leave(cx);
        }
        self.code = None;
        self.tick = None;
        self.shared.account.update(cx, |account, cx| {
            account.link_cancel(cx);
            account.clear_error(cx);
        });
        cx.notify();
    }

    /// The network is left (the name step's Back, a reach's Cancel), and
    /// with it any wait; the address screen comes back, the node in Recent.
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        self.code = None;
        self.shared
            .session
            .update(cx, |session, cx| session.disconnect(cx));
    }

    /// A fresh code, the wait under the last one dropped.
    fn new_code(&mut self, cx: &mut Context<Self>) {
        self.shared.account.update(cx, |account, cx| {
            account.link_cancel(cx);
            account.link_start(cx);
        });
    }

    /// Create account: the name typed, made by the host, which refuses an
    /// empty one itself.
    fn create(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().to_string();
        self.shared
            .account
            .update(cx, |account, cx| account.create_submit(name, cx));
    }

    /// "Add this phone from another device": the code; not while a create
    /// is out, whose answer would land on it.
    fn add_phone(&mut self, cx: &mut Context<Self>) {
        self.shared.account.update(cx, |account, cx| {
            if !account.get().busy {
                account.link_start(cx);
            }
        });
    }

    fn screen_moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // off the account step (joined, made, the network left) its code
        // goes, and the name typed for that network
        if !matches!(
            step(*self.shared.screen.read(cx).get()),
            Some(Step::Name | Step::Code)
        ) {
            self.code = None;
            if !self.name.read(cx).value().is_empty() {
                self.name
                    .update(cx, |name, cx| name.set_value("", window, cx));
            }
        }
        let shown = self.shown(cx);
        // a field off its screen goes, and the keyboard with it
        for (field, on) in [(&self.node, Step::Node), (&self.name, Step::Name)] {
            if shown != Some(on) && field.read(cx).focus_handle(cx).is_focused(window) {
                window.blur(cx);
                keyboard(window, None);
            }
        }
        if shown == Some(Step::Code) {
            if self.tick.is_none() {
                self.tick = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            break;
                        }
                    }
                }));
            }
        } else {
            self.tick = None;
        }
        cx.notify();
    }

    fn account_moved(&mut self, cx: &mut Context<Self>) {
        let code = &self.shared.account.read(cx).get().link_code;
        if !code.is_empty() && self.code.as_ref().is_none_or(|(shown, _)| shown != code) {
            self.code = Some((code.clone(), SystemTime::now()));
        }
        cx.notify();
    }

    /// Join a network (board 02), or on a network Switch network (Expo
    /// `join/node.tsx` connected, under the sheet's title).
    fn node_step(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let palette = look::palette(cx);
        let session = self.shared.session.read(cx).get().clone();
        let account = self.shared.account.read(cx).get();
        let switching = self.switching(cx);
        let unlocking = matches!(self.shared.screen.read(cx).get(), Screen::Unlock { .. });
        // the address refused, the node not reached, or the key or account
        // not found on the node reached; a switch's own
        let problem = match switching {
            true => self.refused.clone(),
            false => [&session.endpoint_error, &session.error]
                .into_iter()
                .chain(unlocking.then_some(&account.error))
                .find(|said| !said.is_empty())
                .cloned(),
        };
        let reaching = if session.connecting {
            Some(host_of(&session.reaching).to_owned())
        } else if unlocking && problem.is_none() {
            Some(host_of(&session.connected_rpc).to_owned())
        } else {
            None
        };
        let focused = self.node.read(cx).focus_handle(cx).is_focused(window);
        let (title, said) = match switching {
            true => (
                None,
                format!(
                    "This phone stays on {} until the switch is done. If the other network has \
                     no account for this phone yet, you can make a new one there, or approve this \
                     phone from a computer that is signed in.",
                    session.network
                ),
            ),
            false => (
                Some("Join a network"),
                "If the network has no account for this phone yet, you can make a new one here, \
                 or approve this phone from a computer that is signed in."
                    .to_owned(),
            ),
        };
        let mut top = div()
            .p(px(look::GUTTER))
            .flex()
            .flex_col()
            .gap(px(look::gap::LG))
            .children(title.map(|title| {
                look::text(look::BIG_TITLE, FontWeight::MEDIUM, palette.foreground())
                    .mt(px(look::gap::XL))
                    .child(title)
            }))
            .child(look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted()).child(said))
            .child(
                field(
                    &self.node,
                    "join/node",
                    "Node address",
                    true,
                    focused,
                    problem.is_some(),
                    palette,
                )
                .child(under(problem, palette)),
            );
        let label = match &reaching {
            Some(host) => format!("Reaching {host}…"),
            None => "Continue".to_owned(),
        };
        let busy = reaching.is_some();
        let mut connect = button("join/connect", label, Kind::Primary, None, busy, palette);
        if !busy {
            connect = connect.on_click(cx.listener(|this, _, _, cx| this.go(cx)));
        }
        top = top.child(connect.w_full());
        // nothing times a reach out: the person can always stop it; a
        // switch's leaves the network in hand as it is, and Back leaves it
        if busy && !switching {
            top = top.child(
                div().flex().justify_center().child(
                    button(
                        "join/cancel",
                        "Cancel".to_owned(),
                        Kind::Quiet,
                        None,
                        false,
                        palette,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.leave(cx))),
                ),
            );
        }
        // content-sized, growing to the screen it is on: the join fills the
        // window, Switch network's sheet holds what it shows
        let mut body = div()
            .id("join")
            .flex_grow(1.)
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(top);
        if !session.recent_endpoints.is_empty() {
            body = body.child(
                div()
                    .min_h(px(40.))
                    .flex()
                    .items_end()
                    .pb(px(4.))
                    .px(px(look::GUTTER))
                    .child(
                        look::text(look::CAPTION, FontWeight::MEDIUM, palette.muted())
                            .child("Recent"),
                    ),
            );
        }
        for (at, node) in session.recent_endpoints.iter().enumerate() {
            body = body.child(self.recent(at, node, &session.connected_rpc, palette, cx));
        }
        div()
            .flex_grow(1.)
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(body)
    }

    /// A node reached before (Expo `Row`): its network over its address;
    /// the one in hand says Current, any other can be forgotten.
    fn recent(
        &self,
        at: usize,
        node: &ducktape_host::backend::RecentEndpoint,
        current: &str,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let url = node.url.clone();
        let title = if node.network.is_empty() {
            host_of(&node.url).to_owned()
        } else {
            node.network.clone()
        };
        let caption = if node.other_chain {
            format!("Other chain · {}", node.url)
        } else {
            node.url.clone()
        };
        let tail = if node.url == current {
            look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                .child("Current")
                .into_any_element()
        } else {
            let forgotten = node.url.clone();
            div()
                .id(SharedString::from(format!("join/forget/{at}")))
                .control(Role::Button, format!("Forget {}", node.url))
                .debug_selector(move || format!("join/forget/{at}"))
                .size(px(look::HIT))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path("icons/x.svg")
                        .size(px(look::ICON_HIT))
                        .text_color(palette.muted()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.shared
                        .session
                        .update(cx, |session, cx| session.forget(&forgotten, cx));
                }))
                .into_any_element()
        };
        // the pick (its icon and words) and the tail are siblings, as the
        // desktop's Recent rows: a press on × forgets and reaches nothing
        let pick = div()
            .id(SharedString::from(format!("join/recent/{at}")))
            .control(Role::Button, title.clone())
            .debug_selector(move || format!("join/recent/{at}"))
            .flex_1()
            .min_w_0()
            .self_stretch()
            .flex()
            .items_center()
            .gap(px(look::gap::LG))
            .py(px(look::gap::XS))
            .pl(px(look::GUTTER))
            .child(
                div().w(px(24.)).flex().justify_center().child(
                    svg()
                        .path("icons/server.svg")
                        .size(px(look::ICON_ROW))
                        .text_color(palette.muted()),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        look::text(look::BODY, FontWeight::NORMAL, palette.foreground())
                            .truncate()
                            .child(title),
                    )
                    .child(
                        look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                            .font_family(ducktape_host::fonts::FAMILY_MONO)
                            .truncate()
                            .child(caption),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.switching(cx) {
                    return this.switch_to(&url, cx);
                }
                let url = url.clone();
                this.shared
                    .session
                    .update(cx, |session, cx| session.connect(url, cx));
            }));
        div()
            .min_h(px(look::ROW_TALL))
            .flex()
            .items_center()
            .gap(px(look::gap::LG))
            .pr(px(look::GUTTER))
            .border_b_1()
            .border_color(palette.border())
            .child(pick)
            .child(tail)
            .into_any_element()
    }

    /// The account step (the desktop's; the form of Expo's `CreateAccount`):
    /// a name for a new account, which the host makes, its refusal (an
    /// empty name's included) in a notice under the field; under the form,
    /// for an account that exists, the way to this phone's code.
    fn name_step(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let palette = look::palette(cx);
        let session = self.shared.session.read(cx).get();
        let account = self.shared.account.read(cx).get();
        let busy = account.busy;
        let problem = (!account.error.is_empty()).then(|| account.error.clone());
        let focused = self.name.read(cx).focus_handle(cx).is_focused(window);
        let header = header(session, palette, cx);
        let joining = format!("Already have an account on {}?", session.network);
        let mut create = button(
            "join/create",
            match busy {
                true => "Creating…",
                false => "Create account",
            }
            .to_owned(),
            Kind::Primary,
            None,
            busy,
            palette,
        );
        if !busy {
            create = create.on_click(cx.listener(|this, _, _, cx| this.create(cx)));
        }
        let form = div()
            .p(px(look::GUTTER))
            .flex()
            .flex_col()
            .gap(px(look::gap::LG))
            .child(
                look::text(look::BIG_TITLE, FontWeight::MEDIUM, palette.foreground())
                    .child("Create an account"),
            )
            .child(
                look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted()).child(
                    "Your key isn't linked to an account yet. An account gives you a name \
                     others see.",
                ),
            )
            .child(field(
                &self.name,
                "join/name",
                "Account name",
                false,
                focused,
                false,
                palette,
            ))
            .children(problem.map(|problem| notice("join/problem", problem, palette)))
            .child(create.w_full());
        let add = account::row(
            "join/add-phone",
            "icons/smartphone.svg",
            "Add this phone from another device",
            None,
            palette,
        )
        .control(Role::Button, "Add this phone from another device")
        .active(|style| style.opacity(look::PRESSED))
        .child(
            svg()
                .path("icons/chevron-right.svg")
                .size(px(look::ICON_ROW))
                .flex_none()
                .text_color(palette.faint()),
        )
        .on_click(cx.listener(|this, _, _, cx| this.add_phone(cx)));
        let body = div()
            .id("join/account")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(form)
            .child(about::section(joining, None, palette))
            .child(add);
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
    }

    /// This phone's code, waiting for consent (board 03): the code a
    /// signed-in device's Add a device types (a computer's account menu, a
    /// phone's Account tab), the key's fingerprint both screens show, the
    /// time left; a failure says why, and New code starts over
    /// with the same key.
    fn code_step(&self, cx: &mut Context<Self>) -> Div {
        let palette = look::palette(cx);
        let session = self.shared.session.read(cx).get();
        let account = self.shared.account.read(cx).get();
        let failure = (!account.error.is_empty()).then(|| account.error.clone());
        let (code, made) = match &self.code {
            Some((code, made)) => (code.clone(), Some(*made)),
            None => (String::new(), None),
        };
        let gone = made.map_or(Duration::ZERO, |made| {
            SystemTime::now().duration_since(made).unwrap_or_default()
        });
        let left = JOIN_WAIT.saturating_sub(gone);
        let left = left.as_secs();
        let fingerprint = noded_client::hex::hex_decode(&account.signer_key)
            .map(|key| noded_client::join::fingerprint(&key))
            .unwrap_or_default();
        let header = header(session, palette, cx);
        let said = "On a signed-in computer, open the account menu and choose Add a device…; on \
                    a signed-in phone, Account › Add a device. Type this code there, and approve \
                    only if both screens show the same fingerprint.";
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::MEDIUM),
            ..HighlightStyle::default()
        };
        let words = StyledText::new(said).with_highlights(
            said.match_indices("Add a device")
                .map(|(at, named)| (at..at + named.len(), bold)),
        );
        let copy = {
            let code = code.clone();
            button(
                "join/copy",
                "Copy".to_owned(),
                Kind::Small,
                Some("icons/copy.svg"),
                false,
                palette,
            )
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
            })
        };
        let mut boxed = div()
            .flex()
            .flex_col()
            .gap(px(look::gap::LG))
            .p(px(look::gap::BLOCK))
            .border_1()
            .border_color(palette.border_strong())
            .child(
                look::text(look::EYEBROW, FontWeight::MEDIUM, palette.muted())
                    .font_family(ducktape_host::fonts::FAMILY_MONO)
                    .letter_spacing(px(0.66))
                    .child("THIS PHONE’S CODE"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(look::gap::SM))
                    .child(
                        look::text(look::DISPLAY, FontWeight::MEDIUM, palette.foreground())
                            .id("join/code")
                            .control(Role::Label, code.clone())
                            .font_family(ducktape_host::fonts::FAMILY_MONO)
                            .child(code),
                    )
                    .child(copy),
            )
            .child(div().h(px(1.)).bg(palette.border()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                            .child("Key fingerprint"),
                    )
                    .child(
                        look::text(look::BODY, FontWeight::NORMAL, palette.foreground())
                            .font_family(ducktape_host::fonts::FAMILY_MONO)
                            .child(fingerprint),
                    ),
            );
        if failure.is_some() {
            boxed = boxed.opacity(0.5);
        }
        let mut body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(look::gap::LG))
            .p(px(look::GUTTER))
            .child(
                look::text(look::BIG_TITLE, FontWeight::MEDIUM, palette.foreground())
                    .child("Approve this phone from another device"),
            )
            .child(look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted()).child(words))
            .child(boxed);
        let again = cx.listener(|this, _, _, cx| this.new_code(cx));
        body = match failure {
            Some(failure) => body.child(notice("join/failure", failure, palette)).child(
                button(
                    "join/new-code",
                    "New code".to_owned(),
                    Kind::Primary,
                    None,
                    false,
                    palette,
                )
                .w_full()
                .on_click(again),
            ),
            None => body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(look::gap::SM))
                        .child(Spinner::new().color(palette.muted()).with_size(px(16.)))
                        .child(
                            look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted()).child(
                                format!(
                                    "Waiting for approval… {}:{:02} left",
                                    left / 60,
                                    left % 60
                                ),
                            ),
                        ),
                )
                .child(div().flex_1())
                .child(
                    div().flex().justify_center().child(
                        button(
                            "join/new-code",
                            "New code".to_owned(),
                            Kind::Quiet,
                            None,
                            false,
                            palette,
                        )
                        .on_click(again),
                    ),
                ),
        };
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
    }
}

impl EventEmitter<Stayed> for Join {}

impl Render for Join {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.shown(cx) {
            Some(Step::Code) => self.code_step(cx),
            Some(Step::Name) => self.name_step(window, cx),
            Some(Step::Node) | None => self.node_step(window, cx),
        }
    }
}

/// The account step's header (Expo `Header` with back): the network over
/// its node, 8 in from the back hit; Back is [`Join::back`].
fn header(session: &SessionState, palette: Palette, cx: &Context<Join>) -> Div {
    let back = cx.listener(|this, _, _, cx| this.back(cx));
    look::header("back", back, palette).child(
        div()
            .flex_1()
            .min_w_0()
            .px(px(look::gap::SM))
            .flex()
            .flex_col()
            .child(
                look::text(look::TITLE, FontWeight::MEDIUM, palette.foreground())
                    .truncate()
                    .child(session.network.clone()),
            )
            .child(
                look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                    .truncate()
                    .child(host_of(&session.connected_rpc).to_owned()),
            ),
    )
}

/// A field (Expo `Field`): its label over the box (the edge 2 in the
/// foreground focused, in danger with a problem), the text in the data
/// face when `mono`; what goes under it is the caller's.
pub(super) fn field(
    input: &Entity<InputState>,
    id: &'static str,
    label: &'static str,
    mono: bool,
    focused: bool,
    problem: bool,
    palette: Palette,
) -> Div {
    let (edge, thick) = match (problem, focused) {
        (true, _) => (palette.danger(), 2.),
        (false, true) => (palette.foreground(), 2.),
        (false, false) => (palette.border_strong(), 1.),
    };
    let (face, role) = match mono {
        true => (ducktape_host::fonts::FAMILY_MONO, look::SECONDARY),
        false => (ducktape_host::fonts::FAMILY_UI, look::BODY),
    };
    let focus = input.clone();
    div()
        .flex()
        .flex_col()
        .child(
            look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                .mb(px(look::gap::XS))
                .child(label),
        )
        .child(
            div()
                .id(id)
                .control(Role::Group, label)
                .min_h(px(look::CONTROL))
                .flex()
                .items_center()
                .px(px(look::gap::LG - (thick - 1.)))
                .border(px(thick))
                .border_color(edge)
                .bg(palette.background())
                .on_click(move |_, window, cx| {
                    focus.update(cx, |input, cx| input.focus(window, cx));
                })
                .child(
                    Input::new(input)
                        .appearance(false)
                        .font_family(face)
                        .text_size(px(role.size()))
                        .py_0()
                        .px_0(),
                ),
        )
}

/// Under the address field: the problem in danger, else the rule.
fn under(problem: Option<String>, palette: Palette) -> Div {
    match problem {
        Some(problem) => {
            look::text(look::CAPTION, FontWeight::NORMAL, palette.danger()).child(problem)
        }
        None => look::text(look::CAPTION, FontWeight::NORMAL, palette.muted()).child(
            "A domain, or a host and a port, with http:// or https:// in front if you like.",
        ),
    }
    .mt(px(look::gap::XS))
}

/// A refusal (Expo `Notice`, danger): 13 in danger on `danger_soft`.
pub(super) fn notice(id: &'static str, said: String, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .control(Role::Alert, said.clone())
        .py(px(look::gap::MD))
        .px(px(look::GUTTER))
        .bg(palette.danger_soft())
        .child(look::text(look::SECONDARY, FontWeight::NORMAL, palette.danger()).child(said))
}

/// A button's kind (Expo `Button`).
pub(super) enum Kind {
    /// 48, the primary's ground, body medium, the row's width.
    Primary,
    /// 36, a hairline, secondary medium.
    Small,
    /// 44, no edge, body regular, muted.
    Quiet,
}

/// An Expo `Button` named `label`, with `icon` before it (16 small, 18
/// else), or a spinner while `busy` (a press box of at least 44: the small
/// one's 36 reaches it unseen).
pub(super) fn button(
    id: &'static str,
    label: String,
    kind: Kind,
    icon: Option<&'static str>,
    busy: bool,
    palette: Palette,
) -> gpui_kit::Stateful<Div> {
    let wide = matches!(kind, Kind::Primary);
    let (height, role, weight, tone, ground, edge) = match kind {
        Kind::Primary => (
            look::PRIMARY,
            look::BODY,
            FontWeight::MEDIUM,
            palette.primary_foreground(),
            Some(palette.primary()),
            Some(palette.primary()),
        ),
        Kind::Small => (
            look::SMALL_BUTTON,
            look::SECONDARY,
            FontWeight::MEDIUM,
            palette.foreground(),
            None,
            Some(palette.border_strong()),
        ),
        Kind::Quiet => (
            look::CONTROL,
            look::BODY,
            FontWeight::NORMAL,
            palette.muted(),
            None,
            None,
        ),
    };
    let mut drawn = div()
        .h(px(height))
        .px(px(if height < look::CONTROL {
            look::gap::LG
        } else {
            look::gap::BLOCK
        }))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(look::gap::SM));
    let glyph = if height < look::CONTROL { 16. } else { 18. };
    if busy {
        drawn = drawn.child(Spinner::new().color(tone).with_size(px(glyph)));
    } else if let Some(icon) = icon {
        drawn = drawn.child(svg().path(icon).size(px(glyph)).text_color(tone));
    }
    drawn = drawn.child(look::text(role, weight, tone).child(label.clone()));
    // a primary button stretches to the row (Expo `wide`)
    if wide {
        drawn = drawn.flex_1();
    }
    if let Some(ground) = ground {
        drawn = drawn.bg(ground);
    }
    if let Some(edge) = edge {
        drawn = drawn.border_1().border_color(edge);
    }
    div()
        .id(id)
        .control(Role::Button, label)
        .debug_selector(move || id.to_owned())
        .min_h(px(look::HIT))
        .flex()
        .items_center()
        .justify_center()
        .child(drawn)
}

/// The address field's keyboard: a URL one, nothing corrected or capitalized.
fn url_keyboard() -> TextInputConfiguration {
    TextInputConfiguration {
        virtual_keyboard: VirtualKeyboard::Url,
        ..TextInputConfiguration::default()
    }
}

/// The name field's keyboard: React Native's default, which Expo's field
/// keeps (sentences capitalized, corrections and suggestions on).
fn name_keyboard() -> TextInputConfiguration {
    TextInputConfiguration {
        autocorrect: true,
        autocapitalize: Autocapitalize::Sentences,
        suggestions: true,
        ..TextInputConfiguration::default()
    }
}

/// A field's keyboard up as `want` says, or down: gm is told (Android);
/// gpui_ios raises its own with the focused field, from the field's
/// `text_input`.
pub(super) fn keyboard(window: &mut Window, want: Option<TextInputConfiguration>) {
    let want = match want {
        Some(config) => SoftKeyboard::Show(config),
        None => SoftKeyboard::Hide,
    };
    ducktape_host::contract::os().soft_keyboard(window, want);
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};

    /// Every host screen has its place on a phone: the desk; the name for
    /// the account step, the code once it waits on one; else the address
    /// (the layout question never comes up on a phone, which has one
    /// layout).
    #[test]
    fn each_screen_draws_as_the_desk_the_name_the_code_or_the_address() {
        assert_eq!(step(Screen::Desk), None);
        assert_eq!(step(Screen::Layout), Some(Step::Node));
        assert_eq!(step(Screen::Connect), Some(Step::Node));
        assert_eq!(step(Screen::Unlock { awaiting: true }), Some(Step::Node));
        for (account, drawn) in [
            (AccountStep::Name, Step::Name),
            (AccountStep::Passkey, Step::Name),
            (AccountStep::Link, Step::Code),
        ] {
            assert_eq!(step(Screen::Account { step: account }), Some(drawn));
        }
        assert_eq!(
            host_of("https://garage.ducktape.industries"),
            "garage.ducktape.industries"
        );
    }

    /// The node the join reaches: its status is the test's own; anything
    /// else asked of it is refused at once, on this machine.
    const ORIGIN: &str = "http://127.0.0.1:1";

    /// Runs the window's work until `done` holds: the key and the account
    /// are found (a node answers) on the host's runtime, off the test's
    /// thread.
    pub(in crate::shell) fn until(
        cx: &mut VisualTestContext,
        done: impl Fn(&mut VisualTestContext) -> bool,
    ) {
        for _ in 0..500 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the join never got there");
    }

    /// The join over the host's own steps, as a phone reaches the account
    /// step: a node reached (its status the test's), this phone's key
    /// opened, the node's word that no account holds it.
    fn on_the_account_step(
        cx: &mut TestAppContext,
    ) -> (Entity<Join>, Shared, &mut VisualTestContext) {
        use ducktape_host::session::tests::{source, status};
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let shared = cx.update(|cx| Shared::for_test(Default::default(), Default::default(), cx));
        let (join, cx) = cx.add_window_view(|window, cx| Join::new(shared.clone(), window, cx));
        let (answers, _) = source(|_| Ok(status(1)));
        shared.session.update(cx, |session, cx| {
            session.connect_through(ORIGIN.into(), answers, cx)
        });
        until(cx, |cx| {
            shared.screen.read_with(cx, |s, _| *s.get()) == Screen::Unlock { awaiting: true }
        });
        // the key let go, so the join that follows asks no relay
        cx.update(|_, cx| {
            cx.background_spawn(ducktape_host::backend::lock_signer())
                .detach()
        });
        let key = shared
            .account
            .read_with(cx, |account, _| account.get().signer_key.clone());
        shared
            .account
            .update(cx, |account, cx| account.resolved(ORIGIN, &key, None, cx));
        cx.run_until_parked();
        (join, shared, cx)
    }

    fn press(cx: &mut VisualTestContext, id: &'static str) {
        let at = cx
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} not drawn"));
        cx.simulate_click(at.center(), Modifiers::none());
        cx.run_until_parked();
    }

    /// The account step as a phone walks it: the name step, no code made
    /// on its own; Create account with no name shows the host's refusal;
    /// Add this phone… shows a code, New code another; Back goes to the
    /// name step, Back again leaves for the address with the node under
    /// Recent; × there forgets it and reaches nothing.
    #[gpui_kit::test]
    fn the_name_step_comes_first_the_code_on_a_tap_and_back_walks_to_the_address(
        cx: &mut TestAppContext,
    ) {
        let _seat = ducktape_host::backend::seat_serial();
        let (join, shared, cx) = on_the_account_step(cx);
        let screen = |cx: &mut VisualTestContext| shared.screen.read_with(cx, |s, _| *s.get());
        let account = |cx: &mut VisualTestContext| {
            shared
                .account
                .read_with(cx, |account, _| account.get().clone())
        };
        let shown = |cx: &mut VisualTestContext| join.read_with(cx, |join, cx| join.shown(cx));
        assert_eq!(shown(cx), Some(Step::Name));
        assert!(account(cx).link_code.is_empty(), "a code came with no tap");

        press(cx, "join/create");
        assert_eq!(account(cx).error, "Type the name others will see first.");
        assert_eq!(shown(cx), Some(Step::Name));

        press(cx, "join/add-phone");
        until(cx, |cx| join.read_with(cx, |join, _| join.code.is_some()));
        assert_eq!(shown(cx), Some(Step::Code));
        let first = join.read_with(cx, |join, _| join.code.clone().unwrap().0);
        join.update(cx, |join, cx| join.new_code(cx));
        cx.run_until_parked();
        let second = join.read_with(cx, |join, _| join.code.clone().unwrap().0);
        assert_ne!(first, second, "New code made no new code");

        join.update(cx, |join, cx| join.back(cx));
        cx.run_until_parked();
        assert_eq!(
            screen(cx),
            Screen::Account {
                step: AccountStep::Name
            }
        );
        assert_eq!(shown(cx), Some(Step::Name));
        assert!(account(cx).link_code.is_empty(), "Back left the wait out");

        join.update(cx, |join, cx| join.back(cx));
        cx.run_until_parked();
        assert_eq!(screen(cx), Screen::Connect);
        let recent = shared
            .session
            .read_with(cx, |session, _| session.get().recent_endpoints.clone());
        assert_eq!(recent.first().map(|node| node.url.as_str()), Some(ORIGIN));

        press(cx, "join/forget/0");
        let after = shared
            .session
            .read_with(cx, |session, _| session.get().clone());
        assert!(after.recent_endpoints.is_empty(), "× forgot nothing");
        assert!(
            !after.connecting && !after.connected,
            "× reached the node it forgot"
        );
        assert_eq!(screen(cx), Screen::Connect);
    }

    /// A join that failed takes the host back to its name step; the phone
    /// keeps the code up, its failure with it, until Back, which lands on
    /// the name step with nothing under the field.
    #[gpui_kit::test]
    fn a_failed_join_stays_on_the_code_until_back_lands_on_a_clear_name_step(
        cx: &mut TestAppContext,
    ) {
        let _seat = ducktape_host::backend::seat_serial();
        let (join, shared, cx) = on_the_account_step(cx);
        let error = |cx: &mut VisualTestContext| {
            shared
                .account
                .read_with(cx, |account, _| account.get().error.clone())
        };
        let shown = |cx: &mut VisualTestContext| join.read_with(cx, |join, cx| join.shown(cx));
        press(cx, "join/add-phone");
        until(cx, |cx| join.read_with(cx, |join, _| join.code.is_some()));

        shared.account.update(cx, |account, cx| {
            account.joined(Err("This code ran out.".into()), cx)
        });
        cx.run_until_parked();
        let screen = shared.screen.read_with(cx, |s, _| *s.get());
        assert_eq!(
            screen,
            Screen::Account {
                step: AccountStep::Name
            }
        );
        assert_eq!(shown(cx), Some(Step::Code), "the failure left the code");
        assert_eq!(error(cx), "This code ran out.");

        join.update(cx, |join, cx| join.back(cx));
        cx.run_until_parked();
        assert_eq!(shown(cx), Some(Step::Name));
        assert_eq!(error(cx), "", "the join's failure showed on the name step");
    }
}
