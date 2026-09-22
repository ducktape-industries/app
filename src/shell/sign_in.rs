//! The app's own sign-in, between reaching a node and its console: unlock
//! this device's key for the network, mint one and write its recovery
//! phrase down, or read without a key. Reads are open on every network;
//! only a write needs a seated key. No program is named here: the key is
//! the app's, the network's name comes from the node.

use super::*;
use screens::Facts;

impl DesktopWindow {
    /// Password → Unlock, or New key, or Browse without a key.
    pub(super) fn unlock(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let password = self.input(
            "password",
            "Password",
            true,
            "",
            Message::PasswordTyped,
            || Message::UnlockSubmit,
            window,
            cx,
        );
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
                            .child(format!("Sign in to {}", state.network)),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(colors.muted_foreground)
                            .child("Your key signs what you write here. It stays on this device, locked with a password."),
                    )
                    .child(password)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                self.action("unlock", "Unlock", || Message::UnlockSubmit, state.unlock_busy)
                                    .primary(),
                            )
                            .child(
                                self.action(
                                    "create-wallet",
                                    "New key",
                                    || Message::CreateWalletSubmit,
                                    state.unlock_busy,
                                )
                                .ghost(),
                            )
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
                    .child(
                        div()
                            .mt_2()
                            .text_size(px(11.))
                            .text_color(colors.muted_foreground)
                            .child("New key: at least 8 characters; the recovery phrase shows once, next."),
                    )
                    .child(
                        self.action("disconnect", "Switch node", || Message::Disconnect, false)
                            .ghost(),
                    ),
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
                    .text_size(px(13.))
                    .child(format!("{:>2}. {word}", n + 1))
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
