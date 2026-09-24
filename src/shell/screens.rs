use super::*;

/// The facts a draw reads, copied out of the model: a draw needs `cx`
/// mutably (fields, listeners) while the model is borrowed from it, so a
/// screen cannot hold `&Ducktape` as it builds. Secrets are not copied
/// here; the screen that shows one reads it itself.
#[derive(Clone)]
pub(crate) struct Facts {
    pub(crate) dark: bool,
    pub(crate) endpoint_error: String,
    pub(crate) recent_endpoints: Vec<crate::backend::RecentEndpoint>,
    pub(crate) connecting: bool,
    /// Connected, but the node stopped answering its status polls.
    pub(crate) reconnecting: bool,
    pub(crate) network: String,
    /// The node the console is on.
    pub(crate) connected_rpc: String,
    pub(crate) other_chain: bool,
    pub(crate) status: String,
    pub(crate) error: String,
    pub(crate) signer_key: String,
    pub(crate) account: Option<Option<(u64, String)>>,
    pub(crate) unlock_error: String,
    pub(crate) unlock_busy: bool,
    pub(crate) key_exists: bool,
    pub(crate) seating: bool,
    pub(crate) locked: bool,
    /// The code this device waits under for another to approve it.
    pub(crate) link_code: String,
    /// The joining key's fingerprint, once its code was found.
    pub(crate) approve_fingerprint: Option<String>,
    pub(crate) passkey_waiting: bool,
    /// The QR URL, once the person picked the phone.
    pub(crate) passkey_qr: Option<String>,
    pub(crate) phrase_quiz: Option<[usize; 3]>,
    pub(crate) active: Option<&'static str>,
    pub(crate) badges: BTreeMap<&'static str, i64>,
    pub(crate) motion: bool,
    pub(crate) appearance: crate::Appearance,
    pub(crate) overlay: Option<crate::Overlay>,
    pub(crate) spotlight_pick: usize,
    pub(crate) node: Option<crate::backend::NodeStatus>,
    pub(crate) height: i64,
    /// Seconds since the height last moved.
    pub(crate) block_age: i64,
    pub(crate) settings_page: crate::SettingsPage,
}

impl Ducktape {
    pub(crate) fn clone_facts(&self) -> Facts {
        Facts {
            dark: self.dark(),
            endpoint_error: self.endpoint_error.clone(),
            recent_endpoints: self.recent_endpoints.clone(),
            connecting: self.connecting,
            reconnecting: self.reconnecting(),
            network: self.network.clone(),
            connected_rpc: self.connected_rpc.clone(),
            other_chain: self.other_chain,
            status: self.status.clone(),
            error: self.error.clone(),
            signer_key: self.signer_key.clone(),
            account: self.account.clone(),
            unlock_error: self.sign_in.unlock_error.clone(),
            unlock_busy: self.sign_in.unlock_busy,
            key_exists: self.key_exists,
            seating: self.sign_in.seating,
            locked: self.sign_in.locked,
            link_code: self.sign_in.link_code.clone(),
            approve_fingerprint: self.approve_fingerprint(),
            passkey_waiting: self.sign_in.passkey_task.is_some(),
            passkey_qr: self.passkey_qr_shown(),
            phrase_quiz: self.sign_in.phrase_quiz,
            active: self.active,
            badges: self.badges.clone(),
            motion: self.motion,
            appearance: self.appearance,
            overlay: self.overlay,
            spotlight_pick: self.spotlight_pick,
            node: self.node.clone(),
            height: self.height,
            block_age: self.block_age(),
            settings_page: self.settings_page,
        }
    }
}

/// A raw program id read as a tab label before its manifest arrives, or
/// after it failed to: hyphens to spaces, first letter capitalised —
/// "module-registry" reads "Module registry" instead of flashing the kebab
/// id and then the manifest name.
pub(super) fn prettify(id: &str) -> String {
    let spaced = id.replace('-', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

/// A field's text reduced to what `input` compares, so a mirrored password
/// is not kept a second time in the clear.
fn digest(text: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::hash::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The avatar's letters for an account `name`: the first letter of its
/// first two words ("Ada Lovelace" → "AL", "ada" → "A"), "?" for none.
pub(super) fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    match letters.is_empty() {
        true => "?".into(),
        false => letters,
    }
}

impl DesktopWindow {
    /// A native text field; Enter dispatches `on_enter`, every change
    /// dispatches `on_change` with the text.
    ///
    /// The model owns the text; the field mirrors it. `value` reads the
    /// model's copy, and a draw writes it into the field when the MODEL
    /// moved since the two last agreed (`mirrored`): a password wiped after
    /// Unlock or Lock, a new key's form reset, the endpoint rewritten to
    /// the origin actually reached. The field state is kept per window for
    /// as long as the window lives, so without this a field would keep
    /// showing text the model no longer holds, and a retry would send
    /// something other than what is on screen.
    ///
    /// It compares against what was last agreed, not against the field's
    /// own text: keys can land in the field before their change event
    /// reaches the model (a burst of keys in one update, as the AX door's
    /// `type` sends), and a draw in between would otherwise wipe them.
    ///
    /// Its accessible name is
    /// `label`, or `placeholder` when a field's hint text already reads as
    /// one (a value-shaped placeholder like an example URL does not). Its
    /// accessible value is the field's current text — unless `secret`,
    /// which keeps that text out of the AX tree the way a masked field's
    /// `PasswordInput` role already does, for a field (the recovery
    /// phrase) that is sensitive without being visually masked.
    #[allow(clippy::too_many_arguments, reason = "one call site per field")]
    pub(super) fn input(
        &mut self,
        key: &'static str,
        placeholder: &'static str,
        masked: bool,
        value: fn(&Ducktape) -> &str,
        on_change: fn(String) -> Message,
        on_enter: fn() -> Message,
        label: Option<gpui_kit::SharedString>,
        secret: bool,
        size: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::input::{Input, InputContentType, InputEvent, InputState};
        if !self.inputs.contains_key(key) {
            let state = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(masked)
            });
            let model = self.model.clone();
            let mirrored = std::rc::Rc::new(std::cell::Cell::new(digest("")));
            let agreed = mirrored.clone();
            let subscription = cx.subscribe_in(&state, window, move |_, input, event, _, cx| {
                match event {
                    InputEvent::PressEnter { .. } => {
                        model.update(cx, |model, cx| model.dispatch(on_enter(), cx));
                    }
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        agreed.set(digest(&text));
                        model.update(cx, |model, cx| model.dispatch(on_change(text), cx));
                    }
                    _ => {}
                }
                cx.notify();
            });
            self.inputs.insert(
                key,
                NativeInput {
                    state,
                    mirrored,
                    _subscription: subscription,
                },
            );
        }
        use gpui_kit::{Focusable as _, StatefulInteractiveElement as _};
        let NativeInput {
            state, mirrored, ..
        } = &self.inputs[key];
        // set_value emits no Change, so mirroring never echoes back.
        let now = digest(value(&self.model.read(cx).state));
        if now != mirrored.get() {
            mirrored.set(now);
            let text = value(&self.model.read(cx).state).to_owned();
            state.update(cx, |state, cx| state.set_value(text, window, cx));
        }
        // bare: the canvas's box around it is `ink::field_box`; its text
        // is `size` (the canvas's `15px`, the account name's `22px`)
        // the kit fixes an input's line at `1.25rem` (20px) inside `8px`
        // padding: a larger face is clipped top and bottom. The line follows
        // the face; the canvas's box around it sets height and inset.
        let input = Input::new(state)
            .id(key)
            .appearance(false)
            .text_size(gpui_kit::px(size))
            .line_height(gpui_kit::relative(1.4))
            .py_0()
            .px_0();
        let input = match masked {
            true => input.content_type(InputContentType::Password),
            false => input,
        };
        let field = crate::a11y::text_field(
            gpui_kit::SharedString::from(format!("{key}/field")),
            &state.read(cx).focus_handle(cx),
            {
                let state = state.clone();
                move |value, window, cx| {
                    state.update(cx, |state, cx| state.replace_all(value, window, cx))
                }
            },
            input.role(gpui_kit::component::RoleOverride::Presentational),
        )
        .aria_label(label.unwrap_or_else(|| placeholder.into()));
        let field = match secret {
            true => field,
            false => field.aria_value(state.read(cx).value().to_string()),
        };
        match masked {
            true => field.role(gpui_kit::Role::PasswordInput),
            false => field.role(gpui_kit::Role::TextInput),
        }
        .into_any_element()
    }

    /// The bar at the window's foot (the Reconnecting board): the node not
    /// answering, which stays until it does, and a passing note (a toast:
    /// it fades, or is dismissed). `width: 560px; height: 56px; gap: 14px;
    /// border: 1.5px solid ink`, a soft shadow; a long note wraps.
    pub(super) fn footer(&self, cx: &gpui_kit::App) -> Option<gpui_kit::Div> {
        use super::ink::*;
        use gpui_kit::*;
        let state = &self.model.read(cx).state;
        let lost = state.reconnecting() && state.screen == Screen::Console;
        let toast = state.toast.clone();
        if !lost && toast.is_empty() {
            return None;
        }
        let ink = Ink::of(state.dark());
        let bar = |id: &'static str| {
            div()
                .id(id)
                .w(px(560.))
                .max_w_full()
                .min_h(px(56.))
                .flex()
                .items_center()
                .gap(px(14.))
                .pl(px(18.))
                .pr(px(10.))
                .py(px(10.))
                .bg(ink.bg)
                .border(px(1.5))
                .border_color(ink.ink)
                .shadow_lg()
        };
        let text = |said: String| sans(400, 14.).flex_1().min_w_0().child(said);
        let lost = lost.then(|| {
            let said = format!(
                "{} isn't answering. What you write stays here.",
                state.network
            );
            bar("reconnecting")
                .control(Role::Status, SharedString::from(said.clone()))
                .child(pulse(false, state.motion, &ink))
                .child(text(said))
                .child(self.button(
                    "reconnect",
                    "Retry now",
                    Kind::Small,
                    || Message::Tick,
                    false,
                    &ink,
                ))
        });
        let toast = (!toast.is_empty()).then(|| {
            bar("toast")
                .control(Role::Status, SharedString::from(toast.clone()))
                // `Text` hands its words to the AX value, leaving the
                // node's name empty; a reader announces the name.
                .child(
                    text(toast.clone())
                        .id("toast-message")
                        .control(Role::Label, SharedString::from(toast)),
                )
                .child(self.button(
                    "toast-dismiss",
                    "Dismiss",
                    Kind::Small,
                    || Message::DismissToast,
                    false,
                    &ink,
                ))
        });
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(32.))
                .px_4()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .children(lost)
                .children(toast),
        )
    }

    /// Main (Connect): an address, the nodes reached before, and why the
    /// last try did not land.
    pub(super) fn connect(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::{self, *};
        use gpui_kit::*;
        let state = self.model.read(cx).state.clone_facts();
        let ink = Ink::of(state.dark);
        let field = self.input(
            "endpoint",
            "127.0.0.1:8844",
            false,
            |state| &state.endpoint,
            Message::EndpointTyped,
            || Message::ConnectSubmit,
            Some("Node address".into()),
            false,
            15.,
            window,
            cx,
        );
        // The address refusal is about what is typed now; a failed try is
        // about the last address. Both clear on the next keystroke or try.
        let note = [&state.endpoint_error, &state.error]
            .into_iter()
            .find(|note| !note.is_empty())
            .cloned();
        let border = match note {
            Some(_) => ink.danger,
            None => ink.strong,
        };
        // Only a try in flight has a status worth reading.
        let below = match (&note, state.connecting) {
            (Some(note), _) => Some(self.alert("connect-error", note.clone(), &ink)),
            (None, true) => Some(
                ink::note(state.status.clone(), ink.muted)
                    .id("connect-status")
                    .role(Role::Status)
                    .aria_label(state.status.clone())
                    .into_any_element(),
            ),
            (None, false) => None,
        };
        let form = self.field(
            "Node address",
            div()
                .flex()
                .gap(px(8.))
                .child(div().flex_1().child(
                    field_box(field, border, 44., &ink).font_family(super::theme::FAMILY_MONO),
                ))
                .child(self.button(
                    "connect",
                    "Connect",
                    Kind::Primary,
                    || Message::ConnectSubmit,
                    state.connecting,
                    &ink,
                ))
                .into_any_element(),
            below,
            &ink,
        );
        let rows = state.recent_endpoints.iter().map(|entry| {
            let pick_model = self.model.clone();
            let pick_target = entry.url.clone();
            let forget_model = self.model.clone();
            let forget_target = entry.url.clone();
            let name = entry.name();
            let danger = ink.danger;
            // `display: flex; align-items: baseline; gap: 16px;
            // padding: 12px 0; border-bottom: 1px solid line`
            let pick = div()
                .id(SharedString::from(format!("recent/{}", entry.url)))
                .control(Role::Button, SharedString::from(entry.label()))
                .cursor_pointer()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(px(16.))
                .hover(|style| style.opacity(0.7))
                .on_click(move |_, _, cx| {
                    let target = pick_target.clone();
                    pick_model.update(cx, |model, cx| {
                        model.dispatch(Message::ConnectTo(target), cx)
                    })
                })
                .child(
                    div()
                        .w(px(120.))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(sans(500, 15.).truncate().child(name))
                        .when(entry.other_chain, |cell| {
                            cell.child(sans(400, 12.).text_color(ink.muted).child("Other chain"))
                        }),
                )
                .child(
                    mono(400, 13.)
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ink.muted)
                        .child(entry.host().to_owned()),
                );
            let forget = div()
                .id(SharedString::from(format!("forget/{}", entry.url)))
                .control(
                    Role::Button,
                    SharedString::from(format!("Forget {}", entry.url)),
                )
                .cursor_pointer()
                .flex_shrink_0()
                .child(
                    sans(400, 16.)
                        .text_color(ink.muted)
                        .hover(move |style| style.text_color(danger))
                        .child("×"),
                )
                .on_click(move |_, _, cx| {
                    let target = forget_target.clone();
                    forget_model.update(cx, |model, cx| {
                        model.dispatch(Message::ForgetEndpoint(target), cx)
                    })
                });
            // Both have a tab stop: a keyboard-only reader reaches a node
            // used before, and can forget it.
            div()
                .flex()
                .items_baseline()
                .gap(px(16.))
                .py(px(12.))
                .border_b_1()
                .border_color(ink.line)
                .child(crate::a11y::keyboard(pick))
                .child(crate::a11y::keyboard(forget))
        });
        let recent = (!state.recent_endpoints.is_empty()).then(|| {
            div()
                .id("recent")
                .flex()
                .flex_col()
                .child(
                    tag("Recent", &ink)
                        .pb(px(8.))
                        .border_b_1()
                        .border_color(ink.line),
                )
                .children(rows)
                .into_any_element()
        });
        let caption = match state.recent_endpoints.is_empty() {
            true => "No network yet.",
            false => "A node, drawn in characters.",
        };
        self.launcher(
            "connect",
            figure::Figure::Node,
            caption.into(),
            None,
            "[01 / 04] Network".into(),
            "Connect to a network".into(),
            Some("Any node on it will do. The node serves the programs you use and keeps your account.".into()),
            std::iter::once(form.into_any_element()).chain(recent).collect(),
            window,
            cx,
        )
    }
}

/// The node, as a breath (the canvas's `.pulse`): 5px to 11px and back
/// over 2s, green to its soft tone, while it answers; red, 5px to 9px over
/// 0.8s while it does not. An 8px dot when motion is off.
pub(super) fn pulse(ok: bool, moving: bool, ink: &super::ink::Ink) -> gpui_kit::AnyElement {
    use gpui_kit::*;
    let (color, soft, period, big) = match ok {
        true => (ink.ok, ink.ok_soft, 2000, 11.),
        false => (ink.danger, ink.danger, 800, 9.),
    };
    let dot = div().rounded_full().flex_shrink_0();
    let well = div()
        .size(px(12.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    match moving {
        false => well.child(dot.size(px(8.)).bg(color)).into_any_element(),
        true => well
            .child(
                dot.with_animation(
                    SharedString::from(format!("pulse/{ok}")),
                    Animation::new(std::time::Duration::from_millis(period))
                        .repeat()
                        .with_easing(bounce(ease_in_out))
                        .with_max_fps(30.),
                    move |dot, delta| {
                        dot.size(px(5. + (big - 5.) * delta))
                            .bg(color.blend(soft.opacity(delta)))
                    },
                ),
            )
            .into_any_element(),
    }
}
