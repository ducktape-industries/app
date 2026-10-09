//! The launcher's recovery-key screens: type an account's 24 words to add
//! this device (`recover`); a new key's words shown once (`phrase`), then
//! three of them asked back (`phrase_check`).

use super::super::super::entities::Account;
use super::super::super::ink::{self, *};
use super::super::fields::TextField;
use super::{LauncherLayer, LauncherScreen, buttons};
use gpui_kit::*;

impl LauncherLayer {
    /// The account's recovery key, typed: its 24 words say yes to this
    /// device's key joining. An old device phrase works as one.
    pub(super) fn recover(&self, window: &mut Window, cx: &App) -> AnyElement {
        let account = self.account.read(cx).get();
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let failed = !account.error.is_empty();
        let phrase = self.fields.restore.input(
            "restore-phrase",
            TextField {
                label: "Recovery key".into(),
                private: true,
                error: failed.then(|| account.error.clone()),
                size: 15.,
            },
            cx,
        );
        let focused = self.fields.restore.focused(window, cx);
        let form = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(field(
                "restore-phrase-label",
                "Recovery key",
                field_box(
                    phrase,
                    focused,
                    match failed {
                        true => ink.danger,
                        false => ink.field,
                    },
                    44.,
                    &ink,
                )
                .into_any_element(),
                failed.then(|| alert("unlock-error", account.error.clone(), &ink)),
                &ink,
            ))
            .child(buttons([ink::button(
                "recover-submit",
                match account.busy {
                    true => "Adding…",
                    false => "Add this device",
                },
                Kind::Primary,
                self.with_field(&self.fields.restore, Account::recover_submit),
                Press::busy(account.busy),
                &ink,
            )]));
        self.frame(
            LauncherScreen {
                id: "recover",
                tight: false,
                caption: self.node_caption(cx),
                back: Some((
                    "recover-back",
                    "← Back",
                    Box::new(self.on_account(Account::recover_cancel)),
                )),
                label: "[03 / 04] Account · recovery key".into(),
                headline: format!("Your {} recovery key", self.session.read(cx).get().network),
                lead: Some("The 24 words you wrote down for this account. They add this device; nothing else changes.".into()),
                body: vec![form.into_any_element()],
            },
            window,
            cx,
        )
    }

    /// Phrase: a new recovery key's 24 words.
    pub(super) fn phrase(&self, window: &mut Window, cx: &App) -> AnyElement {
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let phrase = self.phrase.as_deref().map_or("", String::as_str);
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
                        // the sheet's name reads the words; the numbers
                        // stay out of the tree
                        .child(
                            mono(400, 12.)
                                .text_color(ink.muted)
                                .w(px(18.))
                                .child(format!("{}", column * per_column + row + 1)),
                        )
                        .child(sans(400, 15.).child(word.to_string()))
                }))
        });
        let sheet = crate::a11y::private(
            div()
                .id("phrase")
                .role(Role::Group)
                .aria_label(phrase.to_owned())
                .flex()
                .gap(px(24.))
                .children(columns),
        );
        let done = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .child(ink::button(
                "phrase-done",
                "I wrote them down",
                Kind::Primary,
                self.on_account(Account::phrase_written_down),
                false,
                &ink,
            ))
            .child(ink::note(
                "phrase-warning",
                "Nobody can recover them for you.",
                ink.muted,
            ))
            .child(link_running(
                "phrase-cancel",
                "Not now",
                self.on_account(Account::phrase_cancel),
                false,
                &ink,
            ));
        self.frame(
            LauncherScreen {
                id: "recovery",
                tight: true, // the 24 words need the room
                caption: "Twenty-four words, on paper.".into(),
                back: None,
                label: "Recovery key".into(),
                headline: "Write these down".into(),
                lead: Some(format!(
                "In order, on paper. With them a new device joins your {} account when no other is at hand — and so can anyone holding them.",
                self.session.read(cx).get().network
            )),
                body: vec![sheet.into_any_element(), done.into_any_element()],
            },
            window,
            cx,
        )
    }

    /// PhraseCheck: three words typed back from the paper.
    pub(super) fn phrase_check(
        &self,
        asked: [usize; 3],
        window: &mut Window,
        cx: &App,
    ) -> AnyElement {
        let account = self.account.read(cx).get();
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let keys = ["phrase-word-1", "phrase-word-2", "phrase-word-3"];
        let [a, b, c] = asked.map(|nth| nth + 1);
        let rows: Vec<_> = (self.fields.words.iter().zip(keys))
            .zip(asked)
            .map(|((word, key), nth)| {
                let input = word.input(
                    key,
                    TextField {
                        label: format!("Word {}", nth + 1).into(),
                        private: true,
                        error: None,
                        size: 15.,
                    },
                    cx,
                );
                let focused = word.focused(window, cx);
                field(
                    SharedString::from(format!("{key}-label")),
                    format!("Word {}", nth + 1),
                    field_box(input, focused, ink.field, 44., &ink).into_any_element(),
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
                (!account.error.is_empty())
                    .then(|| alert("unlock-error", account.error.clone(), &ink)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .mt(px(4.))
                    .child(ink::button(
                        "phrase-check",
                        "Confirm",
                        Kind::Primary,
                        self.check_phrase(),
                        account.busy,
                        &ink,
                    ))
                    .child(link_running(
                        "phrase-show",
                        "See the words again",
                        self.on_account(Account::phrase_show_again),
                        false,
                        &ink,
                    )),
            );
        let prompt = format!("Type words {a}, {b} and {c} from your paper.");
        self.frame(
            LauncherScreen {
                id: "recovery-check",
                tight: false,
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
