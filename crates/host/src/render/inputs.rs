//! `Node::Field`: every text field a view draws, one line or many, held by
//! the kit's editing engine. The engine owns the text: every key, selection,
//! IME preedit and undo is its own work, and the view hears each change as
//! `Event::Text` at the host's `revision`. The view asks for an edit with
//! `WidgetCommand::Replace` against the revision it has seen, and a stale
//! ask is carried over what was typed since (`wire::rebase`), never
//! dropped, so no key waits on the view and nothing typed is lost. The
//! view's own text is adopted only on a fresh mount or when its
//! `generation` moves; every other frame leaves the engine's text alone.

use super::*;
use crate::render::native_id;
use crate::text_input::TextInput;
use gpui_kit::base::input::{InlineToken, InputContent, Textarea, TextareaState};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{Edges, EntityInputHandler as _, Keystroke};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone)]
pub(super) enum Engine {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

macro_rules! engine {
    ($engine:expr, |$state:ident| $body:expr) => {
        match $engine {
            Engine::Line($state) => $body,
            Engine::Area($state) => $body,
        }
    };
}

#[derive(Clone)]
pub(super) enum Contract {
    Line(Entity<TextInput<gpui_kit::base::input::InputMode>>),
    Area(Entity<TextInput<gpui_kit::base::input::TextareaMode>>),
}

impl Contract {
    fn configure(&self, configuration: gpui_kit::TextInputConfiguration, cx: &mut App) {
        match self {
            Contract::Line(keys) => keys.update(cx, |keys, _| keys.configure(configuration)),
            Contract::Area(keys) => keys.update(cx, |keys, _| keys.configure(configuration)),
        }
    }

    fn element(&self) -> AnyElement {
        match self {
            Contract::Line(keys) => TextInput::element(keys).into_any_element(),
            Contract::Area(keys) => TextInput::element(keys).into_any_element(),
        }
    }
}

enum Deferred {
    Replace(wire::WidgetCommand),
    Adopt(u64, String, Vec<wire::TextToken>, Range<usize>),
}

enum Made {
    Observed,
    Asked(wire::Edit),
    Adopted,
}

pub(super) struct Field {
    pub(super) engine: Engine,
    pub(super) contract: Contract,
    on_change: Option<u32>,
    on_key: Option<u32>,
    on_submit: Option<u32>,
    claims: Box<[wire::KeyClaim]>,
    generation: u64,
    text: String,
    cursor: wire::TextRange,
    preedit: Option<wire::TextRange>,
    revision: u64,
    log: Vec<(u64, wire::Edit)>,
    deferred: Vec<Deferred>,
    tab_released: bool,
    placeholder: String,
    secure: bool,
    text_input: wire::TextInput,
    fills: bool,
    cap: Option<usize>,
    menu_wanted: std::rc::Rc<std::cell::Cell<bool>>,
    _observation: Subscription,
    _events: Subscription,
    _keyboard: Option<[Subscription; 2]>,
}

enum Pressed {
    Claimed(wire::Event),
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
        let style = self.style(*style);
        let path = self.authored_path.clone();
        debug_assert_eq!(path.last(), Some(id));
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
            let configuration = options.text_input.configuration(*secure);
            let engine = match multiline {
                false => Engine::Line(cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder.clone())
                        .masked(*secure)
                })),
                true => Engine::Area(cx.new(|cx| {
                    let mut state = TextareaState::new(window, cx)
                        .placeholder(placeholder.clone())
                        .auto_grow(1, usize::MAX)
                        .soft_wrap(true)
                        .searchable(false)
                        .context_menu(false);
                    state.set_editor_paddings(Edges::all(px(0.)));
                    state
                })),
            };
            let contract = match &engine {
                Engine::Line(state) => Contract::Line(TextInput::new(state, configuration, cx)),
                Engine::Area(state) => Contract::Area(TextInput::new(state, configuration, cx)),
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
            let keyboard = crate::contract::os()
                .facts()
                .touch_first
                .then(|| keyboard::follow(&engine, path.clone(), window, cx));
            let mut field = Field {
                engine,
                contract,
                on_change: *on_change,
                on_key: *on_key,
                on_submit: *on_submit,
                claims: claims.clone(),
                generation: *generation,
                text: String::new(),
                cursor: Default::default(),
                preedit: None,
                revision: *revision,
                log: Vec::new(),
                deferred: Vec::new(),
                tab_released: false,
                placeholder: placeholder.clone(),
                secure: *secure,
                text_input: options.text_input,
                fills: true,
                cap: None,
                menu_wanted: Default::default(),
                _observation: observation,
                _events: events,
                _keyboard: keyboard,
            };
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
        let mut events = Vec::new();
        let awaited = field
            .deferred
            .iter()
            .rev()
            .find_map(|deferred| match deferred {
                Deferred::Adopt(generation, ..) => Some(*generation),
                _ => None,
            })
            .unwrap_or(field.generation);
        if awaited != *generation {
            events.extend(field.adopt(*generation, value, tokens, cursor.range(), window, cx));
            events.extend(field.land(window, cx));
        }
        if field.placeholder != *placeholder {
            field.placeholder.clone_from(placeholder);
            engine!(&field.engine, |state| state.update(cx, |state, cx| {
                state.set_placeholder(placeholder.clone(), window, cx)
            }));
        }
        let asked = field.keyboard();
        let reconfigured = field.text_input != options.text_input || field.secure != *secure;
        if field.secure != *secure {
            field.secure = *secure;
            if let Engine::Line(state) = &field.engine {
                state.update(cx, |state, cx| state.set_masked(*secure, window, cx));
            }
        }
        if reconfigured {
            field.text_input = options.text_input;
            let configuration = field.configuration();
            field.contract.configure(configuration, cx);
            let raised = field.keyboard() != asked
                && crate::contract::os().facts().touch_first
                && field.is_focused(window, cx);
            if raised {
                keyboard::raise(field, window);
            }
        }
        if let Engine::Area(state) = field.engine.clone() {
            field.set_fills(style.size.height.is_some(), field_rows(&style), cx);
            let selection = cx.theme().selection;
            state.update(cx, |state, _| {
                state.set_editor_style(gpui_base::input::InputEditorStyle {
                    selection,
                    ..Default::default()
                })
            });
            if state.read(cx).is_editable() != editable {
                state.update(cx, |state, cx| state.set_readonly(!editable, cx));
            }
        }
        let engine = field.engine.clone();
        let contract = field.contract.clone();
        let fills = field.fills;
        let menu_layer = crate::contract::os()
            .facts()
            .touch_first
            .then(|| edit_menu::layer(&engine, &field.menu_wanted));
        let accessible = Accessible {
            value: (!secure).then(|| field.text.clone()),
            ..accessible(node)
        };
        for event in events {
            cx.emit(event);
        }
        let focus = engine!(&engine, |state| state.read(cx).focus_handle(cx));
        let focused = focus.is_focused(window) && !options.disabled;
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
        let touched = {
            let path = path.clone();
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                this.field_touched(&path, event, window, cx)
            })
        };
        match &engine {
            Engine::Line(state) => {
                let (placed, drawn) = placed(&style);
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
                .child(contract.element())
                .children(menu_layer)
                .capture_any_mouse_down(touched)
                .refine_style(&placed);
                announce(field, accessible).into_any_element()
            }
            Engine::Area(state) => {
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
                        crate::a11y::text_field(
                            native_id(id),
                            &focus,
                            set_value,
                            Textarea::new(state),
                        )
                        .child(contract.element())
                        .children(menu_layer)
                        .when(fills, |field| field.h_full())
                        .capture_any_mouse_down(touched),
                        accessible,
                    ));
                let mut element = div().relative().id(host_id("field"));
                *element.style() = (*style).clone();
                let element =
                    crate::a11y::around_field(element, focused, crate::a11y::ink(cx)).child(words);
                #[cfg(any(test, feature = "test-support"))]
                let element = {
                    use gpui_kit::test::TestSupportExt as _;
                    element.test_support()
                };
                element.into_any_element()
            }
        }
    }

    fn field_observed(&mut self, path: &AuthoredPath, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.fields.get_mut(path) else {
            return;
        };
        if crate::contract::os().facts().touch_first {
            edit_menu::take_over(&field.engine, &field.menu_wanted, cx);
        }
        let Some((event, typed)) = field.settle(Made::Observed, window, cx) else {
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

    fn field_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
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

    fn field_touched(
        &mut self,
        path: &AuthoredPath,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.fields.get(path) else {
            return;
        };
        let pressed_again_while_it_holds_the_keys = event.button == MouseButton::Left
            && crate::contract::os().facts().touch_first
            && field.is_focused(window, cx);
        if pressed_again_while_it_holds_the_keys {
            keyboard::raise(field, window);
        }
    }
}

impl Field {
    fn configuration(&self) -> gpui_kit::TextInputConfiguration {
        self.text_input.configuration(self.secure)
    }

    fn keyboard(&self) -> crate::contract::Keyboard {
        crate::contract::Keyboard {
            kind: self.text_input.virtual_keyboard,
            secure: self.secure,
        }
    }

    pub(super) fn left(&self, cx: &mut App) {
        edit_menu::left(engine!(&self.engine, |state| state.entity_id()), cx);
    }

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

    pub(super) fn presentation(&self, window: &Window, cx: &App) -> InputPresentation {
        engine!(&self.engine, |state| {
            let state = state.read(cx);
            InputPresentation {
                value: state.value().to_string(),
                secure: self.secure,
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
                .map(|range| wire::TextRange::from(crate::text_input::bytes(&text, range)));
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

    fn set_fills(&mut self, fills: bool, cap: Option<usize>, cx: &mut App) {
        if (self.fills, self.cap) == (fills, cap) {
            return;
        }
        self.fills = fills;
        self.cap = cap;
        if let Engine::Area(state) = &self.engine {
            let rows = cap.filter(|_| !fills).unwrap_or(usize::MAX);
            state.update(cx, |state, cx| state.set_auto_grow(1, rows, cx));
        }
    }

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

    fn adopt(
        &mut self,
        generation: u64,
        value: &str,
        tokens: &[wire::TextToken],
        cursor: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<wire::Event> {
        if self.composing(window, cx) {
            self.deferred.push(Deferred::Adopt(
                generation,
                value.to_owned(),
                tokens.to_vec(),
                cursor,
            ));
            return None;
        }
        self.generation = generation;
        self.install(value, tokens, cursor, window, cx);
        self.log.clear();
        self.settle(Made::Adopted, window, cx)
            .and_then(|(event, _)| event)
    }

    pub(super) fn frame(&mut self, revision: u64, queued: Option<u64>) {
        let held = self.deferred.iter().filter_map(|deferred| match deferred {
            Deferred::Replace(wire::WidgetCommand::Replace { revision, .. }) => Some(*revision),
            _ => None,
        });
        let floor = held.chain(queued).fold(revision, u64::min);
        self.log.retain(|(at, _)| *at > floor);
    }

    fn settle(
        &mut self,
        made: Made,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<(Option<wire::Event>, bool)> {
        let (text, cursor, preedit, tokens) = self.read(window, cx);
        let changed = text != self.text;
        if !changed && cursor == self.cursor && preedit == self.preedit {
            return None;
        }
        let mut edit = None;
        if changed {
            self.revision += 1;
            edit = match made {
                Made::Asked(edit) => Some(edit),
                Made::Observed => {
                    wire::changed_span(&self.text, &text).map(|(range, text)| wire::Edit {
                        range,
                        len: text.len() as u32,
                    })
                }
                Made::Adopted => None,
            };
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
                generation: self.generation,
                revision: self.revision,
                edit,
                text,
                cursor,
                preedit,
                tokens,
            },
        });
        Some((event, changed))
    }

    fn land(&mut self, window: &mut Window, cx: &mut App) -> Vec<wire::Event> {
        if self.preedit.is_some() || self.deferred.is_empty() {
            return Vec::new();
        }
        std::mem::take(&mut self.deferred)
            .into_iter()
            .filter_map(|deferred| match deferred {
                Deferred::Replace(command) => self.ask(command, window, cx),
                Deferred::Adopt(generation, value, tokens, cursor) => {
                    self.adopt(generation, &value, &tokens, cursor, window, cx)
                }
            })
            .collect()
    }

    pub(super) fn ask(
        &mut self,
        command: wire::WidgetCommand,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<wire::Event> {
        let wire::WidgetCommand::Replace { generation, .. } = &command else {
            return None;
        };
        match generation.cmp(&self.generation) {
            std::cmp::Ordering::Less => return None,
            std::cmp::Ordering::Greater => {
                self.deferred.push(Deferred::Replace(command));
                return None;
            }
            std::cmp::Ordering::Equal => {}
        }
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
        let rebased = wire::rebase(range, text.len(), cursor, since);
        let range = clamp(rebased.range, self.text.len());
        let replacement = rebased.replacement(&self.text, &text);
        let edit = engine!(&self.engine, |state| state.update(cx, |state, cx| {
            state.set_selected_range(range, cx);
            let range = state.selected_range();
            let before = state.value().len();
            let span = token.filter(|_| replacement == text && !text.is_empty());
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
                state.replace(replacement, window, cx);
            }
            let len = state.value().len() + range.len() - before;
            state.set_selected_range(clamp(rebased.cursor, state.value().len()), cx);
            wire::Edit {
                range: range.into(),
                len: len as u32,
            }
        }));
        self.settle(Made::Asked(edit), window, cx)
            .and_then(|(event, _)| event)
    }

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
        self.settle(Made::Observed, window, cx)
            .and_then(|(event, _)| event)
    }

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
                .any(|claim| claim.matches(&key, crate::contract::os().facts().mac_modifiers))
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

fn clamp(range: wire::TextRange, len: usize) -> Range<usize> {
    let start = (range.start as usize).min(len);
    let end = (range.end as usize).min(len);
    start.min(end)..start.max(end)
}

const INDENT: &str = "  ";

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

fn outdent(line: &str) -> usize {
    if line.starts_with('\t') {
        return 1;
    }
    line.bytes()
        .take(INDENT.len())
        .take_while(|byte| *byte == b' ')
        .count()
}

const FALLBACK_FIELD_TEXT_PX: f32 = 14.;

const FIELD_LINE_HEIGHT: f32 = 1.618;

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

pub(super) mod edit_menu;

mod keyboard {
    use super::*;
    use crate::contract::SoftKeyboard;
    use gpui_kit::EntityId;
    use std::cell::Cell;

    thread_local! {
        static UP_FOR: Cell<Option<EntityId>> = const { Cell::new(None) };
    }

    fn shown(field: EntityId) {
        UP_FOR.set(Some(field));
    }

    fn taken_from(field: EntityId) -> bool {
        let still = UP_FOR.get() == Some(field);
        if still {
            UP_FOR.set(None);
        }
        still
    }

    pub(super) fn raise(field: &Field, window: &Window) {
        shown(engine!(&field.engine, |state| state.entity_id()));
        crate::contract::os().soft_keyboard(window, SoftKeyboard::Show(field.keyboard()));
    }

    pub(super) fn follow(
        engine: &Engine,
        path: AuthoredPath,
        window: &mut Window,
        cx: &mut Context<ViewTree>,
    ) -> [Subscription; 2] {
        let focus = engine!(engine, |state| state.read(cx).focus_handle(cx));
        let field = engine!(engine, |state| state.entity_id());
        let up = cx.on_focus(&focus, window, move |tree, window, _| {
            if let Some(focused) = tree.fields.get(&path) {
                raise(focused, window);
            }
        });
        let down = cx.on_blur(&focus, window, move |_, window, cx| {
            window.defer(cx, move |window, _| {
                if taken_from(field) {
                    crate::contract::os().soft_keyboard(window, SoftKeyboard::Hide);
                }
            });
        });
        [up, down]
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_keyboard_goes_down_only_for_the_field_it_is_up_for() {
            let (a, b) = (EntityId::from(1u64), EntityId::from(2u64));
            shown(a);
            assert!(taken_from(a), "a field let the keys go");
            assert!(!taken_from(a), "and the keyboard is no one's");
            shown(a);
            shown(b);
            assert!(!taken_from(a), "the keys moved to b: a's blur leaves it up");
            assert!(taken_from(b));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::indent;

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
