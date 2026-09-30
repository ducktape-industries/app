//! The launcher's key screen: this device's key opening, locked, failed, or
//! behind a password from before. See launcher.rs for key vs account.

use super::super::super::entities::{Account, Session};
use super::super::super::ink::{self, *};
use super::super::fields::TextField;
use super::{LauncherLayer, LauncherScreen, buttons, closing};
use gpui_kit::*;

impl LauncherLayer {
    /// SignIn: this device's key, opening, locked, failed, or (from before
    /// keys moved into the system) behind a password asked once.
    pub(super) fn unlock(&self, window: &mut Window, cx: &App) -> AnyElement {
        let (session, account) = (self.session.read(cx).get(), self.account.read(cx).get());
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let network = &session.network;
        let failed = !account.error.is_empty();
        let (label, headline, lead) = match (account.key_exists, account.locked, failed) {
            (true, _, _) => (
                format!("{network} · locked"),
                format!("Sign in to {network}"),
                Some(
                    "This device's key still has a password from before. Type it once; the system keeps the key after that.",
                ),
            ),
            (false, true, _) => (
                format!("{network} · locked"),
                format!("Sign in to {network}"),
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
        let other_chain = session.other_chain.then(|| {
            ink::note(
                "words",
                format!(
                    "This is a different network also called {network}. It gets its own key on this device."
                ),
                ink.muted,
            )
            .id("other-chain")
            .role(Role::Note)
            .into_any_element()
        });
        let password = account.key_exists.then(|| {
            let input = self.fields.password.input(
                "password",
                TextField {
                    label: "Password".into(),
                    private: false,
                    error: failed.then(|| account.error.clone()),
                    size: 15.,
                },
                cx,
            );
            let focused = self.fields.password.focused(window, cx);
            let border = match failed {
                true => ink.danger,
                false => ink.field,
            };
            field(
                "password-label",
                "Password for this device's key",
                field_box(input, focused, border, 44., &ink).into_any_element(),
                failed.then(|| alert("unlock-error", account.error.clone(), &ink)),
                &ink,
            )
            .into_any_element()
        });
        let busy = account.busy || account.seating;
        let primary = match (account.key_exists || account.locked, failed) {
            (true, _) => Some("Unlock"),
            (false, true) => Some("Try again"),
            (false, false) => None,
        }
        .map(|text| {
            buttons([self.button(
                "unlock",
                text,
                Kind::Primary,
                self.with_field(&self.fields.password, Account::unlock),
                busy,
                &ink,
            )])
            .into_any_element()
        });
        let loose_error = (failed && !account.key_exists)
            .then(|| alert("unlock-error", account.error.clone(), &ink));
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
                self.on_account(Account::browse_without_key),
                false,
                &ink,
            )],
            &ink,
        );
        self.frame(
            LauncherScreen {
                id: "sign-in",
                tight: false,
                caption: self.node_caption(cx),
                back: Some((
                    "disconnect",
                    "Other networks",
                    Box::new(self.on_session(Session::disconnect)),
                )),
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
