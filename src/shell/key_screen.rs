//! The launcher's key screen: this device's key opening, locked, failed, or
//! behind a password from before. See launcher.rs for key vs account.

use super::ink::{self, *};
use super::launcher::LauncherScreen;
use super::launcher::{buttons, closing, node_caption};
use super::layers::TextField;
use super::*;
use facts::Facts;
use figure::Figure;

impl Screens {
    /// SignIn: this device's key, opening, locked, failed, or (from before
    /// keys moved into the system) behind a password asked once.
    pub(super) fn unlock(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let failed = !state.unlock_error.is_empty();
        let (label, headline, lead) = match (state.key_exists, state.locked, failed) {
            (true, _, _) => (
                format!("{} · locked", state.network),
                format!("Sign in to {}", state.network),
                Some(
                    "This device's key still has a password from before. Type it once; the system keeps the key after that.",
                ),
            ),
            (false, true, _) => (
                format!("{} · locked", state.network),
                format!("Sign in to {}", state.network),
                None,
            ),
            (false, false, true) => (
                "[02 / 03] Key".to_string(),
                "The key didn't open".to_string(),
                Some("The system holds this device's key and didn't hand it over."),
            ),
            (false, false, false) => (
                "[02 / 03] Key".to_string(),
                "Opening this device's key…".to_string(),
                Some(
                    "It signs what you write, and the system keeps it: no password, nothing to write down.",
                ),
            ),
        };
        let other_chain = state.other_chain.then(|| {
            ink::note(
                "words",
                format!(
                    "This is a different network also called {}. It gets its own key on this device.",
                    state.network
                ),
                ink.muted,
            )
            .id("other-chain")
            .role(Role::Note)
            .into_any_element()
        });
        let password = state.key_exists.then(|| {
            let field = self.input(
                TextField {
                    key: "password",
                    placeholder: "",
                    masked: true,
                    value: |state| match &state.stage {
                        crate::Stage::Unlock(step) => step.password.as_str(),
                        _ => "",
                    },
                    on_change: Message::PasswordTyped,
                    on_enter: || Message::UnlockSubmit,
                    label: Some("Password".into()),
                    private: false,
                    error: failed.then(|| state.unlock_error.clone()),
                    size: 15.,
                },
                window,
                cx,
            );
            let focused = self.field_focused("password", window, cx);
            let border = match failed {
                true => ink.danger,
                false => ink.field,
            };
            self.field(
                "password-label",
                "Password for this device's key",
                field_box(field, focused, border, 44., &ink).into_any_element(),
                failed.then(|| self.alert("unlock-error", state.unlock_error.clone(), &ink)),
                &ink,
            )
            .into_any_element()
        });
        let busy = state.unlock_busy || state.seating;
        let primary = match (state.key_exists || state.locked, failed) {
            (true, _) => Some("Unlock"),
            (false, true) => Some("Try again"),
            (false, false) => None,
        }
        .map(|text| {
            buttons([self.button(
                "unlock",
                text,
                Kind::Primary,
                || Message::UnlockSubmit,
                busy,
                &ink,
            )])
            .into_any_element()
        });
        let loose_error = (failed && !state.key_exists)
            .then(|| self.alert("unlock-error", state.unlock_error.clone(), &ink));
        let form = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .children(other_chain)
            .children(password)
            .children(loose_error)
            .children(primary);
        let links = closing(
            [self.link(
                "browse",
                "Read without a key",
                || Message::BrowseWithoutKey,
                false,
                &ink,
            )],
            &ink,
        );
        self.launcher(
            LauncherScreen {
                id: "sign-in",
                tight: false,
                figure: Figure::Ring,
                caption: node_caption(state),
                back: Some(("disconnect", "Other networks", || Message::Disconnect)),
                label,
                headline,
                lead: lead.map(str::to_owned),
                body: vec![form.into_any_element(), links.into_any_element()],
            },
            window,
            cx,
        )
    }
}
