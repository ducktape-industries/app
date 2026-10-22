use super::*;
use gpui_kit::base::input::{InputMode, Textarea, TextareaMode, TextareaState};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{ParentElement as _, Render, TestAppContext, VisualTestContext, div};
use std::{cell::Cell, rc::Rc};

struct Line {
    state: Entity<InputState>,
    keys: Entity<TextInput<InputMode>>,
}

impl Render for Line {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(Input::new(&self.state))
            .child(TextInput::element(&self.keys))
    }
}

struct Area {
    state: Entity<TextareaState>,
    keys: Entity<TextInput<TextareaMode>>,
}

impl Render for Area {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(Textarea::new(&self.state))
            .child(TextInput::element(&self.keys))
    }
}

fn line(
    value: &str,
    cx: &mut TestAppContext,
) -> (
    Entity<InputState>,
    Entity<TextInput<InputMode>>,
    VisualTestContext,
) {
    cx.update(gpui_kit::init);
    let value = value.to_owned();
    let window = cx.open_window(size(px(400.), px(100.)), move |window, cx| {
        let state = cx.new(|cx| InputState::new(window, cx).default_value(value));
        let keys = TextInput::new(&state, TextInputConfiguration::default(), cx);
        Line { state, keys }
    });
    let root = window.root(cx).unwrap();
    let mut native = VisualTestContext::from_window(window.into(), cx);
    let (state, keys) = root.read_with(&native, |root, _| (root.state.clone(), root.keys.clone()));
    native.update(|window, cx| {
        state.update(cx, |state, cx| state.focus(window, cx));
        crate::render::draw_frame(window, cx);
    });
    (state, keys, native)
}

fn area(
    value: &str,
    cx: &mut TestAppContext,
) -> (
    Entity<TextareaState>,
    Entity<TextInput<TextareaMode>>,
    VisualTestContext,
) {
    cx.update(gpui_kit::init);
    let value = value.to_owned();
    let window = cx.open_window(size(px(400.), px(200.)), move |window, cx| {
        let state = cx.new(|cx| TextareaState::new(window, cx).default_value(value));
        let keys = TextInput::new(&state, TextInputConfiguration::default(), cx);
        Area { state, keys }
    });
    let root = window.root(cx).unwrap();
    let mut native = VisualTestContext::from_window(window.into(), cx);
    let (state, keys) = root.read_with(&native, |root, _| (root.state.clone(), root.keys.clone()));
    native.update(|window, cx| {
        state.update(cx, |state, cx| state.focus(window, cx));
        crate::render::draw_frame(window, cx);
    });
    (state, keys, native)
}

fn enters<M: InputModeKind>(
    state: &Entity<InputBaseState<M>>,
    native: &mut VisualTestContext,
) -> (Rc<Cell<usize>>, gpui_kit::Subscription) {
    let entered = Rc::new(Cell::new(0));
    let seen = entered.clone();
    let heard = native.update(|_, cx| {
        cx.subscribe(state, move |_, event: &InputEvent, _| {
            if let InputEvent::PressEnter { .. } = event {
                seen.set(seen.get() + 1);
            }
        })
    });
    (entered, heard)
}

#[gpui_kit::test]
fn a_field_answers_the_uikit_contract(cx: &mut TestAppContext) {
    let (state, keys, mut native) = line("한글 ok", cx);
    native.update(|window, cx| {
        keys.update(cx, |keys, cx| {
            assert_eq!(keys.text_length_utf16(window, cx), Some(5));
            assert_eq!(keys.text_input_editable_range(window, cx), Some(0..5));
            keys.set_selected_text_range(0..2, window, cx);
        });
        assert_eq!(
            state.read(cx).selected_range(),
            0..6,
            "two syllables, six bytes"
        );
        assert_eq!(
            state.read(cx).value().to_string(),
            "한글 ok",
            "a selection write edits nothing"
        );
        keys.update(cx, |keys, cx| {
            keys.replace_text_in_range(Some(0..2), "", window, cx);
            keys.replace_text_in_range(None, "한", window, cx);
            keys.replace_text_in_range(None, "글", window, cx);
        });
        assert_eq!(
            state.read(cx).value().to_string(),
            "한글 ok",
            "the Korean keyboard's re-insert"
        );
        state.update(cx, |state, cx| state.set_readonly(true, cx));
        keys.update(cx, |keys, cx| {
            assert_eq!(
                keys.text_length_utf16(window, cx),
                None,
                "no keyboard for a read-only field"
            );
            assert_eq!(keys.text_input_editable_range(window, cx), None);
        });
        state.update(cx, |state, cx| {
            state.set_readonly(false, cx);
            state.set_disabled(true, cx);
        });
        keys.update(cx, |keys, cx| {
            assert_eq!(
                keys.text_length_utf16(window, cx),
                None,
                "nor for a disabled one"
            );
        });
    });
}

#[gpui_kit::test]
fn a_software_return_is_enter_on_one_line(cx: &mut TestAppContext) {
    let (state, keys, mut native) = line("ab", cx);
    let (entered, _heard) = enters(&state, &mut native);
    native.update(|window, cx| {
        keys.update(cx, |keys, cx| {
            keys.replace_and_mark_text_in_range(None, "に", None, window, cx);
            keys.replace_text_in_range(None, "\n", window, cx);
            assert_eq!(
                keys.marked_text_range(window, cx),
                None,
                "Return ends the composition"
            );
        })
    });
    native.run_until_parked();
    assert_eq!(entered.get(), 1);
    let value = state.read_with(&native, |state, _| state.value().to_string());
    assert!(value.contains('に'), "{value:?}");
    assert!(!value.contains('\n'), "{value:?}");
    native.simulate_input("\n");
    native.run_until_parked();
    assert_eq!(
        entered.get(),
        2,
        "the window's handler for the field is this one"
    );
    let after = state.read_with(&native, |state, _| state.value().to_string());
    assert_eq!(after, value, "and Return typed nothing");
}

#[gpui_kit::test]
fn a_software_return_straight_after_a_hardware_enter_is_still_enter(cx: &mut TestAppContext) {
    let (state, keys, mut native) = line("ab", cx);
    let (entered, _heard) = enters(&state, &mut native);
    let on_the_field = state.read_with(&native, |state, _| state.input_bounds().center());
    native.simulate_click(on_the_field, gpui_kit::Modifiers::none());
    native.run_until_parked();
    native.update(|window, cx| {
        window.dispatch_keystroke(gpui_kit::Keystroke::parse("enter").unwrap(), cx);
        keys.update(cx, |keys, cx| {
            keys.replace_text_in_range(None, "\n", window, cx)
        });
    });
    native.run_until_parked();
    assert_eq!(
        entered.get(),
        2,
        "the key's Enter once, and the software Return with no frame between"
    );
    let value = state.read_with(&native, |state, _| state.value().to_string());
    assert_eq!(value, "ab");
}

#[gpui_kit::test]
fn a_software_return_is_a_newline_on_many(cx: &mut TestAppContext) {
    let (state, keys, mut native) = area("ab", cx);
    let (entered, _heard) = enters(&state, &mut native);
    native.update(|window, cx| {
        keys.update(cx, |keys, cx| {
            keys.replace_text_in_range(None, "\n", window, cx)
        })
    });
    native.run_until_parked();
    assert_eq!(entered.get(), 0);
    let value = state.read_with(&native, |state, _| state.value().to_string());
    assert_eq!(value.matches('\n').count(), 1, "{value:?}");
}

#[gpui_kit::test]
fn a_selection_reads_one_rectangle_per_line(cx: &mut TestAppContext) {
    let (_state, keys, mut native) = area("ab\ncd", cx);
    let (across, within) = native.update(|window, cx| {
        keys.update(cx, |keys, cx| {
            let bounds = Bounds::default();
            (
                keys.selection_bounds_for_range(1..4, bounds, window, cx),
                keys.selection_bounds_for_range(0..2, bounds, window, cx),
            )
        })
    });
    assert_eq!(across.len(), 2, "{across:?}");
    assert!(across[0].origin.y < across[1].origin.y, "{across:?}");
    assert!(
        across.iter().all(|row| row.size.width > px(0.)),
        "{across:?}"
    );
    assert_eq!(within.len(), 1, "{within:?}");
    assert!(within[0].size.width > px(0.), "{within:?}");
}

type Shown = (SharedString, Range<usize>);

fn told_and_now(
    keys: &Entity<TextInput<InputMode>>,
    state: &Entity<InputState>,
    native: &mut VisualTestContext,
) -> (Option<Shown>, Shown) {
    native.update(|_, cx| {
        let state = state.read(cx);
        (
            keys.read(cx).told.clone(),
            (state.value(), state.selected_range()),
        )
    })
}

#[gpui_kit::test]
fn a_keyboard_edit_is_told_in_its_call_and_an_app_edit_by_the_frame(cx: &mut TestAppContext) {
    let (state, keys, mut native) = line("ab", cx);
    native.update(|window, cx| {
        keys.update(cx, |keys, cx| {
            keys.replace_text_in_range(None, "x", window, cx)
        })
    });
    let (told, now) = told_and_now(&keys, &state, &mut native);
    assert_eq!(
        told,
        Some(now),
        "the keyboard's edit is told inside its own call"
    );
    native.update(|window, cx| state.update(cx, |state, cx| state.set_value("app", window, cx)));
    native.update(crate::render::draw_frame);
    let (told, now) = told_and_now(&keys, &state, &mut native);
    assert_eq!(told, Some(now), "and the frame tells it");
}
