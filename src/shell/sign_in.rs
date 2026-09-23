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
    /// Restore a different one, or replace it with a New one, which first
    /// says what that costs).
    ///
    /// One primary action, full width under the fields; everything else is
    /// a quiet row beneath, so the column never grows past its 460px.
    pub(super) fn unlock(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::Sizable as _;
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        if state.passkey_waiting {
            return self.passkey_waiting(state, cx);
        }
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let danger = hsla_of(design::palette(state.dark).danger);
        // Minting a key: the first one, or a new one in place of this one.
        let minting = !state.key_exists || state.replacing;
        let submit: fn() -> Message = match minting {
            false => || Message::UnlockSubmit,
            true => || Message::CreateWalletSubmit,
        };
        let password = self.input(
            "password",
            // One hint per field key: the field outlives the screen that
            // made it, and a placeholder is set only once.
            "Password",
            true,
            |state| &state.password,
            Message::PasswordTyped,
            submit,
            Some("Password".into()),
            true,
            window,
            cx,
        );
        let confirm = minting.then(|| {
            self.input(
                "confirm-password",
                "Confirm password",
                true,
                |state| &state.confirm_password,
                Message::ConfirmPasswordTyped,
                submit,
                None,
                true,
                window,
                cx,
            )
        });
        let (headline, lead) = match (state.key_exists, state.replacing) {
            (true, true) => (
                "Replace this device's key".to_string(),
                "Your key signs what you write here. It stays on this device, locked with a password.",
            ),
            (true, false) => (
                format!("Sign in to {}", state.network),
                "Unlock this device's key with its password. The key signs what you write here.",
            ),
            (false, _) => (
                format!("Set up a key for {}", state.network),
                "Your key signs what you write here. It stays on this device, locked with a password you choose now.",
            ),
        };
        // The error replaces the standing hint rather than repeating it.
        let below = match (state.unlock_error.is_empty(), minting) {
            (false, _) => Some(
                div()
                    .id("unlock-error")
                    .role(Role::Alert)
                    .aria_label(state.unlock_error.clone())
                    .text_size(px(12.5))
                    .text_color(danger)
                    .child(state.unlock_error.clone()),
            ),
            (true, true) => Some(
                div()
                    .id("password-hint")
                    .text_size(px(11.))
                    .text_color(colors.muted_foreground)
                    .child(format!(
                        "At least {} characters, typed twice. The recovery phrase shows next.",
                        keystore::userkey::MIN_PASSWORD_LEN
                    )),
            ),
            (true, false) => None,
        };
        let warning = state.replacing.then(|| {
            div()
                .id("new-key-warning")
                .role(Role::Note)
                .aria_label("The current key is replaced")
                .p_3()
                .rounded(px(design::radius::CONTROL as f32))
                .border_1()
                .border_color(danger)
                .text_size(px(12.5))
                .child(format!(
                    "A new key takes the current one's place on {}. The current key is gone from this app unless its recovery phrase is written down: only those 24 words bring it back.",
                    state.network
                ))
        });
        let primary = match (state.key_exists, state.replacing) {
            (true, false) => self.action("unlock", "Unlock", submit, state.unlock_busy),
            (true, true) => self.action(
                "create-wallet",
                "Replace with a new key",
                submit,
                state.unlock_busy,
            ),
            (false, _) => self.action("create-wallet", "Create key", submit, state.unlock_busy),
        }
        .primary()
        .w_full();
        let quiet =
            |this: &Self,
             key: &'static str,
             label: &'static str,
             message: fn() -> Message,
             busy: bool| { this.action(key, label, message, busy).ghost().small() };
        let others = match state.replacing {
            true => div().child(quiet(
                self,
                "new-key-cancel",
                "Cancel, keep the current key",
                || Message::NewKeyCancel,
                state.unlock_busy,
            )),
            false => div()
                .flex()
                .flex_wrap()
                .gap_1()
                .child(quiet(
                    self,
                    "restore",
                    "Restore from recovery phrase",
                    || Message::ShowRestore,
                    state.unlock_busy,
                ))
                .when(state.key_exists, |row| {
                    row.child(quiet(
                        self,
                        "new-key",
                        "New key",
                        || Message::ShowNewKey,
                        state.unlock_busy,
                    ))
                })
                .child(quiet(
                    self,
                    "browse",
                    "Read without a key",
                    || Message::BrowseWithoutKey,
                    false,
                ))
                .child(quiet(
                    self,
                    "disconnect",
                    "Switch node",
                    || Message::Disconnect,
                    false,
                )),
        };
        let passkey = (!state.replacing).then(|| self.passkey_offer(state, window, cx));
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
                            .child(lead),
                    )
                    .children(warning)
                    .child(password)
                    .children(confirm)
                    .child(primary)
                    .children(below)
                    .child(others)
                    .children(passkey),
            )
            .children(self.toast(cx))
            .into_any_element()
    }

    /// The passkey path, set apart from the password one. Either way this
    /// device's key signs the writes, so the password above is still
    /// needed; the copy says which path asks for what. With a key already
    /// here the one thing to do is sign in to an existing passkey account;
    /// creating an account belongs to first setup, not beside Unlock.
    fn passkey_offer(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let account_name = (!state.key_exists).then(|| {
            self.input(
                "account-name",
                "Account name",
                false,
                |state| &state.account_name,
                Message::AccountNameTyped,
                || Message::PasskeyCreateSubmit,
                Some("Account name".into()),
                false,
                window,
                cx,
            )
        });
        let copy = match state.key_exists {
            true => {
                "Have an account on a passkey? Type this key's password above, then sign in; your browser asks for the passkey."
            }
            false => {
                "Or keep your account on a passkey. Choose the password above first: it locks the key on this device, and the passkey (your browser asks for it) signs in to the account."
            }
        };
        div()
            .id("passkey")
            .role(Role::Group)
            .aria_label("Passkey")
            .mt_2()
            .pt_3()
            .border_t_1()
            .border_color(colors.border)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(colors.muted_foreground)
                    .child(copy),
            )
            .children(account_name)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .when(!state.key_exists, |row| {
                        row.child(
                            self.action(
                                "passkey-create",
                                "Create account with a passkey",
                                || Message::PasskeyCreateSubmit,
                                state.unlock_busy,
                            )
                            .outline(),
                        )
                    })
                    .child(
                        self.action(
                            "passkey-sign-in",
                            "Sign in with a passkey",
                            || Message::PasskeySignInSubmit,
                            state.unlock_busy,
                        )
                        .outline(),
                    ),
            )
            .into_any_element()
    }

    /// A passkey ceremony is in the browser, or on a phone through the QR:
    /// say so, and offer to stop.
    fn passkey_waiting(&mut self, state: &Facts, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let (title, hint) = match state.passkey_qr {
            Some(_) => (
                "Scan with your phone",
                "Open your phone's camera on the code and follow it. It asks for your passkey twice; a new code shows here for the second time.",
            ),
            None => (
                "Continue in your browser…",
                "Your browser asks for your passkey twice. Come back here once it says you're done.",
            ),
        };
        let phone = match &state.passkey_qr {
            Some(url) => div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("passkey-qr")
                        .role(Role::Image)
                        .aria_label("Passkey QR code")
                        .child(crate::render::qr(&view_wire::Qr {
                            payload: Some(url.clone().into_bytes()),
                            size: Some(view_wire::QrSize::Total(240.)),
                            ..Default::default()
                        })),
                )
                .child(crate::a11y::whole(
                    div()
                        .id("passkey-qr-url")
                        .role(Role::Label)
                        .aria_label(url.clone())
                        .w_full()
                        .text_size(px(11.))
                        .text_color(colors.muted_foreground)
                        .child(url.clone()),
                ))
                .child({
                    let url = url.clone();
                    gpui_kit::component::button::Button::new("passkey-qr-copy")
                        .label("Copy link")
                        .outline()
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                        })
                })
                .into_any_element(),
            None => self
                .action(
                    "passkey-use-phone",
                    "Use a phone instead",
                    || Message::PasskeyUsePhone,
                    false,
                )
                .ghost()
                .into_any_element(),
        };
        div()
            .id("passkey-waiting")
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
                            .id("passkey-waiting-status")
                            .role(Role::Status)
                            .aria_label(title)
                            .text_size(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(colors.muted_foreground)
                            .child(hint),
                    )
                    .child(phone)
                    .child(self.action(
                        "passkey-cancel",
                        "Cancel",
                        || Message::PasskeyCancel,
                        false,
                    )),
            )
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
            |state| &state.restore_phrase,
            Message::RestorePhraseTyped,
            || Message::RestoreSubmit,
            Some("Recovery phrase".into()),
            true,
            window,
            cx,
        );
        let password = self.input(
            "restore-password",
            "New password",
            true,
            |state| &state.restore_password,
            Message::RestorePasswordTyped,
            || Message::RestoreSubmit,
            None,
            true,
            window,
            cx,
        );
        let confirm = self.input(
            "restore-confirm-password",
            "Confirm password",
            true,
            |state| &state.restore_confirm_password,
            Message::RestoreConfirmPasswordTyped,
            || Message::RestoreSubmit,
            None,
            true,
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

    /// The freshly minted key's recovery phrase, then a check that it was
    /// written down: three of its words typed back. Private to the test
    /// door; assistive technology reads it as anyone at the screen would.
    pub(super) fn phrase(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        if let Some(asked) = state.phrase_quiz {
            return self.phrase_check(state, asked, window, cx);
        }
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let words = state
            .phrase
            .split_whitespace()
            .enumerate()
            .map(|(n, word)| {
                div()
                    .w(px(136.))
                    .flex()
                    .gap_1()
                    .text_size(px(13.))
                    .child(
                        // "24." must clear the word beside it.
                        div()
                            .w(px(26.))
                            .flex_shrink_0()
                            .text_right()
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
                            .child("These 24 words rebuild the key on another device, and nothing else can. They show only during this setup; the password only unlocks this copy."),
                    )
                    .child(sheet)
                    .child(
                        self.action("phrase-done", "I wrote it down", || Message::PhraseWrittenDown, false)
                            .primary()
                            .w_full(),
                    ),
            )
            .into_any_element()
    }

    /// "Words 5, 12 and 20": proof the phrase left the screen before the
    /// screen lets it go. The typed words stay out of the AX value like
    /// the phrase itself.
    fn phrase_check(
        &mut self,
        state: &Facts,
        asked: [usize; 3],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let [a, b, c] = asked.map(|nth| nth + 1);
        let prompt = format!("Type words {a}, {b} and {c} from your phrase.");
        type Field = (&'static str, fn(&Ducktape) -> &str, fn(String) -> Message);
        let fields: [Field; 3] = [
            (
                "phrase-word-1",
                |state| &state.quiz_answers[0],
                |text| Message::PhraseWordTyped(0, text),
            ),
            (
                "phrase-word-2",
                |state| &state.quiz_answers[1],
                |text| Message::PhraseWordTyped(1, text),
            ),
            (
                "phrase-word-3",
                |state| &state.quiz_answers[2],
                |text| Message::PhraseWordTyped(2, text),
            ),
        ];
        let rows: Vec<_> = fields
            .into_iter()
            .zip(asked)
            .map(|((key, value, typed), nth)| {
                let field = self.input(
                    key,
                    "Word",
                    false,
                    value,
                    typed,
                    || Message::PhraseCheckSubmit,
                    Some(format!("Word {}", nth + 1).into()),
                    true,
                    window,
                    cx,
                );
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(64.))
                            .flex_shrink_0()
                            .text_size(px(12.5))
                            .text_color(colors.muted_foreground)
                            .child(format!("Word {}", nth + 1)),
                    )
                    .child(div().flex_1().child(field))
            })
            .collect();
        div()
            .id("recovery-check")
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
                            .child("Check your recovery phrase"),
                    )
                    .child(
                        div()
                            .id("phrase-check-prompt")
                            .role(Role::Label)
                            .aria_label(prompt.clone())
                            .text_size(px(12.5))
                            .text_color(colors.muted_foreground)
                            .child(prompt),
                    )
                    .children(rows)
                    .child(
                        self.action(
                            "phrase-check",
                            "Confirm",
                            || Message::PhraseCheckSubmit,
                            false,
                        )
                        .primary()
                        .w_full(),
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
                        self.action(
                            "phrase-show",
                            "Show phrase again",
                            || Message::PhraseShowAgain,
                            false,
                        )
                        .ghost(),
                    ),
            )
            .into_any_element()
    }
}
