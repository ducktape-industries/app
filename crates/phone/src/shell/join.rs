//! Joining a network: the Expo build's join screens (boards 02 and 03,
//! mobile `app/join/{node,code}.tsx`; wasm-mobile SPEC §7.4) over the
//! host's own steps, as the desktop's launcher drives them: the address
//! reached (`Session::submit`), this phone's key opened or made for that
//! network (`Account::open_device_key`), and, with no account on the key
//! yet, a code shown for a computer on the account to approve
//! (`Account::link_start`). A phone joins an account the person already
//! has: no account is made here, as in the Expo build.

use super::look::{self, Palette};
use ducktape_host::a11y::Control as _;
use ducktape_host::backend::join::JOIN_WAIT;
use ducktape_host::contract::SoftKeyboard;
use ducktape_host::session::{AccountState, AccountStep, Screen, Shared};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable as _, spinner::Spinner};
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Div, Entity, Focusable as _, FontWeight,
    HighlightStyle, InteractiveElement as _, IntoElement, ParentElement as _, Render, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, Task,
    TextInputConfiguration, VirtualKeyboard, Window, div, px, svg,
};
use std::time::{Duration, SystemTime};

/// What the join shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Join a network: the node's address, and the nodes reached before.
    Node,
    /// This phone's code, waiting for a computer on the account.
    Code,
}

/// The join step `screen` shows on a phone; `None` for the desk. The
/// desktop's own steps (the layout question, which a phone's one layout
/// never asks, and the recovery key's) never come up on a phone and draw as
/// the address.
pub fn step(screen: Screen) -> Option<Step> {
    match screen {
        Screen::Desk => None,
        Screen::Account { .. } => Some(Step::Code),
        Screen::Connect
        | Screen::Unlock { .. }
        | Screen::Phrase { .. }
        | Screen::Recover
        | Screen::Layout => Some(Step::Node),
    }
}

/// The key has no account yet and nothing is out or failed: a code is made
/// (the phone's one way onto an account). A failed join leaves its error,
/// which the screen shows with New code, rather than a fresh code over it.
fn wants_code(screen: Screen, account: &AccountState) -> bool {
    screen
        == (Screen::Account {
            step: AccountStep::Name,
        })
        && account.link_code.is_empty()
        && account.error.is_empty()
        && !account.busy
}

/// `origin` without its scheme, as the screens name a node.
fn host_of(origin: &str) -> &str {
    origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or(origin)
}

/// The join's screens.
pub struct Join {
    shared: Shared,
    node: Entity<InputState>,
    /// The last code made and when (the wall clock, which runs on while the
    /// phone sleeps, as the relay's does): the countdown's start, and what a
    /// failed join shows faded.
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
                .placeholder("garage.ducktape.industries")
                .default_value(last.clone().unwrap_or_default())
                .text_input(url_keyboard())
        });
        let subscriptions = vec![
            cx.subscribe_in(&node, window, |this, _, event: &InputEvent, window, cx| {
                match event {
                    // a new address makes the last failure stale
                    InputEvent::Change => {
                        let typed = this.node.read(cx).value().to_string();
                        this.shared
                            .session
                            .update(cx, |session, cx| session.set_endpoint(typed, cx));
                    }
                    InputEvent::PressEnter { .. } => this.go(cx),
                    InputEvent::Focus => keyboard(window, true),
                    InputEvent::Blur => keyboard(window, false),
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

    /// Continue: the typed address reached, or refused as no address.
    fn go(&mut self, cx: &mut Context<Self>) {
        let typed = self.node.read(cx).value().trim().to_owned();
        self.shared.session.update(cx, |session, cx| {
            session.set_endpoint(typed, cx);
            session.submit(cx);
        });
    }

    /// Back on the code screen: the network is left, and with it the wait;
    /// the address screen comes back, the node in Recent.
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

    fn screen_moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let screen = *self.shared.screen.read(cx).get();
        let step = step(screen);
        // off the address screen the field goes, and the keyboard with it
        if step != Some(Step::Node) && self.node.read(cx).focus_handle(cx).is_focused(window) {
            window.blur(cx);
            keyboard(window, false);
        }
        if step == Some(Step::Code) {
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
            // read once the update that moved the screen is through: a
            // failed join says why in that same update
            let (screen, account) = (self.shared.screen.clone(), self.shared.account.clone());
            cx.defer(move |cx| {
                if wants_code(*screen.read(cx).get(), account.read(cx).get()) {
                    account.update(cx, |account, cx| account.link_start(cx));
                }
            });
        } else {
            self.tick = None;
            self.code = None;
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

    /// Join a network (board 02).
    fn node_step(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let palette = look::palette(window);
        let session = self.shared.session.read(cx).get().clone();
        let account = self.shared.account.read(cx).get();
        let unlocking = matches!(self.shared.screen.read(cx).get(), Screen::Unlock { .. });
        // the address refused, the node not reached, or the key or account
        // not found on the node reached
        let problem = [&session.endpoint_error, &session.error]
            .into_iter()
            .chain(unlocking.then_some(&account.error))
            .find(|said| !said.is_empty())
            .cloned();
        let reaching = if session.connecting {
            Some(host_of(&session.reaching).to_owned())
        } else if unlocking && problem.is_none() {
            Some(host_of(&session.connected_rpc).to_owned())
        } else {
            None
        };
        let focused = self.node.read(cx).focus_handle(cx).is_focused(window);
        let mut top = div()
            .p(px(look::GUTTER))
            .flex()
            .flex_col()
            .gap(px(look::gap::LG))
            .child(
                look::text(look::BIG_TITLE, FontWeight::MEDIUM, palette.foreground())
                    .mt(px(look::gap::XL))
                    .child("Join a network"),
            )
            .child(
                look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted()).child(
                    "This phone joins an account you already have. You will approve it from a \
                     computer that is signed in.",
                ),
            )
            .child(self.field(focused, problem, palette));
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
        // nothing times a reach out: the person can always stop it
        if busy {
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
        let mut body = div()
            .id("join")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
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
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(body)
    }

    /// The address field (Expo `Field`): its label, the box (the edge 2 in
    /// the foreground focused, in danger with a problem), then the problem
    /// or the rule.
    fn field(&self, focused: bool, problem: Option<String>, palette: Palette) -> Div {
        let (edge, thick) = match (&problem, focused) {
            (Some(_), _) => (palette.danger(), 2.),
            (None, true) => (palette.foreground(), 2.),
            (None, false) => (palette.border_strong(), 1.),
        };
        let node = self.node.clone();
        let under = match problem {
            Some(problem) => {
                look::text(look::CAPTION, FontWeight::NORMAL, palette.danger()).child(problem)
            }
            None => look::text(look::CAPTION, FontWeight::NORMAL, palette.muted()).child(
                "A domain, or a host and a port, with http:// or https:// in front if you like.",
            ),
        };
        div()
            .flex()
            .flex_col()
            .child(
                look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                    .mb(px(look::gap::XS))
                    .child("Node address"),
            )
            .child(
                div()
                    .id("join/node")
                    .control(Role::Group, "Node address")
                    .min_h(px(look::CONTROL))
                    .flex()
                    .items_center()
                    .px(px(look::gap::LG - (thick - 1.)))
                    .border(px(thick))
                    .border_color(edge)
                    .bg(palette.background())
                    .on_click(move |_, window, cx| {
                        node.update(cx, |node, cx| node.focus(window, cx));
                    })
                    .child(
                        Input::new(&self.node)
                            .appearance(false)
                            .font_family(ducktape_host::fonts::FAMILY_MONO)
                            .text_size(px(look::SECONDARY.size()))
                            .py_0()
                            .px_0(),
                    ),
            )
            .child(under.mt(px(look::gap::XS)))
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

    /// This phone's code, waiting for consent (board 03): the code the
    /// computer's "Add a device…" types, the key's fingerprint both screens
    /// show, the time left; a failure says why, and New code starts over
    /// with the same key.
    fn code_step(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let palette = look::palette(window);
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
        let header = div()
            .h(px(look::HEADER))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(look::gap::SM))
            .border_b_1()
            .border_color(palette.border())
            .child(
                div()
                    .id("back")
                    .control(Role::Button, "Back")
                    .size(px(look::HIT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path("icons/arrow-left.svg")
                            .size(px(look::ICON_HIT))
                            .text_color(palette.foreground()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.leave(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
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
            );
        let said = "Open the account menu there, choose Add a device…, type this code, and \
                    approve only if both screens show the same fingerprint.";
        let named = said.find("Add a device…").unwrap_or(0);
        let words = StyledText::new(said).with_highlights([(
            named..named + "Add a device…".len(),
            HighlightStyle {
                font_weight: Some(FontWeight::MEDIUM),
                ..HighlightStyle::default()
            },
        )]);
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
                    .child("Approve this phone from your computer"),
            )
            .child(look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted()).child(words))
            .child(boxed);
        let again = cx.listener(|this, _, _, cx| this.new_code(cx));
        body = match failure {
            Some(failure) => body
                .child(
                    div()
                        .id("join/failure")
                        .control(Role::Alert, failure.clone())
                        .py(px(look::gap::MD))
                        .px(px(look::GUTTER))
                        .bg(palette.danger_soft())
                        .child(
                            look::text(look::SECONDARY, FontWeight::NORMAL, palette.danger())
                                .child(failure),
                        ),
                )
                .child(
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

impl Render for Join {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match step(*self.shared.screen.read(cx).get()) {
            Some(Step::Code) => self.code_step(window, cx),
            _ => self.node_step(window, cx),
        }
    }
}

/// A button's kind (Expo `Button`).
enum Kind {
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
fn button(
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

/// The keyboard for the address: gm is told (Android); gpui_ios raises its
/// own with the focused field, from the field's `text_input`.
fn keyboard(window: &mut Window, up: bool) {
    let want = if up {
        SoftKeyboard::Show(url_keyboard())
    } else {
        SoftKeyboard::Hide
    };
    ducktape_host::contract::os().soft_keyboard(window, want);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};

    /// Every host screen has its place on a phone: the desk, the code for the
    /// account steps, else the address (the layout question never comes up
    /// on a phone, which has one layout).
    #[test]
    fn each_screen_draws_as_the_desk_the_code_or_the_address() {
        assert_eq!(step(Screen::Desk), None);
        assert_eq!(step(Screen::Layout), Some(Step::Node));
        assert_eq!(step(Screen::Connect), Some(Step::Node));
        assert_eq!(step(Screen::Unlock { awaiting: true }), Some(Step::Node));
        let link = Screen::Account {
            step: AccountStep::Link,
        };
        assert_eq!(step(link), Some(Step::Code));
    }

    /// A code is made once the key has no account, never over a failure,
    /// a call out, or a code already shown.
    #[test]
    fn a_code_is_made_only_on_a_clear_account_step() {
        let name = Screen::Account {
            step: AccountStep::Name,
        };
        let clear = AccountState::default();
        assert!(wants_code(name, &clear));
        assert!(!wants_code(Screen::Connect, &clear));
        for account in [
            AccountState {
                error: "The relay said no.".into(),
                ..AccountState::default()
            },
            AccountState {
                busy: true,
                ..AccountState::default()
            },
            AccountState {
                link_code: "KQ4M-9XPT".into(),
                ..AccountState::default()
            },
        ] {
            assert!(!wants_code(name, &account));
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
    /// are found on the host's runtime, off the test's thread.
    fn until(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
        for _ in 0..500 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the join never got there");
    }

    /// The join as a phone walks it, over the host's own steps: a node
    /// reached (its status the test's), this phone's key opened, no account
    /// on it: a code shows; New code makes another; Back leaves for the
    /// address with the node under Recent; × there forgets it and reaches
    /// nothing.
    #[gpui_kit::test]
    fn a_code_shows_new_code_replaces_it_and_back_leaves_the_node_in_recent(
        cx: &mut TestAppContext,
    ) {
        use ducktape_host::session::tests::{source, status};
        let _seat = ducktape_host::backend::seat_serial();
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let shared = cx.update(|cx| Shared::for_test(Default::default(), Default::default(), cx));
        let (join, cx) = cx.add_window_view(|window, cx| Join::new(shared.clone(), window, cx));
        let screen = |cx: &mut VisualTestContext| shared.screen.read_with(cx, |s, _| *s.get());

        let (answers, _) = source(|_| Ok(status(1)));
        shared.session.update(cx, |session, cx| {
            session.connect_through(ORIGIN.into(), answers, cx)
        });
        until(cx, |cx| screen(cx) == Screen::Unlock { awaiting: true });
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
        until(cx, |cx| join.read_with(cx, |join, _| join.code.is_some()));
        assert_eq!(step(screen(cx)), Some(Step::Code));
        let first = join.read_with(cx, |join, _| join.code.clone().unwrap().0);

        join.update(cx, |join, cx| join.new_code(cx));
        cx.run_until_parked();
        let second = join.read_with(cx, |join, _| join.code.clone().unwrap().0);
        assert_ne!(first, second, "New code made no new code");

        join.update(cx, |join, cx| join.leave(cx));
        cx.run_until_parked();
        assert_eq!(screen(cx), Screen::Connect);
        assert!(
            join.read_with(cx, |join, _| join.code.is_none()),
            "a code outlived its network"
        );
        let recent = shared
            .session
            .read_with(cx, |session, _| session.get().recent_endpoints.clone());
        assert_eq!(recent.first().map(|node| node.url.as_str()), Some(ORIGIN));

        let forget = cx.debug_bounds("join/forget/0").expect("a Recent row's ×");
        cx.simulate_click(forget.center(), Modifiers::none());
        cx.run_until_parked();
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
}
