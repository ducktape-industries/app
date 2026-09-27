//! The launcher's recovery-key screens: type an account's 24 words to add
//! this device (`recover`); a new key's words shown once (`phrase`), then
//! three of them asked back (`phrase_check`).

use super::ink::{self, *};
use super::launcher::LauncherScreen;
use super::launcher::{buttons, node_caption};
use super::text_field::TextField;
use super::*;
use facts::Facts;
use figure::Figure;

/// The phrase check's `nth` typed word.
fn answer(state: &Ducktape, nth: usize) -> &str {
    match &state.stage {
        crate::Stage::Phrase(step) => step.answers[nth].as_str(),
        _ => "",
    }
}

impl DesktopWindow {
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
            TextField {
                key: "restore-phrase",
                placeholder: "24 words, separated by spaces",
                masked: false,
                value: |state| match &state.stage {
                    crate::Stage::Recover(step) => step.phrase.as_str(),
                    _ => "",
                },
                on_change: Message::RestorePhraseTyped,
                on_enter: || Message::RecoverSubmit,
                label: Some("Recovery key".into()),
                secret: true,
                size: 15.,
            },
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
            LauncherScreen {
                id: "recover",
                tight: false,
                figure: Figure::Card,
                caption: node_caption(state),
                back: Some(("recover-back", "← Back", || Message::RecoverCancel)),
                label: "[03 / 03] Account · recovery key".into(),
                headline: format!("Your {} recovery key", state.network),
                lead: Some("The 24 words you wrote down for this account. They add this device; nothing else changes.".into()),
                body: vec![form.into_any_element()],
            },
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
        // read here, not copied into every draw's facts; wiped when dropped
        let phrase = zeroize::Zeroizing::new(match &self.model.read(cx).state.stage {
            crate::Stage::Phrase(step) => step.words.to_string(),
            _ => String::new(),
        });
        let words: Vec<&str> = phrase.split_whitespace().collect();
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
                .aria_label(phrase.as_str().to_owned())
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
            LauncherScreen {
                id: "recovery",
                tight: true, // the 24 words need the room
                figure: Figure::Card,
                caption: "Twenty-four words, on paper.".into(),
                back: None,
                label: "Recovery key".into(),
                headline: "Write these down".into(),
                lead: Some(format!(
                "In order, on paper. With them a new device joins your {} account when no other is at hand — and so can anyone holding them.",
                state.network
            )),
                body: vec![sheet.into_any_element(), done.into_any_element()],
            },
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
                |state| answer(state, 0),
                |text| Message::PhraseWordTyped(0, text),
            ),
            (
                "phrase-word-2",
                |state| answer(state, 1),
                |text| Message::PhraseWordTyped(1, text),
            ),
            (
                "phrase-word-3",
                |state| answer(state, 2),
                |text| Message::PhraseWordTyped(2, text),
            ),
        ];
        let [a, b, c] = asked.map(|nth| nth + 1);
        let rows: Vec<_> = fields
            .into_iter()
            .zip(asked)
            .map(|((key, value, on_change), nth)| {
                let field = self.input(
                    TextField {
                        key,
                        placeholder: "",
                        masked: false,
                        value,
                        on_change,
                        on_enter: || Message::PhraseCheckSubmit,
                        label: Some(format!("Word {}", nth + 1).into()),
                        secret: true,
                        size: 15.,
                    },
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
            LauncherScreen {
                id: "recovery-check",
                tight: false,
                figure: Figure::Card,
                caption: "Twenty-four words, on paper.".into(),
                back: None,
                label: "Recovery key · check".into(),
                headline: "Now, three of them".into(),
                lead: Some(prompt),
                body: vec![form.into_any_element()],
            },
            window,
            cx,
        )
    }
}
