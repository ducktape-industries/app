use super::*;

/// The facts a draw reads, copied out so the model lock is not held while
/// elements are built.
#[derive(Clone)]
pub(crate) struct Facts {
    pub(crate) dark: bool,
    pub(crate) endpoint: String,
    pub(crate) endpoint_error: String,
    pub(crate) recent_endpoints: Vec<crate::backend::RecentEndpoint>,
    pub(crate) connected: bool,
    pub(crate) connecting: bool,
    pub(crate) network: String,
    pub(crate) status: String,
    pub(crate) error: String,
    pub(crate) signer_key: String,
    pub(crate) unlock_error: String,
    pub(crate) unlock_busy: bool,
    pub(crate) key_exists: bool,
    pub(crate) browsing: bool,
    pub(crate) restoring: bool,
    pub(crate) passkey_waiting: bool,
    pub(crate) phrase: String,
    pub(crate) active: Option<&'static str>,
    pub(crate) badges: BTreeMap<&'static str, i64>,
}

impl Ducktape {
    pub(crate) fn clone_facts(&self) -> Facts {
        Facts {
            dark: self.dark(),
            endpoint: self.endpoint.clone(),
            endpoint_error: self.endpoint_error.clone(),
            recent_endpoints: self.recent_endpoints.clone(),
            connected: self.connected,
            connecting: self.connecting,
            network: self.network.clone(),
            status: self.status.clone(),
            error: self.error.clone(),
            signer_key: self.signer_key.clone(),
            unlock_error: self.unlock_error.clone(),
            unlock_busy: self.unlock_busy,
            key_exists: self.key_exists,
            browsing: self.browsing,
            restoring: self.restoring,
            passkey_waiting: self.passkey_task.is_some(),
            phrase: self.phrase.clone(),
            active: self.active,
            badges: self.badges.clone(),
        }
    }
}

/// A raw program id read as a rail label before its manifest arrives, or
/// after it failed to: hyphens to spaces, first letter capitalised —
/// "module-registry" reads "Module registry" instead of flashing the kebab
/// id and then the manifest name.
fn prettify(id: &str) -> String {
    let spaced = id.replace('-', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

impl DesktopWindow {
    /// A native text field; Enter dispatches `on_enter`, every change
    /// dispatches `on_change` with the text. Its accessible name is
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
        initial: &str,
        on_change: fn(String) -> Message,
        on_enter: fn() -> Message,
        label: Option<&'static str>,
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
            state.update(cx, |state, cx| {
                state.set_value(initial.to_owned(), window, cx)
            });
            let model = self.model.clone();
            let subscription = cx.subscribe_in(&state, window, move |_, input, event, _, cx| {
                match event {
                    InputEvent::PressEnter { .. } => {
                        model.update(cx, |model, cx| model.dispatch(on_enter(), cx));
                    }
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
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
                    _subscription: subscription,
                },
            );
        }
        use gpui_kit::{Focusable as _, StatefulInteractiveElement as _};
        let state = &self.inputs[key].state;
        let input = Input::new(state).id(key);
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
        .aria_label(label.unwrap_or(placeholder));
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

    pub(super) fn toast(&self, cx: &gpui_kit::App) -> Option<gpui_kit::Stateful<gpui_kit::Div>> {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let toast = self.model.read(cx).state.toast.clone();
        if toast.is_empty() {
            return None;
        }
        let theme = gpui_kit::component::Theme::global(cx);
        Some(
            div()
                .id("toast")
                .role(Role::Status)
                .absolute()
                .bottom_4()
                .right_4()
                .max_w(px(420.))
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_2p5()
                .rounded(px(design::radius::CARD as f32))
                .border_1()
                .border_color(theme.color_tokens().border)
                .bg(theme.popover)
                .shadow_md()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.5))
                        .child(Text::new("toast-message".into(), toast.into())),
                )
                .child(
                    self.action("toast-dismiss", "Dismiss", || Message::DismissToast, false)
                        .ghost()
                        .h_7(),
                ),
        )
    }

    /// Reaching a node: a URL, the ones used before, and why the last try
    /// did not land.
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
            "http://127.0.0.1:8844",
            false,
            &state.endpoint,
            Message::EndpointTyped,
            || Message::ConnectSubmit,
            Some("Node address"),
            false,
            window,
            cx,
        );
        let recent = state.recent_endpoints.iter().map(|entry| {
            let label = match entry.network.is_empty() {
                true => entry.url.clone(),
                false => format!("{} — {}", entry.network, entry.url),
            };
            let row_model = self.model.clone();
            let row_target = entry.url.clone();
            let forget_model = self.model.clone();
            let forget_target = entry.url.clone();
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .id(SharedString::from(format!("recent/{}", entry.url)))
                        .control(Role::Button, SharedString::from(label.clone()))
                        .cursor_pointer()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .px_2()
                        .py_1()
                        .rounded(px(design::radius::CONTROL as f32))
                        .text_size(px(12.5))
                        .text_color(colors.muted_foreground)
                        .hover(|style| style.text_color(colors.foreground))
                        .on_click(move |_, _, cx| {
                            let target = row_target.clone();
                            row_model.update(cx, |model, cx| {
                                model.dispatch(Message::ConnectTo(target), cx)
                            })
                        })
                        .child(label),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("forget/{}", entry.url)))
                        .control(
                            Role::Button,
                            SharedString::from(format!("Forget {}", entry.url)),
                        )
                        .cursor_pointer()
                        .flex_shrink_0()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(design::radius::CONTROL as f32))
                        .text_size(px(12.5))
                        .text_color(colors.muted_foreground)
                        .hover(|style| {
                            style.text_color(hsla_of(design::palette(state.dark).danger))
                        })
                        .on_click(move |_, _, cx| {
                            let target = forget_target.clone();
                            forget_model.update(cx, |model, cx| {
                                model.dispatch(Message::ForgetEndpoint(target), cx)
                            })
                        })
                        .child("×"),
                )
        });
        let note = match (!state.error.is_empty(), !state.endpoint_error.is_empty()) {
            (true, _) => Some(state.error.clone()),
            (_, true) => Some(state.endpoint_error.clone()),
            _ => None,
        };
        div()
            .id("connect")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(460.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Connect to a node"),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(colors.muted_foreground)
                            .child("The node serves everything you will see: its programs, their views, your account."),
                    )
                    .child(field)
                    .child(
                        div().flex().gap_2().child(
                            self.action("connect", "Connect", || Message::ConnectSubmit, state.connecting)
                                .primary(),
                        )
                        .child(div().flex_1().text_size(px(12.5)).text_color(colors.muted_foreground).child(state.status.clone())),
                    )
                    .children(note.map(|note| {
                        div()
                            .id("connect-error")
                            .role(Role::Alert)
                            .aria_label(note.clone())
                            .text_size(px(12.5))
                            .text_color(hsla_of(design::palette(state.dark).danger))
                            .child(note)
                    }))
                    .when(!state.recent_endpoints.is_empty(), |card| {
                        card.child(
                            div()
                                .mt_2()
                                .text_size(px(11.))
                                .text_color(colors.muted_foreground)
                                .child("Recent"),
                        )
                        .children(recent)
                    }),
            )
            .into_any_element()
    }

    /// Inside a node: the rail of its programs on the left, the open one's
    /// view on the right, and the key's lock at the foot. Reached with a
    /// seated key or by choosing to read without one (`sign_in`).
    pub(super) fn console(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let state = self.model.read(cx).state.clone_facts();
        let sidebar_bg = {
            let theme = gpui_kit::component::Theme::global(cx);
            theme.sidebar
        };
        let palette = design::palette(state.dark);
        let ink_fg = hsla_of(palette.sidebar_foreground);
        let ink_muted = hsla_of(palette.sidebar_muted);
        let ink_raised = hsla_of(palette.sidebar_raised);
        let ink_border = hsla_of(palette.sidebar_border);
        let accent = hsla_of(palette.accent);
        let faint = hsla_of(palette.faint);
        // The kit's `ghost()` variant reads its text in the page's theme
        // foreground, which is dark for the light content area — invisible
        // on this always-dark rail. Same rail row colours, hover included.
        let rail_button = gpui_kit::component::button::ButtonCustomVariant::new(cx)
            .foreground(ink_muted)
            .hover(ink_raised);
        let rail = crate::runtime::rail();
        if rail.iter().any(|row| row.note == Some("Loading")) {
            window.request_animation_frame();
        }
        // Below the threshold, a fixed `RAIL_WIDTH` left the open program's
        // own pane narrower than `layout::MIN_PANE_WIDTH` — the rail was
        // fine, the pane behind it was not. Collapse the rail to its rows'
        // own initials instead: same ids, same AX names and roles, fewer
        // pixels.
        let narrow = window.viewport_size().width < px(NARROW_WINDOW_WIDTH);
        self.initialize_panes(
            state
                .active
                .or_else(|| rail.iter().find(|row| !row.empty).map(|row| row.module)),
            cx,
        );
        let active = self
            .layout
            .panes
            .get(self.layout.focused)
            .map(|pane| pane.module);
        let rows = rail.iter().filter(|row| !row.empty).map(|row| {
            let module = row.module;
            let selected = active == Some(module);
            let badge = state.badges.get(module).copied().unwrap_or(0);
            // Before the manifest lands (or if it never does) the label is
            // the program's own id: shown prettified, with a small dot for
            // status rather than a "· Loading" suffix that would otherwise
            // flash and vanish once the real name arrives.
            let shown = match row.note {
                Some(_) => prettify(&row.label),
                None => row.label.clone(),
            };
            let name = match row.note {
                Some(note) => format!("{shown} · {note}"),
                None => shown.clone(),
            };
            let initial = shown.chars().next().map(String::from).unwrap_or_default();
            div()
                .id(SharedString::from(format!("rail/{module}")))
                .control(Role::Tab, SharedString::from(name))
                .aria_selected(selected)
                .focusable()
                .tab_stop(true)
                .flex()
                .items_center()
                .when(narrow, |row| row.justify_center())
                .gap_2()
                .h(px(28.))
                .px_2()
                .when(narrow, |row| row.px_0())
                .mb_0p5()
                .rounded(px(design::radius::CONTROL as f32))
                .cursor_pointer()
                .text_size(px(13.))
                .font_weight(match selected {
                    true => FontWeight::MEDIUM,
                    false => FontWeight::NORMAL,
                })
                .text_color(match selected {
                    true => ink_fg,
                    false => ink_muted,
                })
                .when(selected, |row| row.bg(ink_raised))
                .hover(move |style| style.bg(ink_raised).text_color(ink_fg))
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    let message = if event.modifiers().shift {
                        Message::SplitView(module)
                    } else {
                        Message::SelectView(module)
                    };
                    this.pane_message(message, window, cx);
                }))
                .map(|built| match narrow {
                    // The rail's id and AX name (`.control` above) still
                    // name the whole program; only the sighted label
                    // shrinks to its initial.
                    true => built.child(div().child(initial)),
                    false => built
                        .child(div().flex_1().min_w_0().truncate().child(shown))
                        .when(row.note == Some("Loading"), |row| {
                            row.child(
                                div()
                                    .size(px(6.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(ink_muted),
                            )
                        })
                        .when(row.note == Some("Failed"), |row| {
                            row.child(
                                div()
                                    .size(px(6.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(hsla_of(palette.danger)),
                            )
                        })
                        .when(badge > 0, |row| {
                            row.child(
                                div()
                                    .px_1p5()
                                    .rounded_full()
                                    .bg(accent)
                                    .text_size(px(10.))
                                    .text_color(hsla_of(palette.background))
                                    .child(badge.to_string()),
                            )
                        }),
                })
        });
        let rows: Vec<_> = rows.collect();
        let unlocked = !state.signer_key.is_empty();
        // The key's lock; signing in itself is a screen of its own (sign_in.rs).
        let foot = match unlocked {
            true => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(ink_muted)
                        .truncate()
                        .child(format!(
                            "key {}…",
                            &state.signer_key[..state.signer_key.len().min(12)]
                        )),
                )
                .child(
                    self.action("lock", "Lock", || Message::Lock, false)
                        .custom(rail_button)
                        .w_full(),
                ),
            false => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(ink_muted)
                        .child("Reading without a key"),
                )
                .child(
                    self.action("sign-in", "Sign in", || Message::SignIn, false)
                        .primary()
                        .w_full(),
                ),
        };
        let sidebar = div()
            .id("rail")
            .w(px(if narrow {
                RAIL_COMPACT_WIDTH
            } else {
                RAIL_WIDTH
            }))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(sidebar_bg)
            .border_r_1()
            .border_color(ink_border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .when(narrow, |row| row.justify_center())
                    .gap_2()
                    .px(px(if narrow { 0. } else { 12. }))
                    .pt(px(if cfg!(target_os = "macos") { 44. } else { 12. }))
                    .pb_2()
                    .child(
                        div()
                            .id("rail-connection")
                            .control(
                                Role::Status,
                                SharedString::from(if state.connected {
                                    "Connected"
                                } else {
                                    "Not connected"
                                }),
                            )
                            .size(px(6.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(if state.connected { accent } else { faint }),
                    )
                    .when(!narrow, |row| {
                        row.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ink_fg)
                                        .truncate()
                                        .child(state.network.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(ink_muted)
                                        .truncate()
                                        .child(state.status.clone()),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .id("rail-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .role(Role::TabList)
                    .children(rows)
                    .when(rail.is_empty(), |list| {
                        list.child(
                            div()
                                .px_2()
                                .text_size(px(12.))
                                .text_color(ink_muted)
                                .child("No programs listed yet"),
                        )
                    }),
            )
            .child(
                div()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(ink_border)
                    // THE HOST'S OWN WORD ON WHAT IS RECORDING: a seated view
                    // draws inside its seat and can neither paint here nor
                    // decline to be listed.
                    .when_some(crate::runtime::capturing(), |rail, recording| {
                        rail.child(
                            div()
                                .id("capture-indicator")
                                .role(Role::Status)
                                .mb_1()
                                .px_1p5()
                                .py_0p5()
                                .rounded_full()
                                .bg(hsla_of(palette.danger))
                                .text_size(px(10.))
                                .text_color(hsla_of(palette.background))
                                .child(recording),
                        )
                    })
                    .child(foot)
                    .child(
                        self.action("disconnect", "Switch node", || Message::Disconnect, false)
                            .custom(rail_button)
                            .w_full()
                            .mt_1(),
                    ),
            );
        let seat = self.pane_stage(window, cx);
        div()
            .id("console")
            .size_full()
            .flex()
            .when(self.kind == crate::shell::WindowKind::Console, |frame| {
                frame.child(sidebar)
            })
            .child(div().id("seat").flex_1().min_w_0().h_full().child(seat))
            .children(self.toast(cx))
            .into_any_element()
    }
}
