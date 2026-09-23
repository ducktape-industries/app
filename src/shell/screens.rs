use super::*;

/// The facts a draw reads, copied out so the model lock is not held while
/// elements are built.
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
    pub(crate) network_menu: bool,
    pub(crate) status: String,
    pub(crate) error: String,
    pub(crate) signer_key: String,
    pub(crate) account: Option<Option<(u64, String)>>,
    pub(crate) unlock_error: String,
    pub(crate) unlock_busy: bool,
    pub(crate) key_exists: bool,
    pub(crate) browsing: bool,
    pub(crate) restoring: bool,
    pub(crate) passkey_waiting: bool,
    pub(crate) account_step: bool,
    /// The QR URL, once the person picked the phone.
    pub(crate) passkey_qr: Option<String>,
    pub(crate) replacing: bool,
    pub(crate) phrase: String,
    pub(crate) phrase_quiz: Option<[usize; 3]>,
    pub(crate) active: Option<&'static str>,
    pub(crate) badges: BTreeMap<&'static str, i64>,
    pub(crate) motion: bool,
    pub(crate) appearance: crate::Appearance,
    pub(crate) popover: Option<crate::Popover>,
    pub(crate) spotlight: bool,
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
            network_menu: self.network_menu,
            status: self.status.clone(),
            error: self.error.clone(),
            signer_key: self.signer_key.clone(),
            account: self.account.clone(),
            unlock_error: self.unlock_error.clone(),
            unlock_busy: self.unlock_busy,
            key_exists: self.key_exists,
            browsing: self.browsing,
            restoring: self.restoring,
            passkey_waiting: self.passkey_task.is_some(),
            account_step: self.account_step,
            passkey_qr: (self.passkey_task.is_some()
                && self
                    .passkey_phone
                    .load(std::sync::atomic::Ordering::Relaxed)
                && !self.passkey_qr.is_empty())
            .then(|| self.passkey_qr.clone()),
            replacing: self.replacing,
            phrase: self.phrase.clone(),
            phrase_quiz: self.phrase_quiz,
            active: self.active,
            badges: self.badges.clone(),
            motion: self.motion,
            appearance: self.appearance,
            popover: self.popover,
            spotlight: self.spotlight,
            spotlight_pick: self.spotlight_pick,
            node: self.node.clone(),
            height: self.height,
            block_age: self.wall_now - self.block_seen,
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
        use gpui_kit::component::Sizable as _;
        // as tall as the square buttons beside it
        let input = Input::new(state).id(key).large();
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

    pub(super) fn action(
        &self,
        key: impl Into<gpui_kit::ElementId>,
        label: impl Into<gpui_kit::SharedString>,
        message: fn() -> Message,
        disabled: bool,
    ) -> gpui_kit::component::button::Button {
        use gpui_kit::component::Disableable as _;
        let model = self.model.clone();
        let button = gpui_kit::component::button::Button::new(key)
            .label(label)
            .disabled(disabled)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                model.update(cx, |model, cx| model.dispatch(message(), cx))
            });
        crate::a11y::disabled(button, disabled)
    }

    /// The bar at the window's foot, for what the app has to say: the
    /// node not answering (it stays until the node does), and a passing
    /// note (a toast: it fades, or is dismissed). Centered, and as wide as
    /// its words need up to a limit: a long note wraps instead of running
    /// off the window's edge.
    pub(super) fn footer(&self, cx: &gpui_kit::App) -> Option<gpui_kit::Div> {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let state = &self.model.read(cx).state;
        let lost = state.reconnecting() && state.screen == Screen::Console;
        let toast = state.toast.clone();
        if !lost && toast.is_empty() {
            return None;
        }
        let palette = design::palette(state.dark());
        let bar = |id: &'static str| {
            div()
                .id(id)
                .w(px(560.))
                .max_w_full()
                .flex()
                .items_center()
                .gap_3()
                .pl_4()
                .pr_2()
                .py_2()
                .bg(hsla_of(palette.background))
                .border(px(1.5))
                .border_color(hsla_of(palette.foreground))
                .shadow_md()
                .text_size(px(13.))
        };
        let lost = lost.then(|| {
            let said = format!(
                "{} isn't answering. What you write stays here.",
                state.network
            );
            bar("reconnecting")
                .control(Role::Status, SharedString::from(said.clone()))
                .child(pulse(false, state.motion, palette))
                .child(div().flex_1().min_w_0().child(said))
                .child(
                    launcher::square(
                        self.action("reconnect", "Retry now", || Message::Tick, false)
                            .outline(),
                    )
                    .h(px(32.)),
                )
        });
        let toast = (!toast.is_empty()).then(|| {
            bar("toast")
                .control(Role::Status, SharedString::from(toast.clone()))
                .child(
                    // `Text` hands its words to the AX value, leaving the
                    // node's name empty; a reader announces the name.
                    div()
                        .id("toast-message")
                        .control(Role::Label, SharedString::from(toast.clone()))
                        .flex_1()
                        .min_w_0()
                        .child(toast),
                )
                .child(
                    self.action("toast-dismiss", "Dismiss", || Message::DismissToast, false)
                        .ghost()
                        .h_7(),
                )
        });
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(24.))
                .px_4()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .children(lost)
                .children(toast),
        )
    }

    /// Reaching a node: an address, the ones used before, and why the
    /// last try did not land.
    pub(super) fn connect(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let state = self.model.read(cx).state.clone_facts();
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let field = self.input(
            "endpoint",
            "127.0.0.1:8844",
            false,
            |state| &state.endpoint,
            Message::EndpointTyped,
            || Message::ConnectSubmit,
            Some("Node address".into()),
            false,
            window,
            cx,
        );
        let danger = hsla_of(design::palette(state.dark).danger);
        let recent = state.recent_endpoints.iter().map(|entry| {
            let label = entry.label();
            let row_model = self.model.clone();
            let row_target = entry.url.clone();
            let forget_model = self.model.clone();
            let forget_target = entry.url.clone();
            let name = match entry.network.is_empty() {
                true => entry.host().to_owned(),
                false => entry.network.clone(),
            };
            let pick = div()
                .id(SharedString::from(format!("recent/{}", entry.url)))
                .control(Role::Button, SharedString::from(label.clone()))
                .cursor_pointer()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap_4()
                .py(px(10.))
                .hover(|style| style.opacity(0.7))
                .on_click(move |_, _, cx| {
                    let target = row_target.clone();
                    row_model.update(cx, |model, cx| {
                        model.dispatch(Message::ConnectTo(target), cx)
                    })
                })
                .child(
                    div()
                        .w(px(120.))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(name),
                        )
                        .when(entry.other_chain, |cell| {
                            cell.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.muted_foreground)
                                    .child("Other chain"),
                            )
                        }),
                )
                .child(
                    launcher::mono(entry.host().to_owned(), colors.muted_foreground)
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                );
            let forget = div()
                .id(SharedString::from(format!("forget/{}", entry.url)))
                .control(
                    Role::Button,
                    SharedString::from(format!("Forget {}", entry.url)),
                )
                .cursor_pointer()
                .flex_shrink_0()
                .px_2()
                .text_size(px(15.))
                .text_color(colors.muted_foreground)
                .hover(move |style| style.text_color(danger))
                .on_click(move |_, _, cx| {
                    let target = forget_target.clone();
                    forget_model.update(cx, |model, cx| {
                        model.dispatch(Message::ForgetEndpoint(target), cx)
                    })
                })
                .child("×");
            // Neither row had a tab stop: a keyboard-only reader could never
            // reach a previously-used node, or forget one, from this list.
            div()
                .flex()
                .items_center()
                .gap_1()
                .border_b_1()
                .border_color(colors.border)
                .child(crate::a11y::keyboard(pick))
                .child(crate::a11y::keyboard(forget))
        });
        // The address refusal is about what is typed now; a failed try is
        // about the last address. Both clear on the next keystroke or try.
        let note = [&state.endpoint_error, &state.error]
            .into_iter()
            .find(|note| !note.is_empty())
            .cloned();
        let form = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                self.field(
                    "Node address",
                    div()
                        .flex()
                        .gap_2()
                        .child(div().flex_1().child(field))
                        .child(launcher::square(
                            self.action(
                                "connect",
                                "Connect",
                                || Message::ConnectSubmit,
                                state.connecting,
                            )
                            .primary(),
                        ))
                        .into_any_element(),
                ),
            )
            // Only a try in flight has a status worth reading; before one,
            // "Not connected" beside Connect is noise.
            .when(state.connecting, |form| {
                form.child(
                    div()
                        .id("connect-status")
                        .role(Role::Status)
                        .aria_label(state.status.clone())
                        .text_size(px(13.))
                        .text_color(colors.muted_foreground)
                        .child(state.status.clone()),
                )
            })
            .children(note.map(|note| self.alert("connect-error", note, cx)));
        let recent = (!state.recent_endpoints.is_empty()).then(|| {
            div()
                .id("recent")
                .flex()
                .flex_col()
                .child(
                    launcher::mono("Recent", colors.muted_foreground)
                        .pb_2()
                        .border_b_1()
                        .border_color(colors.border),
                )
                .children(recent)
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

/// The node, as a breath: slow and green while it answers, quick and red
/// while it does not. Still when motion is off.
pub(super) fn pulse(ok: bool, moving: bool, palette: &design::Palette) -> gpui_kit::AnyElement {
    use gpui_kit::*;
    let (color, soft, period, big) = match ok {
        true => (palette.success, palette.success_soft, 2000, 11.),
        false => (palette.danger, palette.danger_soft, 800, 9.),
    };
    let (color, soft) = (hsla_of(color), hsla_of(soft));
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
                        let size = 5. + (big - 5.) * delta;
                        let ink = match ok {
                            true => color.blend(soft.opacity(delta)),
                            false => color,
                        };
                        dot.size(px(size)).bg(ink)
                    },
                ),
            )
            .into_any_element(),
    }
}
