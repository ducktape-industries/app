//! The launcher's key and account screens, in that order: this device's
//! key (kept by the system and opened on its own; the screen shows only
//! while it opens or when it is locked), then the network's account for
//! that key (create one, or add this device to one: another device of it,
//! its passkey or its recovery key says yes). Reads are open
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

    /// This device's key for `state.network`, kept by the system and opened
    /// on its own: this screen shows only while it opens, after a Lock, when
    /// the system would not hand it over, or for a password-locked key from
    /// before (asked once, then kept by the system). Nothing about accounts:
    /// that is the next step.
    pub(super) fn unlock(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let failed = !state.unlock_error.is_empty();
        let (label, headline, lead) = match (state.key_exists, state.locked, failed) {
            (true, _, _) => (
                format!("{} · locked", state.network),
                format!("Sign in to {}", state.network),
                "This device's key still has a password from before. Type it once; the system keeps the key after that.",
            ),
            (false, true, _) => (
                format!("{} · locked", state.network),
                format!("Sign in to {}", state.network),
                "You locked this device's key. It stays with the system; unlock to write again.",
            ),
            (false, false, true) => (
                "[02 / 04] Key".to_string(),
                "The key didn't open".to_string(),
                "The system holds this device's key and didn't hand it over.",
            ),
            (false, false, false) => (
                "[02 / 04] Key".to_string(),
                "Opening this device's key…".to_string(),
                "It signs what you write, and the system keeps it: no password, nothing to write down.",
            ),
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
        let password = state.key_exists.then(|| {
            let field = self.input(
                "password",
                "Password",
                true,
                |state| &state.password,
                Message::PasswordTyped,
                || Message::UnlockSubmit,
                Some("Password".into()),
                true,
                window,
                cx,
            );
            self.field("Password", field)
        });
        let primary = match (state.key_exists || state.locked, failed) {
            (true, _) => Some(("unlock", "Unlock")),
            (false, true) => Some(("unlock", "Try again")),
            // opening on its own: nothing to press
            (false, false) => None,
        }
        .map(|(id, label)| {
            div().flex().child(square(
                self.action(
                    id,
                    label,
                    || Message::UnlockSubmit,
                    state.unlock_busy || state.seating,
                )
                .primary(),
            ))
        });
        let form = div()
            .flex()
            .flex_col()
            .gap_3()
            .children(other_chain)
            .children(password)
            .children(failed.then(|| self.alert("unlock-error", state.unlock_error.clone(), cx)))
            .children(primary)
            .children((!failed && state.seating).then(|| {
                div()
                    .id("seating")
                    .role(Role::Status)
                    .text_size(px(13.))
                    .text_color(colors.muted_foreground)
                    .child("Asking the system for it…")
            }));
        let links = div().flex().flex_col().gap_2().child(self.quiet_link(
            "browse",
            "Read without a key",
            || Message::BrowseWithoutKey,
            cx,
        ));
        self.launcher(
            "sign-in",
            Figure::Ring,
            Self::where_(state),
            Some(("disconnect", "← Other networks", || Message::Disconnect)),
            label,
            headline,
            Some(lead.into()),
            vec![form.into_any_element(), links.into_any_element()],
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
        if !state.link_code.is_empty() {
            return self.link_waiting(state, window, cx);
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
        // An account made elsewhere: this device's key joins it, and
        // something the account already trusts says yes — another device
        // of it, its passkey, or its recovery key.
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
                    .child("Add this device to it. Something the account already trusts says yes; this device's key then signs for the account too."),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(square(
                        self.action(
                            "link-device",
                            "From another device",
                            || Message::LinkStart,
                            state.unlock_busy,
                        )
                        .outline(),
                    ))
                    .child(square(
                        self.action(
                            "passkey-sign-in",
                            "With a passkey",
                            || Message::PasskeySignInSubmit,
                            state.unlock_busy,
                        )
                        .outline(),
                    ))
                    .child(square(
                        self.action(
                            "recover",
                            "With a recovery key",
                            || Message::RecoverShow,
                            state.unlock_busy,
                        )
                        .outline(),
                    )),
            );
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

    /// This device waits under a short code for one already on the account
    /// to approve it (the account menu's "Add a device…"). The fingerprint
    /// is what the person compares on both screens before saying yes.
    fn link_waiting(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let colors = gpui_kit::component::Theme::global(cx).color_tokens();
        let fingerprint = crate::backend::hex_decode(&state.signer_key)
            .map(|key| crate::backend::join::fingerprint(&key))
            .unwrap_or_default();
        let big = |id: &'static str, label: &'static str, text: String| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(mono(label, colors.muted_foreground))
                .child(crate::a11y::whole(
                    div()
                        .id(id)
                        .role(Role::Label)
                        .aria_label(text.clone())
                        .text_size(px(28.))
                        .font_family(super::theme::FAMILY_MONO)
                        .child(text),
                ))
        };
        let body = vec![
            div()
                .flex()
                .gap_8()
                .child(big("link-code", "Code", state.link_code.clone()))
                .child(big("link-fingerprint", "This device", fingerprint))
                .into_any_element(),
            div()
                .id("link-waiting-status")
                .role(Role::Status)
                .text_size(px(13.))
                .line_height(px(20.))
                .text_color(colors.muted_foreground)
                .child("On a device already signed in, open the account menu, choose \"Add a device…\" and type the code. Check that it shows the same four-and-four before approving. The code lasts five minutes.")
                .into_any_element(),
            div()
                .flex()
                .flex_col()
                .gap_3()
                .children(
                    (!state.unlock_error.is_empty())
                        .then(|| self.alert("link-error", state.unlock_error.clone(), cx)),
                )
                .child(div().flex().child(square(
                    self.action("link-cancel", "Cancel", || Message::LinkCancel, false)
                        .outline(),
                )))
                .into_any_element(),
        ];
        self.launcher(
            "link-waiting",
            Figure::Pair,
            Self::where_(state),
            None,
            "[04 / 04] Account · another device".into(),
            "Approve this device".into(),
            None,
            body,
            window,
            cx,
        )
    }

    /// The account's recovery key, typed: its 24 words say yes to this
    /// device's key joining. A phrase from a key made before keys moved into
    /// the system works too — that key is on the account already.
    pub(super) fn recover(
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
            || Message::RecoverSubmit,
            Some("Recovery key".into()),
            true,
            window,
            cx,
        );
        let label = match state.unlock_busy {
            true => "Adding…",
            false => "Add this device",
        };
        let form = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.field("Recovery key", phrase))
            .children(
                (!state.unlock_error.is_empty())
                    .then(|| self.alert("unlock-error", state.unlock_error.clone(), cx)),
            )
            .child(
                div().flex().child(square(
                    self.action(
                        "recover-submit",
                        label,
                        || Message::RecoverSubmit,
                        state.unlock_busy,
                    )
                    .primary(),
                )),
            );
        self.launcher(
            "recover",
            Figure::Sheets,
            Self::where_(state),
            Some(("recover-back", "← Back", || Message::RecoverCancel)),
            "[04 / 04] Account · recovery key".into(),
            format!("Your {} recovery key", state.network),
            Some("The 24 words you wrote down for this account. They add this device; nothing else changes.".into()),
            vec![form.into_any_element()],
            window,
            cx,
        )
    }

    /// A new recovery key for the account, then a check that it was
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
            )
            .child(self.quiet_link("phrase-cancel", "Not now", || Message::PhraseCancel, cx));
        self.launcher(
            "recovery",
            Figure::Sheets,
            "Twenty-four words, on paper.".into(),
            None,
            "Recovery key".into(),
            "Write these down".into(),
            Some(format!("In order, on paper. With them, a new device joins your {} account when no other device is at hand — and so can anyone holding them. They show only now.", state.network)),
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
            Some(("phrase-show", "← Show the words again", || {
                Message::PhraseShowAgain
            })),
            "Recovery key · check".into(),
            "Now, three of them".into(),
            None,
            vec![form.into_any_element()],
            window,
            cx,
        )
    }
}
