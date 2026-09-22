//! The app's own sign-in, between reaching a node and its console: unlock
//! this device's key for the network, mint one and write its recovery
//! phrase down, restore one from its 24 words, or read without a key. Reads
//! are open on every network; only a write needs a seated key. No program
//! is named here: the key is the app's, the network's name comes from the
//! node.

use super::*;
use screens::Facts;

impl DesktopWindow {
    /// No key on this device for `state.network` → set one up (Create,
    /// Restore, or read without one). A key already here → Unlock (or
    /// Restore a different one, or mint a New one).
    pub(super) fn unlock(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let submit: fn() -> Message = match state.key_exists {
            true => || Message::UnlockSubmit,
            false => || Message::CreateWalletSubmit,
        };
        let password = self.input(
            "password",
            "Password",
            true,
            "",
            Message::PasswordTyped,
            submit,
            None,
            window,
            cx,
        );
        let confirm = (!state.key_exists).then(|| {
            self.input(
                "confirm-password",
                "Confirm password",
                true,
                "",
                Message::ConfirmPasswordTyped,
                submit,
                None,
                window,
                cx,
            )
        });
        let headline = match state.key_exists {
            true => format!("Sign in to {}", state.network),
            false => format!("Set up a key for {}", state.network),
        };
        div()
            .id("sign-in")
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
                            .child(headline),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(colors.muted_foreground)
                            .child("Your key signs what you write here. It stays on this device, locked with a password."),
                    )
                    .child(password)
                    .children(confirm)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                self.action(
                                    match state.key_exists {
                                        true => "unlock",
                                        false => "create-wallet",
                                    },
                                    match state.key_exists {
                                        true => "Unlock",
                                        false => "Create key",
                                    },
                                    submit,
                                    state.unlock_busy,
                                )
                                .primary(),
                            )
                            .child(
                                self.action(
                                    "restore",
                                    "Restore from recovery phrase",
                                    || Message::ShowRestore,
                                    state.unlock_busy,
                                )
                                .ghost(),
                            )
                            .when(state.key_exists, |row| {
                                row.child(
                                    self.action(
                                        "create-wallet",
                                        "New key",
                                        || Message::CreateWalletSubmit,
                                        state.unlock_busy,
                                    )
                                    .ghost(),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                self.action("browse", "Read without a key", || Message::BrowseWithoutKey, false)
                                    .ghost(),
                            ),
                    )
                    .when(!state.unlock_error.is_empty(), |card| {
                        card.child(
                            div()
                                .id("unlock-error")
                                .role(Role::Alert)
                                .aria_label(state.unlock_error.clone())
                                .text_size(px(12.5))
                                .text_color(hsla_of(design::palette(state.dark).danger))
                                .child(state.unlock_error.clone()),
                        )
                    })
                    .when(!state.key_exists, |card| {
                        card.child(
                            div()
                                .mt_2()
                                .text_size(px(11.))
                                .text_color(colors.muted_foreground)
                                .child("At least 8 characters, and the confirmation must match. The recovery phrase shows once, next."),
                        )
                    })
                    .child(
                        self.action("disconnect", "Switch node", || Message::Disconnect, false)
                            .ghost(),
                    ),
            )
            .children(self.toast(cx))
            .into_any_element()
    }

    /// Rebuilding this device's key from its 24 words, under a fresh
    /// password for this copy.
    pub(super) fn restore(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let phrase = self.input(
            "restore-phrase",
            "24 words, separated by spaces",
            false,
            "",
            Message::RestorePhraseTyped,
            || Message::RestoreSubmit,
            Some("Recovery phrase"),
            window,
            cx,
        );
        let password = self.input(
            "restore-password",
            "New password",
            true,
            "",
            Message::RestorePasswordTyped,
            || Message::RestoreSubmit,
            None,
            window,
            cx,
        );
        let confirm = self.input(
            "restore-confirm-password",
            "Confirm password",
            true,
            "",
            Message::RestoreConfirmPasswordTyped,
            || Message::RestoreSubmit,
            None,
            window,
            cx,
        );
        div()
            .id("restore")
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
                            .child(format!("Restore {}'s key", state.network)),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(colors.muted_foreground)
                            .child("The 24 words from when this key was made. They rebuild it; the password only locks this copy."),
                    )
                    .child(phrase)
                    .child(password)
                    .child(confirm)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                self.action("restore-submit", "Restore", || Message::RestoreSubmit, state.unlock_busy)
                                    .primary(),
                            )
                            .child(
                                self.action("restore-back", "Back", || Message::RestoreCancel, false)
                                    .ghost(),
                            ),
                    )
                    .when(!state.unlock_error.is_empty(), |card| {
                        card.child(
                            div()
                                .id("unlock-error")
                                .role(Role::Alert)
                                .aria_label(state.unlock_error.clone())
                                .text_size(px(12.5))
                                .text_color(hsla_of(design::palette(state.dark).danger))
                                .child(state.unlock_error.clone()),
                        )
                    }),
            )
            .children(self.toast(cx))
            .into_any_element()
    }

    /// The freshly minted key's recovery phrase, once. Private to the test
    /// door; assistive technology reads it as anyone at the screen would.
    pub(super) fn phrase(&mut self, state: &Facts, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let words = state
            .phrase
            .split_whitespace()
            .enumerate()
            .map(|(n, word)| {
                div()
                    .w(px(140.))
                    .flex()
                    .text_size(px(13.))
                    .child(
                        div()
                            .w(px(22.))
                            .flex_shrink_0()
                            .text_color(colors.muted_foreground)
                            .child(format!("{}.", n + 1)),
                    )
                    .child(div().child(word.to_string()))
            });
        let sheet = crate::a11y::private(
            div()
                .id("phrase")
                .role(Role::Group)
                .aria_label(state.phrase.clone())
                .flex()
                .flex_wrap()
                .gap_1()
                .p_3()
                .rounded(px(design::radius::CONTROL as f32))
                .bg(colors.muted)
                .children(words),
        );
        div()
            .id("recovery")
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
                            .child("Write down your recovery phrase"),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(colors.muted_foreground)
                            .child("These words rebuild the key on another device. They show once; the password only unlocks this copy."),
                    )
                    .child(sheet)
                    .child(
                        self.action("phrase-done", "I wrote it down", || Message::PhraseWrittenDown, false)
                            .primary(),
                    ),
            )
            .into_any_element()
    }
}
