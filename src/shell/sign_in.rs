//! The launcher's key and account screens, each ported from its board on
//! the design canvas (SignIn, Passkey, CreateAccount, Phrase, PhraseCheck).
//! The KEY is this device's, kept by the system and opened on its own: its
//! screen shows only while it opens, after a Lock, or for a password-locked
//! key from before. The ACCOUNT is the network's: create one, or add this
//! device to one — another device of it, its passkey or its recovery key
//! says yes. No program is named here.

use super::ink::{self, *};
use super::*;
use figure::Figure;
use screens::Facts;

/// The canvas's button row: `display: flex; gap: 12px; margin-top: 4px`.
fn buttons(children: impl IntoIterator<Item = gpui_kit::AnyElement>) -> gpui_kit::Div {
    use gpui_kit::*;
    div()
        .flex()
        .flex_wrap()
        .gap(px(12.))
        .mt(px(4.))
        .children(children)
}

/// The canvas's closing links: `gap: 10px; padding-top: 20px;
/// border-top: 1px solid line`.
fn closing(children: impl IntoIterator<Item = gpui_kit::AnyElement>, ink: &Ink) -> gpui_kit::Div {
    use gpui_kit::*;
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(10.))
        .pt(px(20.))
        .border_t_1()
        .border_color(ink.line)
        .children(children)
}

impl DesktopWindow {
    /// The node reached, as the drawing's caption: "testkit · 127.0.0.1:8844".
    fn where_(state: &Facts) -> String {
        let host = crate::backend::host_of(&state.connected_rpc);
        format!("{} · {host}", state.network)
    }

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
                "[02 / 04] Key".to_string(),
                "The key didn't open".to_string(),
                Some("The system holds this device's key and didn't hand it over."),
            ),
            (false, false, false) => (
                "[02 / 04] Key".to_string(),
                "Opening this device's key…".to_string(),
                Some(
                    "It signs what you write, and the system keeps it: no password, nothing to write down.",
                ),
            ),
        };
        let other_chain = state.other_chain.then(|| {
            ink::note(
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
                "password",
                "",
                true,
                |state| &state.password,
                Message::PasswordTyped,
                || Message::UnlockSubmit,
                Some("Password".into()),
                true,
                15.,
                window,
                cx,
            );
            let border = match failed {
                true => ink.danger,
                false => ink.strong,
            };
            self.field(
                "Password for this device's key",
                field_box(field, border, 44., &ink).into_any_element(),
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
            "sign-in",
            Figure::Ring,
            Self::where_(state),
            Some(("disconnect", "Other networks", || Message::Disconnect)),
            label,
            headline,
            lead.map(str::to_owned),
            vec![form.into_any_element(), links.into_any_element()],
            window,
            cx,
        )
    }

    /// Passkey: the ceremony is in the browser, or on a phone through the QR.
    fn passkey_waiting(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let (title, hint) = match state.passkey_qr {
            Some(_) => (
                "Scan with your phone",
                "Point its camera at the code. It asks for your passkey twice; this screen moves on by itself.",
            ),
            None => (
                "Continue in your browser",
                "Your browser asks for your passkey twice. This screen moves on by itself.",
            ),
        };
        let row = state.passkey_qr.as_ref().map(|url| {
            let copy = {
                let url = url.clone();
                crate::a11y::keyboard(
                    sans(400, 14.)
                        .id("passkey-qr-copy")
                        .control(Role::Button, "Copy the link instead")
                        .underline()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                        })
                        .child("Copy the link instead"),
                )
            };
            div()
                .flex()
                .items_start()
                .gap(px(20.))
                .child(
                    div()
                        .id("passkey-qr")
                        .role(Role::Image)
                        .aria_label("Passkey QR code")
                        .size(px(168.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border(px(1.5))
                        .border_color(ink.ink)
                        .bg(gpui_kit::white())
                        .child(crate::render::qr(&view_wire::Qr {
                            payload: Some(url.clone().into_bytes()),
                            size: Some(view_wire::QrSize::Total(150.)),
                            ..Default::default()
                        })),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(10.))
                        .child(
                            tag("Waiting", &ink)
                                .id("passkey-waiting-status")
                                .role(Role::Status),
                        )
                        .child(sans(500, 16.).child("Your passkey, twice"))
                        .child(ink::note(
                            "A new code appears here for the second time.",
                            ink.muted,
                        ))
                        .child(copy),
                )
                .into_any_element()
        });
        let links = closing(
            [
                match state.passkey_qr {
                    Some(_) => None,
                    None => Some(self.link(
                        "passkey-use-phone",
                        "Use a phone instead",
                        || Message::PasskeyUsePhone,
                        false,
                        &ink,
                    )),
                },
                Some(self.link(
                    "passkey-cancel",
                    "Cancel",
                    || Message::PasskeyCancel,
                    false,
                    &ink,
                )),
            ]
            .into_iter()
            .flatten(),
            &ink,
        );
        self.launcher(
            "passkey-waiting",
            Figure::Pair,
            Self::where_(state),
            None,
            "[04 / 04] Account · passkey".into(),
            title.into(),
            Some(hint.into()),
            row.into_iter().chain([links.into_any_element()]).collect(),
            window,
            cx,
        )
    }

    /// CreateAccount: name the account this key signs for, or add this
    /// device to one that exists.
    pub(super) fn account_step(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        if state.passkey_waiting {
            return self.passkey_waiting(state, window, cx);
        }
        if !state.link_code.is_empty() {
            return self.link_waiting(state, window, cx);
        }
        let ink = Ink::of(state.dark);
        let name = self.input(
            "create-account-name",
            "",
            false,
            |state| &state.account_name,
            Message::AccountNameTyped,
            || Message::CreateAccountSubmit,
            Some("Account name".into()),
            false,
            22.,
            window,
            cx,
        );
        let below = match state.unlock_error.is_empty() {
            true => ink::note(
                "The account number is given when it's created. Change the name later in Settings.",
                ink.muted,
            )
            .into_any_element(),
            false => self.alert("create-account-error", state.unlock_error.clone(), &ink),
        };
        let busy = state.unlock_busy;
        let form = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                self.field(
                    "Name",
                    sans(400, 22.)
                        .child(field_box(name, ink.strong, 56., &ink).text_size(px(22.)))
                        .into_any_element(),
                    Some(below),
                    &ink,
                ),
            )
            .child(
                buttons([
                    self.button(
                        "create-account",
                        match busy {
                            true => "Creating…",
                            false => "Create account",
                        },
                        Kind::Primary,
                        || Message::CreateAccountSubmit,
                        busy,
                        &ink,
                    ),
                    self.button(
                        "passkey-create",
                        "Create with a passkey",
                        Kind::Secondary,
                        || Message::PasskeyCreateSubmit,
                        busy,
                        &ink,
                    ),
                    self.link(
                        "create-account-later",
                        "Not now",
                        || Message::CreateAccountLater,
                        true,
                        &ink,
                    ),
                ])
                .items_center(),
            );
        // The common way in gets its own line; the other two share one.
        let join = closing(
            [
                sans(500, 14.)
                    .child(format!("Already have an account on {}?", state.network))
                    .into_any_element(),
                self.link(
                    "link-device",
                    "Add this device from another device",
                    || Message::LinkStart,
                    false,
                    &ink,
                ),
                div()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap(px(5.))
                    .child(ink::note("Or use", ink.muted))
                    .child(self.link(
                        "passkey-sign-in",
                        "a passkey",
                        || Message::PasskeySignInSubmit,
                        true,
                        &ink,
                    ))
                    .child(ink::note("or", ink.muted))
                    .child(self.link(
                        "recover",
                        "a recovery key",
                        || Message::RecoverShow,
                        true,
                        &ink,
                    ))
                    .into_any_element(),
            ],
            &ink,
        )
        .id("join-account")
        .role(Role::Group)
        .aria_label("Already have an account");
        self.launcher(
            "account-step",
            Figure::Pair,
            Self::where_(state),
            None,
            "[04 / 04] Account".into(),
            "What should people call you?".into(),
            Some(format!(
                "Your account is the name beside everything you write on {}. This device's key signs for it.",
                state.network
            )),
            vec![form.into_any_element(), join.into_any_element()],
            window,
            cx,
        )
    }

    /// This device waits under a short code for one already on the account
    /// to approve it; the fingerprint is compared on both screens.
    fn link_waiting(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let fingerprint = crate::backend::hex_decode(&state.signer_key)
            .map(|key| crate::backend::join::fingerprint(&key))
            .unwrap_or_default();
        let big = |id: &'static str, name: &'static str, text: String| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(tag(name, &ink))
                .child(crate::a11y::whole(
                    mono(400, 28.)
                        .id(id)
                        .role(Role::Label)
                        .aria_label(text.clone())
                        .child(text),
                ))
        };
        let body = vec![
            div()
                .flex()
                .gap(px(40.))
                .child(big("link-code", "Code", state.link_code.clone()))
                .child(big("link-fingerprint", "This device", fingerprint))
                .into_any_element(),
            ink::note(
                "On a device already signed in, open the account menu, choose \"Add a device…\" and type the code. Approve there only if it shows the same four-and-four. The code lasts five minutes.",
                ink.muted,
            )
            .id("link-waiting-status")
            .role(Role::Status)
            .into_any_element(),
            div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .children((!state.unlock_error.is_empty())
                    .then(|| self.alert("link-error", state.unlock_error.clone(), &ink)))
                .child(buttons([self.button(
                    "link-cancel",
                    "Cancel",
                    Kind::Secondary,
                    || Message::LinkCancel,
                    false,
                    &ink,
                )]))
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
    /// device's key joining. An old device phrase works as one.
    pub(super) fn recover(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let phrase = self.input(
            "restore-phrase",
            "24 words, separated by spaces",
            false,
            |state| &state.restore_phrase,
            Message::RestorePhraseTyped,
            || Message::RecoverSubmit,
            Some("Recovery key".into()),
            true,
            15.,
            window,
            cx,
        );
        let failed = !state.unlock_error.is_empty();
        let form = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                self.field(
                    "Recovery key",
                    field_box(
                        phrase,
                        match failed {
                            true => ink.danger,
                            false => ink.strong,
                        },
                        44.,
                        &ink,
                    )
                    .into_any_element(),
                    failed.then(|| self.alert("unlock-error", state.unlock_error.clone(), &ink)),
                    &ink,
                ),
            )
            .child(buttons([self.button(
                "recover-submit",
                match state.unlock_busy {
                    true => "Adding…",
                    false => "Add this device",
                },
                Kind::Primary,
                || Message::RecoverSubmit,
                state.unlock_busy,
                &ink,
            )]));
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

    /// Phrase: a new recovery key's 24 words, then a check of three.
    pub(super) fn phrase(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        if let Some(asked) = state.phrase_quiz {
            return self.phrase_check(state, asked, window, cx);
        }
        let ink = Ink::of(state.dark);
        let words: Vec<&str> = state.phrase.split_whitespace().collect();
        let per_column = words.len().div_ceil(3).max(1);
        // `grid-template-columns: repeat(3, 1fr); grid-auto-flow: column;
        // column-gap: 24px`, each `<li>` `gap 10px; padding 6px 0;
        // border-bottom 1px`
        let columns = words.chunks(per_column).enumerate().map(|(column, chunk)| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .children(chunk.iter().enumerate().map(|(row, word)| {
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(10.))
                        .py(px(6.))
                        .border_b_1()
                        .border_color(ink.line)
                        .child(tag(format!("{}", column * per_column + row + 1), &ink).w(px(18.)))
                        .child(sans(400, 15.).child(word.to_string()))
                }))
        });
        let sheet = crate::a11y::private(
            div()
                .id("phrase")
                .role(Role::Group)
                .aria_label(state.phrase.clone())
                .flex()
                .gap(px(24.))
                .children(columns),
        );
        let done = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .child(self.button(
                "phrase-done",
                "I wrote them down",
                Kind::Primary,
                || Message::PhraseWrittenDown,
                false,
                &ink,
            ))
            .child(ink::note("Nobody can recover them for you.", ink.muted))
            .child(self.link(
                "phrase-cancel",
                "Not now",
                || Message::PhraseCancel,
                false,
                &ink,
            ));
        self.launcher(
            "recovery",
            Figure::Sheets,
            "Twenty-four words, on paper.".into(),
            None,
            "Recovery key".into(),
            "Write these down".into(),
            Some(format!(
                "In order, on paper. With them a new device joins your {} account when no other is at hand — and so can anyone holding them.",
                state.network
            )),
            vec![sheet.into_any_element(), done.into_any_element()],
            window,
            cx,
        )
    }

    /// PhraseCheck: three words typed back from the paper.
    fn phrase_check(
        &mut self,
        state: &Facts,
        asked: [usize; 3],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
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
        let [a, b, c] = asked.map(|nth| nth + 1);
        let rows: Vec<_> = fields
            .into_iter()
            .zip(asked)
            .map(|((key, value, typed), nth)| {
                let field = self.input(
                    key,
                    "",
                    false,
                    value,
                    typed,
                    || Message::PhraseCheckSubmit,
                    Some(format!("Word {}", nth + 1).into()),
                    true,
                    15.,
                    window,
                    cx,
                );
                self.field(
                    format!("Word {}", nth + 1),
                    field_box(field, ink.strong, 44., &ink).into_any_element(),
                    None,
                    &ink,
                )
                .into_any_element()
            })
            .collect();
        let form = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .children(rows)
            .children(
                (!state.unlock_error.is_empty())
                    .then(|| self.alert("unlock-error", state.unlock_error.clone(), &ink)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .mt(px(4.))
                    .child(self.button(
                        "phrase-check",
                        "Confirm",
                        Kind::Primary,
                        || Message::PhraseCheckSubmit,
                        state.unlock_busy,
                        &ink,
                    ))
                    .child(self.link(
                        "phrase-show",
                        "See the words again",
                        || Message::PhraseShowAgain,
                        false,
                        &ink,
                    )),
            );
        let prompt = format!("Type words {a}, {b} and {c} from your paper.");
        self.launcher(
            "recovery-check",
            Figure::Sheets,
            "Twenty-four words, on paper.".into(),
            None,
            "Recovery key · check".into(),
            "Now, three of them".into(),
            Some(prompt),
            vec![form.into_any_element()],
            window,
            cx,
        )
    }
}
