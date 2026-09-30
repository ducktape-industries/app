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

/// One native text field, made with the layer that draws it and kept as
/// long as that layer.
pub(in crate::shell) struct NativeInput {
    pub(in crate::shell) state: Entity<InputState>,
    /// Drawn as dots, with the password role.
    masked: bool,
    _events: Subscription,
}

/// How a layer draws one of its fields (`NativeInput::input`).
pub(in crate::shell) struct TextField {
    /// Its accessible name.
    pub(in crate::shell) label: gpui_kit::SharedString,
    /// Sensitive without being masked (the recovery phrase): its value is
    /// read aloud, and the test door masks it (`a11y::AX_PRIVATE`).
    pub(in crate::shell) private: bool,
    /// The error the screen draws with it: the field reports it invalid,
    /// and says the error as its description (AX-108).
    pub(in crate::shell) error: Option<String>,
    /// The text size, in the canvas's px (`ink::fit` scales it).
    pub(in crate::shell) size: f32,
}

impl NativeInput {
    /// A field of the layer `V`, its id `key`: every change hands the text
    /// to `on_change`, Enter calls `on_enter`, and the layer draws again.
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

    /// Empties the field, when it holds anything; no change goes out.
    pub(in crate::shell) fn wipe(&self, window: &mut Window, cx: &mut gpui_kit::App) {
        self.set(String::new(), window, cx);
    }

    /// Puts `text` in the field, when it holds something else; no change
    /// goes out (`set_value` emits none).
    pub(in crate::shell) fn set(&self, text: String, window: &mut Window, cx: &mut gpui_kit::App) {
        if *self.state.read(cx).value() != *text {
            self.state
                .update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }

    /// Whether the field holds focus, for the box drawn around it
    /// ([`crate::a11y::around_field`]).
    pub(in crate::shell) fn focused(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        use gpui_kit::Focusable as _;
        self.state.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The field `key`, drawn as `field` says.
    ///
    /// Its accessible name is `label`. Its accessible value is the field's
    /// current text — unless masked: a `PasswordInput` shows no text, so it
    /// gives none (the door's mask is not the platform's; a value set here
    /// reaches every AX client). `private` marks a field (the recovery
    /// phrase) that is sensitive without being visually masked
    /// [`crate::a11y::AX_PRIVATE`]: assistive technology reads the text on
    /// the screen; the test door masks it.
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
        let element = bare(key, &self.state, size, masked, cx).aria_label(label);
        let element = match masked {
            true => element,
            false => element.aria_value(self.state.read(cx).value().to_string()),
        };
        // every form here refuses its fields empty (AX-109)
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

/// The field over `state` with no node of its own: no role, name or value.
/// It takes Tab; the box drawn around it wears the ring
/// ([`crate::a11y::around_field`]), and a label or a
/// [`crate::a11y::combo_box`] around it speaks for it. Its id is `key`;
/// the door sets its value through `"{key}/field"`.
pub(in crate::shell) fn bare(
    key: &'static str,
    state: &Entity<InputState>,
    size: f32,
    masked: bool,
    cx: &gpui_kit::App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    use gpui_kit::Focusable as _;
    use gpui_kit::component::input::{Input, InputContentType};
    // bare: the canvas's box around it is `ink::field_box`; its text
    // is `size` (the canvas's `15px`, the account name's `22px`)
    // the kit fixes an input's line at `1.25rem` (20px) inside `8px`
    // padding: a larger face is clipped top and bottom. The line follows
    // the face; the canvas's box around it sets height and inset.
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
