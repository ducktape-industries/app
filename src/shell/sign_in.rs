//! The launcher's key and account screens, in that order: this device's
//! key (unlock it, make one and write its 24 words down, restore one from
//! them, or read without one), then the network's account for that key
//! (create one, or add this device to one a passkey holds). Reads are open
//! on every network; only a write needs a seated key. No program is named
//! here: the key is the app's, the network's name comes from the node.

use super::*;
use figure::Figure;
use launcher::{mono, square};
use screens::Facts;

impl DesktopWindow {
    /// The node reached, as the drawing's caption: "testkit · 127.0.0.1:8844".
    fn where_(state: &Facts) -> String {
        let rpc = &state.connected_rpc;
        let host = rpc.split_once("://").map_or(rpc.as_str(), |(_, host)| host);
        format!("{} · {host}", state.network)
    }

    /// This device's key for `state.network`. None here yet → make one
    /// (or restore it, or read without one). One here → unlock it (or
    /// restore a different one, or replace it with a new one, which first
    /// says what that costs). Nothing about accounts: that is the next
    /// step, once a key is unlocked.
    pub(super) fn unlock(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
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
        let (label, headline, lead) = match (state.key_exists, state.replacing) {
            (true, true) => (
                "[02 / 04] Key · replace".to_string(),
                "Replace this device's key".to_string(),
                format!(
                    "A new key signs what you write on {}. It stays on this device, locked with a password.",
                    state.network
                ),
            ),
            (true, false) => (
                format!("{} · locked", state.network),
                format!("Sign in to {}", state.network),
                "Unlock this device's key with its password. The key signs what you write here."
                    .to_string(),
            ),
            (false, _) => (
                "[02 / 04] Key".to_string(),
                format!("Set up a key for {}", state.network),
                format!(
                    "It signs what you write on {} and never leaves this device. A password you choose now keeps it locked.",
                    state.network
                ),
            ),
        };
        // The error replaces the standing hint rather than repeating it.
        let below = match (state.unlock_error.is_empty(), minting) {
            (false, _) => Some(self.alert("unlock-error", state.unlock_error.clone(), cx)),
            (true, true) => Some(
                div()
                    .id("password-hint")
                    .text_size(px(12.5))
                    .text_color(colors.muted_foreground)
                    .child(format!(
                        "At least {} characters, typed twice. The recovery phrase shows next.",
                        keystore::userkey::MIN_PASSWORD_LEN
                    )),
            ),
            (true, false) => None,
        };
        // Two chains can share a name; this one's keys are its own
        // (`backend::bind_keyring`), and the person hears why it asks anew.
        let other_chain = state.other_chain.then(|| {
            div()
                .id("other-chain")
                .control(
                    Role::Note,
                    format!("This is a different network also called {}", state.network),
                )
                .text_size(px(13.))
                .child(format!(
                    "This is a different network also called {}. It gets its own key on this device; the other {}'s key stays with that one.",
                    state.network, state.network
                ))
        });
        let warning = state.replacing.then(|| {
            div()
                .id("new-key-warning")
                .role(Role::Note)
                .aria_label("The current key is replaced")
                .p_3()
                .border_1()
                .border_color(hsla_of(design::palette(state.dark).danger))
                .text_size(px(13.))
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
        .primary();
        let links = match state.replacing {
            true => vec![self.quiet_link(
                "new-key-cancel",
                "Cancel, keep the current key",
                || Message::NewKeyCancel,
                cx,
            )],
            false => {
                let mut links = vec![self.quiet_link(
                    "restore",
                    "Restore from recovery phrase",
                    || Message::ShowRestore,
                    cx,
                )];
                if state.key_exists {
                    links.push(self.quiet_link(
                        "new-key",
                        "Replace this key",
                        || Message::ShowNewKey,
                        cx,
                    ));
                }
                links.push(self.quiet_link(
                    "browse",
                    "Read without a key",
                    || Message::BrowseWithoutKey,
                    cx,
                ));
                links
            }
        };
        let form = div()
            .flex()
            .flex_col()
            .gap_3()
            .children(other_chain)
            .children(warning)
            .child(self.field("Password", password))
            .children(confirm.map(|confirm| self.field("Confirm password", confirm)))
            .children(below)
            .child(div().flex().child(square(primary)));
        let body = vec![
            form.into_any_element(),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(links)
                .into_any_element(),
        ];
        self.launcher(
            "sign-in",
            Figure::Ring,
            Self::where_(state),
            Some(("disconnect", "← Other networks", || Message::Disconnect)),
            label,
            headline,
            Some(lead),
            body,
            window,
            cx,
        )
    }

    /// A passkey ceremony is in the browser, or on a phone through the QR:
    /// say so, and offer to stop.
    fn passkey_waiting(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let (title, hint) = match state.passkey_qr {
            Some(_) => (
                "Scan with your phone",
                "Point its camera at the code and follow it. It asks for your passkey twice; a new code shows here for the second time.",
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
                .items_start()
                .gap_2()
                .child(
                    div()
                        .id("passkey-qr")
                        .role(Role::Image)
                        .aria_label("Passkey QR code")
                        .child(crate::render::qr(&view_wire::Qr {
                            payload: Some(url.clone().into_bytes()),
                            size: Some(view_wire::QrSize::Total(200.)),
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
                    square(
                        gpui_kit::component::button::Button::new("passkey-qr-copy")
                            .label("Copy link")
                            .outline()
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                            }),
                    )
                })
                .into_any_element(),
            None => self
                .quiet_link(
                    "passkey-use-phone",
                    "Use a phone instead",
                    || Message::PasskeyUsePhone,
                    cx,
                )
                .into_any_element(),
        };
        let status = div()
            .id("passkey-waiting-status")
            .role(Role::Status)
            .aria_label(title)
            .text_size(px(13.))
            .text_color(colors.muted_foreground)
            .child(hint);
        let body = vec![
            status.into_any_element(),
            phone,
            div()
                .flex()
                .child(square(
                    self.action("passkey-cancel", "Cancel", || Message::PasskeyCancel, false)
                        .outline(),
                ))
                .into_any_element(),
        ];
        self.launcher(
            "passkey-waiting",
            Figure::Pair,
            Self::where_(state),
            None,
            "[04 / 04] Account · passkey".into(),
            title.into(),
            None,
            body,
            window,
            cx,
        )
    }

    /// The account step: the unlocked key holds no account on this
    /// network. Create one (identity's self-serve `Create`, signed by the
    /// key — optionally with a passkey joining it), or add this device's
    /// key to an account that already exists, a passkey on it consenting
    /// (`AddKey`). "Not now" goes to the desk; reading and signing work
    /// there without an account, and the menu bar's "Create account" comes
    /// back here.
    pub(super) fn account_step(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        if state.passkey_waiting {
            return self.passkey_waiting(state, window, cx);
        }
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let name = self.input(
            "create-account-name",
            "Your name",
            false,
            |state| &state.account_name,
            Message::AccountNameTyped,
            || Message::CreateAccountSubmit,
            Some("Account name".into()),
            false,
            window,
            cx,
        );
        let label = match state.unlock_busy {
            true => "Creating…",
            false => "Create account",
        };
        let create = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.field("Name", name))
            .children(
                (!state.unlock_error.is_empty())
                    .then(|| self.alert("create-account-error", state.unlock_error.clone(), cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(square(
                        self.action(
                            "create-account",
                            label,
                            || Message::CreateAccountSubmit,
                            state.unlock_busy,
                        )
                        .primary(),
                    ))
                    .child(square(
                        self.action(
                            "passkey-create",
                            "Create with a passkey",
                            || Message::PasskeyCreateSubmit,
                            state.unlock_busy,
                        )
                        .outline(),
                    )),
            );
        // An account made elsewhere: this device's key joins it. The
        // account's passkey is what says yes.
        let join = div()
            .id("join-account")
            .role(Role::Group)
            .aria_label("Already have an account")
            .pt_4()
            .border_t_1()
            .border_color(colors.border)
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(format!("Already have an account on {}?", state.network)))
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(colors.muted_foreground)
                    .child("Add this device to it. The account's passkey says yes, in your browser or on your phone; this device's key then signs for the account too."),
            )
            .child(div().flex().child(square(
                self.action(
                    "passkey-sign-in",
                    "Add this device with a passkey",
                    || Message::PasskeySignInSubmit,
                    state.unlock_busy,
                )
                .outline(),
            )));
        let body = vec![
            create.into_any_element(),
            join.into_any_element(),
            div()
                .child(self.quiet_link(
                    "create-account-later",
                    "Not now",
                    || Message::CreateAccountLater,
                    cx,
                ))
                .into_any_element(),
        ];
        self.launcher(
            "account-step",
            Figure::Pair,
            Self::where_(state),
            None,
            "[04 / 04] Account".into(),
            "What should people call you?".into(),
            Some(format!("An account is the name beside everything you write on {}. This device's key signs for it.", state.network)),
            body,
            window,
            cx,
        )
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
        let form = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.field("Recovery phrase", phrase))
            .child(self.field("New password", password))
            .child(self.field("Confirm password", confirm))
            .children(
                (!state.unlock_error.is_empty())
                    .then(|| self.alert("unlock-error", state.unlock_error.clone(), cx)),
            )
            .child(
                div().flex().child(square(
                    self.action(
                        "restore-submit",
                        "Restore",
                        || Message::RestoreSubmit,
                        state.unlock_busy,
                    )
                    .primary(),
                )),
            );
        self.launcher(
            "restore",
            Figure::Sheets,
            Self::where_(state),
            Some(("restore-back", "← Back", || Message::RestoreCancel)),
            "[02 / 04] Key · restore".into(),
            format!("Restore {}'s key", state.network),
            Some("The 24 words from when this key was made. They rebuild it; the password only locks this copy.".into()),
            vec![form.into_any_element()],
            window,
            cx,
        )
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
        let words: Vec<&str> = state.phrase.split_whitespace().collect();
        let per_column = words.len().div_ceil(3).max(1);
        // Three columns read top to bottom, 1–8, 9–16, 17–24.
        let columns = words.chunks(per_column).enumerate().map(|(column, chunk)| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .children(chunk.iter().enumerate().map(|(row, word)| {
                    div()
                        .flex()
                        .items_baseline()
                        .gap_2()
                        .py(px(5.))
                        .border_b_1()
                        .border_color(colors.border)
                        .text_size(px(14.))
                        .child(
                            mono(
                                format!("{}", column * per_column + row + 1),
                                colors.muted_foreground,
                            )
                            .w(px(20.)),
                        )
                        .child(div().child(word.to_string()))
                }))
        });
        let sheet = crate::a11y::private(
            div()
                .id("phrase")
                .role(Role::Group)
                .aria_label(state.phrase.clone())
                .flex()
                .gap_5()
                .children(columns),
        );
        let done = div()
            .flex()
            .items_center()
            .gap_4()
            .child(square(
                self.action(
                    "phrase-done",
                    "I wrote them down",
                    || Message::PhraseWrittenDown,
                    false,
                )
                .primary(),
            ))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.muted_foreground)
                    .child("Nobody can recover them for you."),
            );
        self.launcher(
            "recovery",
            Figure::Sheets,
            "Twenty-four words, on paper.".into(),
            None,
            "[03 / 04] Phrase".into(),
            "Write these down".into(),
            Some("In order, on paper. They are the only way back to this key on another device, and anyone holding them can write as you. They show only now.".into()),
            vec![sheet.into_any_element(), done.into_any_element()],
            window,
            cx,
        )
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
                    .gap_3()
                    .child(
                        mono(format!("Word {}", nth + 1), colors.muted_foreground)
                            .w(px(64.))
                            .flex_shrink_0(),
                    )
                    .child(div().flex_1().child(field))
                    .into_any_element()
            })
            .collect();
        let prompt = div()
            .id("phrase-check-prompt")
            .role(Role::Label)
            .aria_label(prompt.clone())
            .text_size(px(13.))
            .text_color(colors.muted_foreground)
            .child(prompt);
        let form = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(prompt)
            .children(rows)
            .children(
                (!state.unlock_error.is_empty())
                    .then(|| self.alert("unlock-error", state.unlock_error.clone(), cx)),
            )
            .child(
                div().flex().child(square(
                    self.action(
                        "phrase-check",
                        "Confirm",
                        || Message::PhraseCheckSubmit,
                        false,
                    )
                    .primary(),
                )),
            );
        self.launcher(
            "recovery-check",
            Figure::Sheets,
            "Twenty-four words, on paper.".into(),
            Some(("phrase-show", "← Show the phrase again", || {
                Message::PhraseShowAgain
            })),
            "[03 / 04] Phrase · check".into(),
            "Now, three of them".into(),
            None,
            vec![form.into_any_element()],
            window,
            cx,
        )
    }
}
