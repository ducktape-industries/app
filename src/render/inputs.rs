//! `Node::Field`: every text field a view draws, one line or many, held by
//! the kit's editing engine. The engine owns the text: every key, selection,
//! IME preedit and undo is its own work, and the guest hears each change as
//! `Event::Text` at the host's `revision`. The guest asks for an edit with
//! `WidgetCommand::Replace` against the revision it has seen, and a stale
//! ask is carried over what was typed since (`wire::rebase`), never
//! dropped, so no key waits on the guest and nothing typed is lost. The
//! guest's own text is adopted only on a fresh mount or when its
//! `generation` moves; every other frame leaves the engine's text alone.

use super::*;
use crate::render::native_id;
use gpui_kit::base::input::{InlineToken, InputContent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{Edges, EntityInputHandler as _, Keystroke};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// The tallest a growing field gets before it scrolls inside itself. The
/// field's byte cap is the real limit; this is only the point past which
/// growing the element stops being how anyone reads it.
const MAX_ROWS: usize = 4096;

/// The kit state a field's text lives in: one line, or many.
#[derive(Clone)]
pub(super) enum Engine {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

/// Runs `body` on whichever state the field holds.
macro_rules! engine {
    ($engine:expr, |$state:ident| $body:expr) => {
        match $engine {
            Engine::Line($state) => $body,
            Engine::Area($state) => $body,
        }
    };
}

/// An ask held while an IME composes: the engine's text is the IME's until
/// it commits, and an edit under a preedit would be an edit in it.
enum Deferred {
    Replace(wire::WidgetCommand),
    Adopt(String, Vec<wire::TextToken>, Range<usize>),
}

/// A mounted `Node::Field`: the engine's state, the guest's routes, and the
/// text as last reported, which every later report is measured against.
pub(super) struct Field {
    pub(super) engine: Engine,
    on_change: Option<u32>,
    on_key: Option<u32>,
    on_submit: Option<u32>,
    claims: Vec<wire::KeyClaim>,
    /// The guest's generation the engine's text was adopted from.
    generation: u64,
    /// The guest's revision as of the frame before this one. Every ask a
    /// frame carries was built after the frame before it, so the log is
    /// kept from there.
    seen: u64,
    /// The engine's text, cursor and preedit as last reported, at `revision`.
    text: String,
    cursor: wire::TextRange,
    preedit: Option<wire::TextRange>,
    revision: u64,
    /// Every edit since `seen`, at the revision that made it.
    log: Vec<(u64, wire::Edit)>,
    deferred: Vec<Deferred>,
    /// Esc let go of Tab: the next Tab leaves the field instead of
    /// indenting. Any other key takes it back.
    tab_released: bool,
    placeholder: String,
    secure: bool,
    /// Whether a growing field fills its box or grows to `cap` rows.
    fills: bool,
    cap: Option<usize>,
    _observation: Subscription,
    _events: Subscription,
}

/// What a key pressed in a focused field came to.
enum Pressed {
    /// The guest claimed it: its event, which stops the key here.
    Claimed(wire::Event),
    /// The field spent it (an indent).
    Taken,
    Passed,
}

impl ViewTree {
    pub(super) fn field(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Field {
            id,
            multiline,
            value,
            cursor,
            generation,
            revision,
            tokens,
            claims,
            options,
            placeholder,
            secure,
            on_change,
            on_key,
            on_submit,
            style,
        } = node
        else {
            unreachable!()
        };
        let path = self.authored_path.clone();
        debug_assert_eq!(path.last(), Some(id));
        // Native key bindings consume Enter, Tab and navigation before
        // element key listeners: a guest's claim runs at gpui's pre-action
        // seam, one interceptor for the tree's fields.
        if self.keystrokes.is_none() {
            let tree = cx.entity().downgrade();
            self.keystrokes = Some(cx.intercept_keystrokes(move |event, window, cx| {
                let _ = tree.update(cx, |this, cx| this.field_key(&event.keystroke, window, cx));
            }));
        }
        if !self.fields.contains_key(&path) {
            let saved = self
                .presentation
                .inputs
                .remove(&path)
                .filter(|saved| saved.value == *value && saved.secure == *secure);
            let engine = match multiline {
                false => Engine::Line(cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder.clone())
                        .masked(*secure)
                })),
                true => Engine::Area(cx.new(|cx| {
                    let mut state = TextareaState::new(window, cx)
                        .placeholder(placeholder.clone())
                        .auto_grow(1, MAX_ROWS)
                        .soft_wrap(true)
                        .searchable(false)
                        .context_menu(false);
                    state.set_editor_paddings(Edges::all(px(0.)));
                    state
                })),
            };
            let observed = path.clone();
            let observation = engine!(&engine, |state| cx.observe_in(
                state,
                window,
                move |this, _, window, cx| this.field_observed(&observed, window, cx)
            ));
            let submitted = path.clone();
            let events = engine!(&engine, |state| cx.subscribe_in(
                state,
                window,
                move |this, _, event, _, cx| {
                    if let InputEvent::PressEnter { .. } = event
                        && let Some(message) = this.fields.get(&submitted).and_then(|f| f.on_submit)
                    {
                        this.activate();
                        cx.emit(wire::Event::Message(message));
                    }
                }
            ));
            let mut field = Field {
                engine,
                on_change: *on_change,
                on_key: *on_key,
                on_submit: *on_submit,
                claims: claims.clone(),
                generation: *generation,
                seen: *revision,
                text: String::new(),
                cursor: Default::default(),
                preedit: None,
                revision: *revision,
                log: Vec::new(),
                deferred: Vec::new(),
                tab_released: false,
                placeholder: placeholder.clone(),
                secure: *secure,
                fills: true,
                cap: None,
                _observation: observation,
                _events: events,
            };
            // the guest's text, with the selection its user left in it if
            // this is the same text a new generation of the guest shows
            let (selection, focused) = match saved {
                Some(saved) => (saved.selection, saved.focused),
                None => (cursor.range(), false),
            };
            field.install(value, tokens, selection, window, cx);
            field.text = value.clone();
            field.cursor = field.read(window, cx).1;
            if focused {
                field.focus(window, cx);
            }
            self.fields.insert(path.clone(), field);
        }
        let editable = !options.disabled && !options.read_only;
        let field = self.fields.get_mut(&path).expect("field inserted");
        field.on_change = *on_change;
        field.on_key = *on_key;
        field.on_submit = *on_submit;
        field.claims.clone_from(claims);
        field.prune();
        field.seen = *revision;
        let mut events = Vec::new();
        if field.generation != *generation {
            field.generation = *generation;
            events.extend(field.adopt(value, tokens, cursor.range(), window, cx));
        }
        if field.placeholder != *placeholder {
            field.placeholder.clone_from(placeholder);
            engine!(&field.engine, |state| state.update(cx, |state, cx| {
                state.set_placeholder(placeholder.clone(), window, cx)
            }));
        }
        if field.secure != *secure {
            field.secure = *secure;
            if let Engine::Line(state) = &field.engine {
                state.update(cx, |state, cx| state.set_masked(*secure, window, cx));
            }
        }
        if let Engine::Area(state) = field.engine.clone() {
            // A height the guest gave is a box to fill; none (a composer's
            // `min_h`..`max_h`) is a field as tall as its lines, which the
            // field's own auto-grow reports up to the parent that waits on it.
            field.set_fills(style.size.height.is_some(), field_rows(style), cx);
            // read-only as the kit's: it refuses what the user types, and
            // takes what the guest asks
            if state.read(cx).is_editable() != editable {
                state.update(cx, |state, cx| state.set_readonly(!editable, cx));
            }
        }
        let engine = field.engine.clone();
        let fills = field.fills;
        let accessible = Accessible {
            value: (!secure).then(|| field.text.clone()),
            ..accessible(node)
        };
        for event in events {
            cx.emit(event);
        }
        let focus = engine!(&engine, |state| state.read(cx).focus_handle(cx));
        // a disabled field wears no ring, as the kit's own look would not
        let focused = focus.is_focused(window) && !options.disabled;
        // assistive technology's set-value is input too, taken as the
        // kit's own programmatic change; a field that takes no typing
        // takes none of it either
        let set_value = {
            let tree = cx.entity().downgrade();
            let path = path.clone();
            move |value: String, window: &mut Window, cx: &mut App| {
                let _ = tree.update(cx, |this, cx| {
                    let Some(field) = this.fields.get(&path) else {
                        return;
                    };
                    engine!(&field.engine, |state| state.update(cx, |state, cx| {
                        if state.is_editable() {
                            state.replace_all(value, window, cx);
                        }
                    }));
                });
            }
        };
        match &engine {
            Engine::Line(state) => {
                // The field's one node is `a11y::text_field`, carrying the
                // mapping; a secure field is a password, which keeps its value
                // out of the tree. The wrapper wears the view's id and is the
                // box the view's parent lays out (`placed`); the kit's input
                // inside takes a host name nested under it, where no view
                // child can be. The kit fixes the line at 20px inside `8px`
                // padding: a face over ~16px is clipped top and bottom. The
                // line follows the face and the kit's height centres it; a
                // view's own style still wins. Focused, the kit's box wears
                // the ring on itself (`around_field`); the kit's own focus
                // look is off: a second ring painted round the outside, and a
                // border colour laid over any style given here.
                let (placed, drawn) = placed(style);
                let mut input = Input::new(state)
                    .id(host_id("input"))
                    .disabled(options.disabled)
                    .readonly(options.read_only)
                    .focus_bordered(false)
                    .line_height(gpui_kit::relative(1.4))
                    .py_0()
                    .refine_style(&drawn);
                if *secure {
                    input = input.content_type(InputContentType::Password);
                }
                let input = crate::a11y::around_field(input, focused, crate::a11y::ink(cx));
                let field = crate::a11y::text_field(
                    native_id(id),
                    &focus,
                    set_value,
                    input.role(gpui_kit::component::RoleOverride::Presentational),
                )
                .refine_style(&placed);
                announce(field, accessible).into_any_element()
            }
            Engine::Area(state) => {
                // The view's box is the field's, border and padding: it wears
                // the ring, not the text inside the padding. The box the
                // guest gave, not the room the words take: a press in the
                // empty part of a card is a press on the card's writing. A
                // field asked to shrink has no empty part to press — its box
                // IS its words — and taking the parent's height there would
                // be taking the height the parent is waiting on this field
                // to report.
                let pressed = path.clone();
                let words = div()
                    .relative()
                    .w_full()
                    .when(fills, |element| element.h_full())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.field_pressed(&pressed, event, window, cx)
                        }),
                    )
                    .child(announce(
                        // the base Textarea draws no node of its own, nor a box
                        crate::a11y::text_field(
                            "editor-field",
                            &focus,
                            set_value,
                            Textarea::new(state),
                        )
                        .when(fills, |field| field.h_full()),
                        accessible,
                    ));
                let mut element = div().relative().id(native_id(id));
                *element.style() = style.clone();
                crate::a11y::around_field(element, focused, crate::a11y::ink(cx))
                    .child(words)
                    .child(self.measure(&path, cx))
                    .into_any_element()
            }
        }
    }

    /// The engine's state moved under the writer's hands, or under a guest's
    /// ask already reported: report what differs from the last report. A
    /// text that changed is the person's input; a caret that moved alone,
    /// or a blink, is not.
    fn field_observed(&mut self, path: &AuthoredPath, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.fields.get_mut(path) else {
            return;
        };
        let Some((event, typed)) = field.settle(None, window, cx) else {
            return;
        };
        let landed = field.land(window, cx);
        if typed {
            self.activate();
        }
        for event in event.into_iter().chain(landed) {
            cx.emit(event);
        }
    }

    /// Only the keys the guest claimed are taken off the focused field.
    /// Everything else — every arrow, every Backspace, every selection —
    /// belongs to the editing engine, and taking one of those away is how
    /// an editor stops being one.
    ///
    /// The OS key is the input: it activates the view here, where the host
    /// receives it, whatever the field or the guest makes of it. Escape
    /// leaves things and activates nothing. Esc lets go of Tab for the next
    /// key, whatever else it does: the guest that claimed it still hears it,
    /// and the field and the view still see it pass (owner, 2026-09-28;
    /// AX-022). A plain Tab right after Esc is not the guest's even when it
    /// claimed Tab: the claim would swallow the very key that leaves.
    fn field_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        // A lone modifier's release reaches interceptors since gpui-pre
        // 0.3.7 (zed ba42ab9d9). It is no key here: it neither spends Esc's
        // leave nor takes it back, and no claim takes it, which would also
        // keep the release from the window's modifier listeners.
        if matches!(
            keystroke.key.as_str(),
            "shift" | "control" | "alt" | "platform" | "function"
        ) {
            return;
        }
        let mut focused = None;
        for (path, field) in self.fields.iter_mut() {
            match field.is_focused(window, cx) {
                true => focused = Some(path.clone()),
                false => field.tab_released = false,
            }
        }
        let Some(path) = focused else {
            return;
        };
        let field = self.fields.get_mut(&path).expect("the focused field");
        // A preedit is the IME's: its keys are not the guest's to claim.
        if field.composing(window, cx) {
            field.tab_released = false;
            return;
        }
        match field.key(keystroke, window, cx) {
            Pressed::Claimed(event) => {
                cx.emit(event);
                cx.stop_propagation();
            }
            Pressed::Taken => cx.stop_propagation(),
            Pressed::Passed => {}
        }
        if keystroke.key != "escape" {
            self.activate();
        }
    }

    /// A press in the box that the field itself did not take — the empty
    /// room under the last line — is still a press on the writing. It puts
    /// the caret at the end of the text, the way clicking under the words in
    /// any text box does, instead of landing nowhere.
    fn field_pressed(
        &mut self,
        path: &AuthoredPath,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate();
        let Some(Field {
            engine: Engine::Area(state),
            ..
        }) = self.fields.get(path)
        else {
            return;
        };
        let state = state.clone();
        let on_the_words = state.read(cx).input_bounds().contains(&event.position);
        state.update(cx, |state, cx| {
            if !on_the_words {
                let end = state.value().len();
                state.set_selected_range(end..end, cx);
            }
            state.focus(window, cx);
        });
    }
}

impl Field {
    pub(super) fn is_focused(&self, window: &Window, cx: &App) -> bool {
        engine!(&self.engine, |state| state
            .read(cx)
            .focus_handle(cx)
            .is_focused(window))
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        engine!(&self.engine, |state| state
            .update(cx, |state, cx| { state.focus(window, cx) }));
    }

    /// The field as its user left it, for a new generation's tree.
    pub(super) fn presentation(&self, window: &Window, cx: &App) -> InputPresentation {
        engine!(&self.engine, |state| {
            let state = state.read(cx);
            InputPresentation {
                value: state.value().to_string(),
                secure: self.secure,
                // ponytail: forward only; gpui-base 0.7.0 reads a backward
                // range as empty (`normalize_token_range`), so a backward
                // selection comes back with its caret at the far end. Save
                // `cursor()` too once upstream takes one.
                selection: state.selected_range(),
                focused: state.focus_handle(cx).is_focused(window),
            }
        })
    }

    fn composing(&self, window: &mut Window, cx: &mut App) -> bool {
        engine!(&self.engine, |state| state.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }))
    }

    /// The engine's text, selection, preedit and spans, in bytes.
    fn read(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> (
        String,
        wire::TextRange,
        Option<wire::TextRange>,
        Vec<wire::TextToken>,
    ) {
        engine!(&self.engine, |state| state.update(cx, |state, cx| {
            let text = state.value().to_string();
            let cursor = wire::TextRange::from(state.selected_range());
            let preedit = state
                .marked_text_range(window, cx)
                .map(|range| utf16_bytes(&text, range));
            let tokens = state
                .tokens()
                .iter()
                .map(|span| wire::TextToken {
                    range: span.range().into(),
                    id: span.token().id().to_string(),
                })
                .collect();
            (text, cursor, preedit, tokens)
        }))
    }

    /// Whether the field takes the box it was given or the room its own
    /// words need. Set from the node's height, because a field that always
    /// asked for all of its parent's height gave a shrinking box nothing to
    /// shrink to.
    fn set_fills(&mut self, fills: bool, cap: Option<usize>, cx: &mut App) {
        if (self.fills, self.cap) == (fills, cap) {
            return;
        }
        self.fills = fills;
        self.cap = cap;
        if let Engine::Area(state) = &self.engine {
            let rows = cap.filter(|_| !fills).unwrap_or(MAX_ROWS);
            state.update(cx, |state, cx| state.set_auto_grow(1, rows, cx));
        }
    }

    /// The guest's text and spans, as the engine's: the one place its history
    /// is cleared, since the text it held is not this one.
    fn install(
        &self,
        value: &str,
        tokens: &[wire::TextToken],
        selection: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut content = InputContent::new(value.to_owned());
        for token in tokens {
            let range = token.range.range();
            let Some(text) = value.get(range.clone()) else {
                continue;
            };
            if let Ok(next) = content
                .clone()
                .with_token(range, InlineToken::new(token.id.clone(), text.to_owned()))
            {
                content = next;
            }
        }
        engine!(&self.engine, |state| state.update(cx, |state, cx| {
            state.set_value(content, window, cx);
            state.set_selected_range(selection, cx);
        }));
    }

    /// A new generation of the guest's text: adopted, and reported as the
    /// edit it was from the text before, so an ask built against that text
    /// is carried over it. Held while an IME composes.
    fn adopt(
        &mut self,
        value: &str,
        tokens: &[wire::TextToken],
        cursor: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<wire::Event> {
        if self.composing(window, cx) {
            self.deferred
                .push(Deferred::Adopt(value.to_owned(), tokens.to_vec(), cursor));
            return None;
        }
        self.install(value, tokens, cursor, window, cx);
        self.settle(None, window, cx).and_then(|(event, _)| event)
    }

    /// The log is kept from the frame before the one shown: every ask that
    /// frame carries was built after it, and a held ask from before it.
    fn prune(&mut self) {
        let held = self
            .deferred
            .iter()
            .filter_map(|deferred| match deferred {
                Deferred::Replace(wire::WidgetCommand::Replace { revision, .. }) => Some(*revision),
                _ => None,
            })
            .min()
            .unwrap_or(u64::MAX);
        let floor = self.seen.min(held);
        self.log.retain(|(at, _)| *at > floor);
    }

    /// What differs from the last report, reported: the text at its next
    /// revision when it changed (`exact` being the edit that changed it,
    /// else the span that differs), the cursor and the preedit as they
    /// stand. `None` when nothing moved. The second answer says whether
    /// the text itself changed.
    fn settle(
        &mut self,
        exact: Option<wire::Edit>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<(Option<wire::Event>, bool)> {
        let (text, cursor, preedit, tokens) = self.read(window, cx);
        // the guest's cap is the field's: what it cannot be sent is cut
        // back, through the history, as any edit
        if text.len() > wire::MAX_FIELD_BYTES {
            let mut cut = wire::MAX_FIELD_BYTES;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            engine!(&self.engine, |state| state.update(cx, |state, cx| {
                state.set_selected_range(cut..text.len(), cx);
                state.replace("", window, cx);
            }));
            return self.settle(None, window, cx);
        }
        let changed = text != self.text;
        if !changed && cursor == self.cursor && preedit == self.preedit {
            return None;
        }
        if changed {
            self.revision += 1;
            let edit = exact.or_else(|| {
                wire::changed_span(&self.text, &text).map(|(range, text)| wire::Edit {
                    range,
                    len: text.len() as u32,
                })
            });
            if let Some(edit) = edit {
                self.log.push((self.revision, edit));
            }
            self.text.clone_from(&text);
        }
        self.cursor = cursor;
        self.preedit = preedit;
        let event = self.on_change.map(|handler| wire::Event::Text {
            handler,
            change: wire::TextChange {
                revision: self.revision,
                text,
                cursor,
                preedit,
                tokens,
            },
        });
        Some((event, changed))
    }

    /// The asks held while an IME composed land once it has committed, in
    /// the order they came.
    fn land(&mut self, window: &mut Window, cx: &mut App) -> Vec<wire::Event> {
        if self.preedit.is_some() || self.deferred.is_empty() {
            return Vec::new();
        }
        std::mem::take(&mut self.deferred)
            .into_iter()
            .filter_map(|deferred| match deferred {
                Deferred::Replace(command) => self.ask(command, window, cx),
                Deferred::Adopt(value, tokens, cursor) => {
                    self.adopt(&value, &tokens, cursor, window, cx)
                }
            })
            .collect()
    }

    /// A guest's `Replace`: carried over every edit since the revision it
    /// read, landed through the engine's history so the writer can undo it,
    /// and reported at the revision it made. One span when the guest named
    /// a token for it, as plain text when the engine refuses the span (a
    /// masked field, a range off a grapheme) or the field is at its span
    /// cap. Held while an IME composes.
    pub(super) fn ask(
        &mut self,
        command: wire::WidgetCommand,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<wire::Event> {
        if self.composing(window, cx) {
            self.deferred.push(Deferred::Replace(command));
            return None;
        }
        let wire::WidgetCommand::Replace {
            revision,
            range,
            text,
            token,
            cursor,
            ..
        } = command
        else {
            return None;
        };
        let since: Vec<wire::Edit> = self
            .log
            .iter()
            .filter(|(at, _)| *at > revision)
            .map(|(_, edit)| *edit)
            .collect();
        let (range, cursor) = wire::rebase(range, text.len(), cursor, since);
        let range = clamp(range, self.text.len());
        let edit = engine!(&self.engine, |state| state.update(cx, |state, cx| {
            state.set_selected_range(range, cx);
            let range = state.selected_range();
            let before = state.value().len();
            let span =
                token.filter(|_| !text.is_empty() && state.tokens().len() < wire::MAX_FIELD_TOKENS);
            let landed = span.is_some_and(|id| {
                state
                    .replace_range_with_token(
                        range.clone(),
                        InlineToken::new(id, text.clone()),
                        window,
                        cx,
                    )
                    .is_ok()
            });
            if !landed {
                state.replace(text.clone(), window, cx);
            }
            let len = state.value().len() + range.len() - before;
            state.set_selected_range(clamp(cursor, state.value().len()), cx);
            wire::Edit {
                range: range.into(),
                len: len as u32,
            }
        }));
        self.settle(Some(edit), window, cx)
            .and_then(|(event, _)| event)
    }

    /// A view's cursor command moves the caret where it is and takes no
    /// keys: only `Focus` does, and only past the seat's gate.
    pub(super) fn cursor_command(
        &mut self,
        command: &wire::WidgetCommand,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<wire::Event> {
        use wire::WidgetCommand as C;
        let text = &self.text;
        let at = |index: u32| {
            text.grapheme_indices(true)
                .nth(index as usize)
                .map_or(text.len(), |(offset, _)| offset)
        };
        let range = match command {
            C::CursorFront { .. } => 0..0,
            C::CursorEnd { .. } => text.len()..text.len(),
            C::Cursor { position, .. } => at(*position)..at(*position),
            C::SelectAll { .. } => 0..text.len(),
            C::Select { start, end, .. } => at(*start).min(at(*end))..at(*start).max(at(*end)),
            _ => return None,
        };
        engine!(&self.engine, |state| state
            .update(cx, |state, cx| { state.set_selected_range(range, cx) }));
        self.settle(None, window, cx).and_then(|(event, _)| event)
    }

    /// A key pressed in this focused field, no IME composing.
    fn key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut App) -> Pressed {
        let released = std::mem::replace(&mut self.tab_released, keystroke.key == "escape");
        let plain_tab = keystroke.key == "tab"
            && !keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
            && !keystroke.modifiers.function;
        let leaving = released && plain_tab;
        let key = wire::keyboard::KeyState::from(keystroke);
        if !leaving
            && let Some(handler) = self.on_key
            && self
                .claims
                .iter()
                .any(|claim| claim.matches(&key, cfg!(target_os = "macos")))
        {
            return Pressed::Claimed(wire::Event::KeyDown {
                handler,
                phase: wire::DispatchPhase::Bubble,
                event: wire::interactivity::KeyDown {
                    state: key,
                    repeat: false,
                    prefer_character_input: false,
                },
            });
        }
        if plain_tab && !released && self.indent(keystroke.modifiers.shift, window, cx) {
            return Pressed::Taken;
        }
        Pressed::Passed
    }

    /// Tab in a growing field, which the writer means as an indent and the
    /// field would otherwise spend on leaving. The editing engine has an
    /// indent of its own and will not run it here — it is switched off for
    /// a field that grows with its text, which is every guest editor — so
    /// the keystroke walks on to the window's focus ring and the caret never
    /// sees it. Type the indent instead, through the engine's history as
    /// any edit. A one-line field has no indent: Tab leaves it.
    fn indent(&mut self, outward: bool, window: &mut Window, cx: &mut App) -> bool {
        let Engine::Area(state) = &self.engine else {
            return false;
        };
        let (text, selected, writable) = {
            let state = state.read(cx);
            (
                state.value().to_string(),
                state.selected_range(),
                state.is_editable(),
            )
        };
        if !writable {
            return false;
        }
        // Shift+Tab against a line with no indent left to give has nothing
        // to do, and a key with nothing to do is the key that walks the
        // focus ring. Only an indent that actually moved is one this field
        // keeps.
        let Some((range, replacement, moved)) = indent(&text, selected, outward) else {
            return false;
        };
        state.update(cx, |state, cx| {
            state.set_selected_range(range, cx);
            state.replace(replacement, window, cx);
            state.set_selected_range(moved, cx);
        });
        true
    }
}

/// `range`, in bytes of a text `len` long: ends past the text are its end,
/// and a backward range runs forward.
fn clamp(range: wire::TextRange, len: usize) -> Range<usize> {
    let start = (range.start as usize).min(len);
    let end = (range.end as usize).min(len);
    start.min(end)..start.max(end)
}

/// A UTF-16 range of `text`, as gpui's input handler speaks, in bytes.
fn utf16_bytes(text: &str, range: Range<usize>) -> wire::TextRange {
    let at = |units: usize| {
        let mut counted = 0;
        for (byte, character) in text.char_indices() {
            if counted >= units {
                return byte;
            }
            counted += character.len_utf16();
        }
        text.len()
    };
    wire::TextRange::from(at(range.start)..at(range.end))
}

/// One indent. Two spaces, the editing engine's own tab size: what an indent
/// has to do in prose is line the next line up under this one, and a hard tab
/// lines it up against a stop no painter here draws.
const INDENT: &str = "  ";

/// The one edit Tab makes — the span of `text` it rewrites and what the
/// span reads after — and where the selection lands then. `None` when the
/// key had nothing to do.
///
/// A caret types an indent where it stands. A SELECTION moves whole lines
/// instead — that is what makes Tab worth having in a list, and replacing
/// the selected words with two spaces is a deletion nobody asked for. The
/// lines move as one edit, so one undo gives them back.
fn indent(
    text: &str,
    selected: Range<usize>,
    outward: bool,
) -> Option<(Range<usize>, String, Range<usize>)> {
    let lo = selected.start.min(selected.end);
    let hi = selected.start.max(selected.end);
    let typing = lo == hi && !outward;
    if typing {
        let at = lo + INDENT.len();
        return Some((lo..lo, INDENT.to_owned(), at..at));
    }
    // The first line is the one the selection starts ON, wherever in it that
    // is; the last is the last one it starts BEFORE, so a selection carried to
    // the head of a line leaves that line alone, as it does everywhere else.
    let head = text[..lo].rfind('\n').map_or(0, |at| at + 1);
    let mut lines = String::new();
    let mut edits: Vec<(usize, usize, usize)> = Vec::new();
    let mut at = head;
    for line in text[head..].split_inclusive('\n') {
        let touched = at == head || at < hi;
        if !touched {
            break;
        }
        match outward {
            true => {
                let shed = outdent(line);
                edits.push((at, 0, shed));
                lines.push_str(&line[shed..]);
            }
            false => {
                edits.push((at, INDENT.len(), 0));
                lines.push_str(INDENT);
                lines.push_str(line);
            }
        }
        at += line.len();
    }
    let nothing_to_shed = edits
        .iter()
        .all(|(_, added, removed)| *added + *removed == 0);
    if nothing_to_shed {
        return None;
    }
    let shifted = |offset: usize| {
        let mut moved = offset;
        for &(start, added, removed) in &edits {
            if start > offset {
                break;
            }
            moved += added;
            moved -= removed.min(offset - start);
        }
        moved
    };
    Some((
        head..at,
        lines,
        shifted(selected.start)..shifted(selected.end),
    ))
}

/// How much of a line's leading whitespace one Shift+Tab takes back: a hard
/// tab whole, or up to an indent's worth of spaces.
fn outdent(line: &str) -> usize {
    if line.starts_with('\t') {
        return 1;
    }
    line.bytes()
        .take(INDENT.len())
        .take_while(|byte| *byte == b' ')
        .count()
}

/// The text size `field_rows` assumes when the node's style sets none.
/// Note: gpui's own default is 16px (`rems(1.)`), so a field styled without
/// a size is counted at a smaller face than it is drawn with.
const FALLBACK_FIELD_TEXT_PX: f32 = 14.;

/// gpui's default line height as a multiple of the text size (`phi()`,
/// 1.618034, to the three places the literal always had).
const FIELD_LINE_HEIGHT: f32 = 1.618;

/// How many lines a field that grows with its words shows before it scrolls:
/// what its node's `max_h` holds, less its vertical padding, at the field's
/// line height.
fn field_rows(style: &gpui_kit::StyleRefinement) -> Option<usize> {
    use gpui_kit::{AbsoluteLength, DefiniteLength, Length};
    let pixels = |length: Option<DefiniteLength>| match length {
        Some(DefiniteLength::Absolute(AbsoluteLength::Pixels(pixels))) => f32::from(pixels),
        _ => 0.,
    };
    let Some(Length::Definite(max)) = style.max_size.height else {
        return None;
    };
    let pad = pixels(style.padding.top) + pixels(style.padding.bottom);
    let size = match style.text.font_size {
        Some(AbsoluteLength::Pixels(size)) => f32::from(size),
        _ => FALLBACK_FIELD_TEXT_PX,
    };
    let rows = (pixels(Some(max)) - pad) / (size * FIELD_LINE_HEIGHT);
    Some((rows.floor() as usize).max(1))
}

/// A view's `style` for its one-line field, split between the wrapper, the
/// box the parent lays out, and the kit's box drawn inside it. Where the
/// field sits (its margin, its place, its share of a flex line) is the
/// wrapper's; its size is both's, so the kit's box fills the wrapper: a
/// fraction of the parent is the wrapper's to take, and the kit's box takes
/// the whole of the wrapper, not that fraction of it again. The rest is the
/// kit's box. A wrapper left the full width would take a flex line's room
/// from a spacer and its siblings, and a press there would focus the field.
fn placed(
    style: &gpui_kit::StyleRefinement,
) -> (gpui_kit::StyleRefinement, gpui_kit::StyleRefinement) {
    let mut drawn = style.clone();
    let placed = gpui_kit::StyleRefinement {
        position: drawn.position.take(),
        inset: std::mem::take(&mut drawn.inset),
        margin: std::mem::take(&mut drawn.margin),
        align_self: drawn.align_self.take(),
        flex_grow: drawn.flex_grow.take(),
        flex_shrink: drawn.flex_shrink.take(),
        flex_basis: drawn.flex_basis.take(),
        grid_location: drawn.grid_location.take(),
        size: drawn.size.clone(),
        min_size: drawn.min_size.clone(),
        max_size: drawn.max_size.clone(),
        ..Default::default()
    };
    for size in [&mut drawn.size, &mut drawn.min_size, &mut drawn.max_size] {
        for length in [&mut size.width, &mut size.height] {
            if let Some(gpui_kit::Length::Definite(gpui_kit::DefiniteLength::Fraction(fraction))) =
                length
            {
                *fraction = 1.;
            }
        }
    }
    (placed, drawn)
}

#[cfg(test)]
mod tests {
    use super::indent;

    /// A caret types an indent; a selection moves whole lines, Shift+Tab
    /// takes them back and gives up on a line with nothing left to shed.
    #[test]
    fn tab_indents_a_caret_a_block_and_gives_it_back() {
        let after = |text: &str, selected, outward| {
            indent(text, selected, outward).map(|(range, lines, moved)| {
                let mut next = text.to_owned();
                next.replace_range(range, &lines);
                (next, moved)
            })
        };
        assert_eq!(after("ab", 1..1, false), Some(("a  b".into(), 3..3)));
        assert_eq!(
            after("one\ntwo\nthree", 1..9, false),
            Some(("  one\n  two\n  three".into(), 3..15))
        );
        // a selection carried to the head of a line leaves that line alone
        assert_eq!(
            after("one\ntwo\nthree", 0..8, false),
            Some(("  one\n  two\nthree".into(), 2..12))
        );
        assert_eq!(
            after("  one\n\ttwo\nthree", 2..12, true),
            Some(("one\ntwo\nthree".into(), 0..9))
        );
        assert_eq!(after("one", 0..3, true), None);
        assert_eq!(after("one", 1..1, true), None);
    }
}
