//! A field's half of the UIKit text contract, drawn after the kit's field so its handler is the one the window keeps:
//! length, editable range, selection writes, a rectangle per line and the keyboard hints are answered here,
//! the rest by the field's state, and a software Return on a one-line field is its Enter.

use gpui_kit::base::input::{InputBaseState, InputEvent, InputModeKind};
use gpui_kit::{
    App, AppContext as _, Bounds, ClipboardItem, Context, ElementInputHandler, Entity,
    EntityInputHandler, Focusable as _, IntoElement, Pixels, Point, SharedString, Styled as _,
    Subscription, TextInputConfiguration, UTF16Selection, Window, canvas, px, size,
};
use std::ops::Range;

pub struct TextInput<M: InputModeKind> {
    state: Entity<InputBaseState<M>>,
    configuration: TextInputConfiguration,
    told: Option<(SharedString, Range<usize>)>,
    keyed: bool,
    _keys: Subscription,
}

impl<M: InputModeKind> TextInput<M> {
    pub fn new(
        state: &Entity<InputBaseState<M>>,
        configuration: TextInputConfiguration,
        cx: &mut App,
    ) -> Entity<Self> {
        let state = state.clone();
        cx.new(|cx| {
            let this = cx.entity().downgrade();
            let focus = state.read(cx).focus_handle(cx);
            let keys = cx.intercept_keystrokes(move |event, window, cx| {
                let enter = event.keystroke.key == "enter";
                let ours = enter && focus.is_focused(window);
                let _ = this.update(cx, |this: &mut Self, _| this.keyed = ours);
            });
            Self {
                state,
                configuration,
                told: None,
                keyed: false,
                _keys: keys,
            }
        })
    }

    pub fn configure(&mut self, configuration: TextInputConfiguration) {
        self.configuration = configuration;
    }

    pub fn element(this: &Entity<Self>) -> impl IntoElement + use<M> {
        let this = this.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let focus = this.read(cx).state.read(cx).focus_handle(cx);
                window.handle_input(&focus, ElementInputHandler::new(bounds, this.clone()), cx);
                let focused = focus.is_focused(window);
                this.update(cx, |this, cx| {
                    this.keyed = false;
                    if focused {
                        this.tell(window, cx);
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    fn tell(&mut self, window: &Window, cx: &App) {
        let state = self.state.read(cx);
        let now = (state.value(), state.selected_range());
        if let Some((text, selection)) = &self.told {
            let focus = state.focus_handle(cx);
            if *text != now.0 {
                window.notify_text_input_changed(&focus);
            }
            if *selection != now.1 {
                window.notify_text_selection_changed(&focus);
            }
        }
        self.told = Some(now);
    }

    fn edit<R>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut InputBaseState<M>, &mut Window, &mut Context<InputBaseState<M>>) -> R,
    ) -> R {
        self.keyed = false;
        let answer = self.state.update(cx, |state, cx| edit(state, window, cx));
        self.tell(window, cx);
        answer
    }

    fn editable(&self, cx: &App) -> Option<Range<usize>> {
        let state = self.state.read(cx);
        state
            .is_editable()
            .then(|| 0..state.value().encode_utf16().count())
    }
}

impl<M: InputModeKind> EntityInputHandler for TextInput<M> {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.state.update(cx, |state, cx| {
            EntityInputHandler::text_for_range(state, range, adjusted, window, cx)
        })
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.state.update(cx, |state, cx| {
            EntityInputHandler::selected_text_range(state, ignore_disabled_input, window, cx)
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.state.update(cx, |state, cx| {
            EntityInputHandler::marked_text_range(state, window, cx)
        })
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit(window, cx, |state, window, cx| {
            EntityInputHandler::unmark_text(state, window, cx)
        })
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        self.edit(window, cx, |state, window, cx| {
            EntityInputHandler::paste(state, item, window, cx)
        })
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keyed = std::mem::take(&mut self.keyed);
        self.edit(window, cx, |state, window, cx| {
            let typed = keyed || !entered(state, text);
            if typed {
                EntityInputHandler::replace_text_in_range(state, range, text, window, cx);
                return;
            }
            if EntityInputHandler::marked_text_range(state, window, cx).is_some() {
                EntityInputHandler::unmark_text(state, window, cx);
            }
            cx.emit(InputEvent::PressEnter {
                secondary: false,
                shift: false,
            });
        })
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(window, cx, |state, window, cx| {
            EntityInputHandler::replace_and_mark_text_in_range(
                state,
                range,
                new_text,
                new_selected_range,
                window,
                cx,
            )
        })
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.state.update(cx, |state, cx| {
            let bounds = state.text_bounds().unwrap_or(element_bounds);
            EntityInputHandler::bounds_for_range(state, range_utf16, bounds, window, cx)
        })
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        let state = self.state.read(cx);
        rows(state, bytes(&state.value(), range_utf16))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.state.update(cx, |state, cx| {
            EntityInputHandler::character_index_for_point(state, point, window, cx)
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(window, cx, |state, _, cx| {
            let range = bytes(&state.value(), range_utf16);
            state.set_selected_range(range, cx)
        })
    }

    fn text_length_utf16(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        self.editable(cx).map(|range| range.end)
    }

    fn accepts_text_input(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.state.update(cx, |state, cx| {
            EntityInputHandler::accepts_text_input(state, window, cx)
        })
    }

    fn text_input_configuration(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> TextInputConfiguration {
        self.configuration.clone()
    }

    fn text_input_editable_range(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.editable(cx)
    }
}

fn newlines(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == '\n' || c == '\r')
}

fn entered<M: InputModeKind>(state: &InputBaseState<M>, text: &str) -> bool {
    newlines(text) && state.is_single_line() && state.is_editable()
}

pub(crate) fn bytes(text: &str, range: Range<usize>) -> Range<usize> {
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
    at(range.start)..at(range.end)
}

fn rows<M: InputModeKind>(state: &InputBaseState<M>, range: Range<usize>) -> Vec<Bounds<Pixels>> {
    let text = state.value();
    let ends = text[range.clone()]
        .char_indices()
        .map(|(at, _)| range.start + at)
        .chain([range.end]);
    let mut rows: Vec<Bounds<Pixels>> = Vec::new();
    for at in ends {
        let Some(caret) = state.range_to_bounds(&(at..at)) else {
            continue;
        };
        match rows.last_mut() {
            Some(row) if row.origin.y == caret.origin.y => {
                row.size.width = caret.origin.x - row.origin.x
            }
            _ => rows.push(Bounds::new(caret.origin, size(px(0.), caret.size.height))),
        }
    }
    rows
}

#[cfg(test)]
mod tests;
