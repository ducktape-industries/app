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
}
