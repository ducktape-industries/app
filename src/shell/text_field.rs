//! The native text field every screen types into: the model owns the
//! text, the field mirrors it (`DesktopWindow::input`), and its state
//! lives as long as its window (`NativeInput`).

use super::*;

/// One native text field's state, kept for as long as its window lives
/// (see `DesktopWindow::input`).
pub(super) struct NativeInput {
    pub(super) state: Entity<gpui_kit::component::input::InputState>,
    /// A digest of the model text the field last agreed with — what it
    /// sent on its last change, or what the model last pushed into it.
    mirrored: std::rc::Rc<std::cell::Cell<u64>>,
    _subscription: gpui_kit::Subscription,
}

/// A field's text reduced to what `input` compares, so a mirrored password
/// is not kept a second time in the clear.
fn digest(text: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::hash::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// What a screen asks of its text field (`DesktopWindow::input`).
pub(super) struct TextField {
    /// Its element id, and what its state is kept under in the window.
    pub(super) key: &'static str,
    pub(super) placeholder: &'static str,
    /// Its accessible name; the placeholder when `None`.
    pub(super) label: Option<gpui_kit::SharedString>,
    /// Drawn as dots, with the password role.
    pub(super) masked: bool,
    /// Sensitive without being masked (the recovery phrase): its value is
    /// read aloud, and the test door masks it (`a11y::AX_PRIVATE`).
    pub(super) private: bool,
    /// The text size, in the canvas's px (`ink::fit` scales it).
    pub(super) size: f32,
    /// The model's copy of the text, which the field mirrors.
    pub(super) value: fn(&Ducktape) -> &str,
    /// What every change dispatches, with the text.
    pub(super) on_change: fn(String) -> Message,
    /// What Enter dispatches.
    pub(super) on_enter: fn() -> Message,
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
    /// accessible value is the field's current text — unless `masked`: a
    /// `PasswordInput` shows no text, so it gives none (the door's mask is
    /// not the platform's; a value set here reaches every AX client).
    /// `private` marks a field (the recovery phrase) that is sensitive
    /// without being visually masked [`crate::a11y::AX_PRIVATE`]: assistive
    /// technology reads the text on the screen; the test door masks it.
    pub(super) fn input(
        &mut self,
        field: TextField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::StatefulInteractiveElement as _;
        let (label, placeholder) = (field.label.clone(), field.placeholder);
        let (masked, private) = (field.masked, field.private);
        let (state, field) = self.bare_input(field, window, cx);
        let field = field.aria_label(label.unwrap_or_else(|| placeholder.into()));
        let field = match masked {
            true => field,
            false => field.aria_value(state.read(cx).value().to_string()),
        };
        let field = match private {
            true => crate::a11y::private(field),
            false => field,
        };
        match masked {
            true => field.role(gpui_kit::Role::PasswordInput),
            false => field.role(gpui_kit::Role::TextInput),
        }
        .into_any_element()
    }

    /// [`Self::input`]'s field with no node of its own, and its state: no
    /// role, name or value. It takes Tab and wears the ring; a
    /// [`crate::a11y::combo_box`] around it and its list speaks for it.
    pub(super) fn bare_input(
        &mut self,
        field: TextField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (
        Entity<gpui_kit::component::input::InputState>,
        gpui_kit::Stateful<gpui_kit::Div>,
    ) {
        let TextField {
            key,
            placeholder,
            masked,
            size,
            value,
            on_change,
            on_enter,
            ..
        } = field;
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
        use gpui_kit::Focusable as _;
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
            .text_size(gpui_kit::px(super::ink::fit(size)))
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
        );
        (state.clone(), field)
    }
}
