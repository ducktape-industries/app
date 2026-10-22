//! The native text fields the shell's layers type into, as an element over
//! a field's state (`bare`). Each field is made with the layer that draws
//! it and owns what is typed: every change goes out from its event
//! (`NativeInput::new`), and nothing is written into it at a draw. What
//! the model resets without typing (a password wiped as its step goes, the
//! address rewritten to the node reached) the layer writes into the field
//! from its observers.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement as _, Styled as _, Subscription, Window,
};

pub(in crate::shell) struct NativeInput {
    pub(in crate::shell) state: Entity<InputState>,
    masked: bool,
    _events: Subscription,
}

pub(in crate::shell) struct TextField {
    pub(in crate::shell) label: gpui_kit::SharedString,
    pub(in crate::shell) private: bool,
    pub(in crate::shell) error: Option<String>,
    pub(in crate::shell) size: f32,
}

impl NativeInput {
    pub(in crate::shell) fn new<V: 'static>(
        placeholder: &'static str,
        masked: bool,
        on_change: impl Fn(&mut V, String, &mut Context<V>) + 'static,
        on_enter: impl Fn(&mut V, &mut Context<V>) + 'static,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Self {
        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .masked(masked)
        });
        let events = cx.subscribe_in(&state, window, move |this, input, event, _, cx| {
            match event {
                InputEvent::PressEnter { .. } => on_enter(this, cx),
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    on_change(this, text, cx);
                }
                _ => {}
            }
            cx.notify();
        });
        Self {
            state,
            masked,
            _events: events,
        }
    }

    pub(in crate::shell) fn wipe(&self, window: &mut Window, cx: &mut gpui_kit::App) {
        self.set(String::new(), window, cx);
    }

    pub(in crate::shell) fn set(&self, text: String, window: &mut Window, cx: &mut gpui_kit::App) {
        if *self.state.read(cx).value() != *text {
            self.state
                .update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }

    pub(in crate::shell) fn focused(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        use gpui_kit::Focusable as _;
        self.state.read(cx).focus_handle(cx).is_focused(window)
    }

    pub(in crate::shell) fn input(
        &self,
        key: &'static str,
        field: TextField,
        cx: &gpui_kit::App,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::StatefulInteractiveElement as _;
        let TextField {
            label,
            private,
            error,
            size,
        } = field;
        let masked = self.masked;
        let role = match masked {
            true => gpui_kit::Role::PasswordInput,
            false => gpui_kit::Role::TextInput,
        };
        let native = || {
            inspected(
                &self.state,
                Some(role),
                Some(label.clone()),
                masked || private,
                cx,
            )
        };
        let element = crate::a11y::inspected(bare(key, &self.state, size, masked, cx), native)
            .aria_label(label);
        let element = match masked {
            true => element,
            false => element.aria_value(self.state.read(cx).value().to_string()),
        };
        let mut patch = crate::a11y::Patch::default().required();
        if private {
            patch = patch.class_name(crate::a11y::AX_PRIVATE);
        }
        let element = match error {
            Some(error) => {
                patch = patch.invalid();
                element.aria_description(error)
            }
            None => element,
        };
        patch
            .on(match masked {
                true => element.role(gpui_kit::Role::PasswordInput),
                false => element.role(gpui_kit::Role::TextInput),
            })
            .into_any_element()
    }
}

pub(in crate::shell) fn bare(
    key: &'static str,
    state: &Entity<InputState>,
    size: f32,
    masked: bool,
    cx: &gpui_kit::App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    use gpui_kit::Focusable as _;
    use gpui_kit::component::input::{Input, InputContentType};
    let input = Input::new(state)
        .id(key)
        .appearance(false)
        .text_size(gpui_kit::px(super::super::ink::fit(size)))
        .line_height(gpui_kit::relative(1.4))
        .py_0()
        .px_0();
    let input = match masked {
        true => input.content_type(InputContentType::Password),
        false => input,
    };
    crate::a11y::text_field(
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
}

pub(in crate::shell) fn inspected(
    state: &Entity<InputState>,
    role: Option<gpui_kit::Role>,
    label: Option<gpui_kit::SharedString>,
    hidden: bool,
    cx: &gpui_kit::App,
) -> crate::a11y::Native {
    let set = state.clone();
    crate::a11y::Native {
        role,
        label,
        value: (!hidden).then(|| state.read(cx).value().to_string()),
        set_value: Some(std::rc::Rc::new(
            move |value: String, window: &mut Window, cx: &mut gpui_kit::App| {
                set.update(cx, |state, cx| state.replace_all(value, window, cx))
            },
        )),
    }
}
