//! A long press in a text field on a touch-first device: the host shuts the
//! kit's drawn menu, tells the shell to show the OS's own over the selection,
//! applies the pick, and draws a growing field's handles.
use super::*;
use crate::contract::testing::{edit_menu_told_for_test as told, touch_first_for_test};
use crate::contract::{EditAction, EditMenu};
use crate::render::inputs::Engine;

const VALUE: &str = "quick select value";

fn on_the_word() -> gpui_kit::Point<gpui_kit::Pixels> {
    point(px(60.), px(12.))
}

fn line(secure: bool) -> wire::Node {
    let mut field = input("Name", secure, false);
    if let wire::Node::Field { value, cursor, .. } = &mut field {
        *value = VALUE.into();
        *cursor = wire::TextRange::caret(VALUE.len());
    }
    field
}

fn composer(value: &str) -> wire::Node {
    area(
        "composer",
        Some("Message"),
        value,
        sized_style(Some(fixed(300.)), Some(fixed(40.))),
    )
}

fn mounted_under(
    touch: bool,
    field: wire::Node,
    cx: &mut gpui_kit::TestAppContext,
) -> (Entity<ViewTree>, gpui_kit::VisualTestContext) {
    touch_first_for_test(touch);
    cx.update(gpui_kit::init);
    let root = container_with_style("root", div().size_full().style().clone(), [field]);
    let window = cx.open_window(size(px(320.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    drawn(&mut native);
    told();
    (tree, native)
}

fn drawn(native: &mut gpui_kit::VisualTestContext) {
    native.run_until_parked();
    native.update(crate::render::draw_frame);
    native.run_until_parked();
}

fn finger(
    native: &mut gpui_kit::VisualTestContext,
    phase: gpui_kit::TouchPhase,
    at: gpui_kit::Point<gpui_kit::Pixels>,
) {
    let touch = gpui_kit::TouchEvent {
        id: gpui_kit::TouchId(1),
        phase,
        position: at,
        predicted_position: None,
        force: None,
    };
    native.update(|window, cx| {
        let _ = window.dispatch_event(touch.to_platform_input(), cx);
    });
    drawn(native);
}

fn long_press(native: &mut gpui_kit::VisualTestContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
    finger(native, gpui_kit::TouchPhase::Started, at);
    let held = gpui_kit::GestureTuning::default().long_press_duration;
    native
        .executor()
        .advance_clock(held + std::time::Duration::from_millis(1));
    drawn(native);
    finger(native, gpui_kit::TouchPhase::Ended, at);
}

fn pick(native: &mut gpui_kit::VisualTestContext, action: EditAction) {
    native.cx.update(|cx| crate::render::edit(action, cx));
    drawn(native);
}

fn engine(tree: &Entity<ViewTree>, native: &mut gpui_kit::VisualTestContext) -> Engine {
    native.update(|_, cx| {
        tree.read(cx)
            .fields
            .values()
            .next()
            .expect("a field")
            .engine
            .clone()
    })
}

fn live(
    tree: &Entity<ViewTree>,
    native: &mut gpui_kit::VisualTestContext,
) -> Option<gpui_base::TouchSelectionSnapshot> {
    let engine = engine(tree, native);
    native.update(|_, cx| match &engine {
        Engine::Line(state) => state.read(cx).touch_selection(),
        Engine::Area(state) => state.read(cx).touch_selection(),
    })
}

fn shown(tree: &Entity<ViewTree>, native: &mut gpui_kit::VisualTestContext) -> (String, String) {
    native.update(|window, cx| {
        let tree = tree.read(cx);
        let field = tree.fields.values().next().expect("a field");
        let shown = field.presentation(window, cx);
        (shown.value[shown.selection.clone()].to_owned(), shown.value)
    })
}

fn clipboard(native: &mut gpui_kit::VisualTestContext) -> Option<String> {
    native.cx.read_from_clipboard().and_then(|item| item.text())
}

fn offered(menu: &[EditMenu]) -> Vec<EditAction> {
    match menu {
        [EditMenu::Show(_, offers)] => offers.clone(),
        other => panic!("one Show was told, not {other:?}"),
    }
}

#[gpui_kit::test]
fn under_touch_a_long_press_in_a_field_tells_the_os_menu_over_the_word_and_shuts_the_kits(
    cx: &mut gpui_kit::TestAppContext,
) {
    for (kind, field) in [("line", line(false)), ("area", composer(VALUE))] {
        let (tree, mut native) = mounted_under(true, field, cx);
        long_press(&mut native, on_the_word());
        assert_eq!(
            shown(&tree, &mut native).0,
            "select",
            "{kind}: the long press took the word"
        );
        let held = live(&tree, &mut native).expect("a live touch selection");
        assert!(
            !held.is_menu_open(),
            "{kind}: the kit's own menu was left open"
        );
        assert_eq!(
            told(),
            [EditMenu::Show(
                held.bounds().expect("the word is in view"),
                vec![
                    EditAction::Cut,
                    EditAction::Copy,
                    EditAction::Paste,
                    EditAction::SelectAll
                ],
            )],
            "{kind}: the OS's menu over the word, once"
        );
        drawn(&mut native);
        assert_eq!(told(), Vec::new(), "{kind}: a redraw tells nothing new");
    }
    touch_first_for_test(false);
}

#[gpui_kit::test]
fn a_secret_fields_menu_offers_no_cut_or_copy(cx: &mut gpui_kit::TestAppContext) {
    let (_tree, mut native) = mounted_under(true, line(true), cx);
    long_press(&mut native, on_the_word());
    let offers = offered(&told());
    assert!(offers.contains(&EditAction::Paste), "{offers:?}");
    let leaks = offers.contains(&EditAction::Cut) || offers.contains(&EditAction::Copy);
    assert!(!leaks, "a secret's menu offered {offers:?}");
    touch_first_for_test(false);
}

#[gpui_kit::test]
fn a_tap_elsewhere_in_the_field_takes_the_menu_down(cx: &mut gpui_kit::TestAppContext) {
    for (kind, field) in [("line", line(false)), ("area", composer(VALUE))] {
        let (tree, mut native) = mounted_under(true, field, cx);
        long_press(&mut native, on_the_word());
        assert_eq!(offered(&told()).len(), 4, "{kind}");
        native.simulate_click(point(px(30.), px(12.)), gpui_kit::Modifiers::none());
        drawn(&mut native);
        assert_eq!(
            live(&tree, &mut native),
            None,
            "{kind}: the tap left the word"
        );
        assert_eq!(told(), [EditMenu::Hide], "{kind}: the menu went down");
    }
    touch_first_for_test(false);
}

#[gpui_kit::test]
fn each_pick_lands_in_the_field_the_menu_was_shown_for(cx: &mut gpui_kit::TestAppContext) {
    for (kind, field) in [("line", line(false)), ("area", composer(VALUE))] {
        let (tree, mut native) = mounted_under(true, field, cx);
        long_press(&mut native, on_the_word());
        assert_eq!(offered(&told()).len(), 4, "{kind}");
        pick(&mut native, EditAction::Copy);
        assert_eq!(clipboard(&mut native).as_deref(), Some("select"), "{kind}");
        assert_eq!(
            shown(&tree, &mut native),
            ("select".to_owned(), VALUE.to_owned()),
            "{kind}: a copy keeps the word and its handles"
        );
        drawn(&mut native);
        assert_eq!(
            told(),
            Vec::new(),
            "{kind}: the menu a copy closed stays closed"
        );
        long_press(&mut native, on_the_word());
        assert_eq!(offered(&told()).len(), 4, "{kind}");
        pick(&mut native, EditAction::SelectAll);
        assert_eq!(shown(&tree, &mut native).0, VALUE, "{kind}");
        let all = live(&tree, &mut native).expect("select all keeps the touch selection");
        assert_eq!(
            told(),
            [EditMenu::Show(
                all.bounds().expect("in view"),
                vec![EditAction::Cut, EditAction::Copy, EditAction::Paste],
            )],
            "{kind}: the menu comes back over the whole text"
        );
        pick(&mut native, EditAction::Cut);
        assert_eq!(
            shown(&tree, &mut native).1,
            "",
            "{kind}: the cut took it all"
        );
        assert_eq!(clipboard(&mut native).as_deref(), Some(VALUE), "{kind}");
        assert_eq!(
            told(),
            Vec::new(),
            "{kind}: the menu a pick closed is not shown again"
        );
        long_press(&mut native, on_the_word());
        assert!(
            offered(&told()).contains(&EditAction::Paste),
            "{kind}: an empty field's menu offers Paste"
        );
        pick(&mut native, EditAction::Paste);
        assert_eq!(shown(&tree, &mut native).1, VALUE, "{kind}: pasted");
    }
    touch_first_for_test(false);
}

#[gpui_kit::test]
fn the_composers_end_handle_drags_the_end_of_the_selection(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted_under(true, composer(VALUE), cx);
    long_press(&mut native, on_the_word());
    assert_eq!(offered(&told()).len(), 4);
    let end = live(&tree, &mut native).expect("a touch selection").end();
    let grab = point(end.left(), end.bottom() + px(4.));
    native.simulate_mouse_move(
        grab,
        None::<gpui_kit::MouseButton>,
        gpui_kit::Modifiers::none(),
    );
    native.simulate_mouse_down(
        grab,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::none(),
    );
    drawn(&mut native);
    assert_eq!(
        told(),
        [EditMenu::Hide],
        "the menu steps aside while a handle drags"
    );
    let far = point(px(290.), grab.y);
    native.simulate_mouse_move(
        far,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::none(),
    );
    drawn(&mut native);
    native.simulate_mouse_up(
        far,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::none(),
    );
    drawn(&mut native);
    assert_eq!(shown(&tree, &mut native).0, "select value");
    let dragged = live(&tree, &mut native).expect("still a touch selection");
    assert_eq!(
        offered(&told()),
        [
            EditAction::Cut,
            EditAction::Copy,
            EditAction::Paste,
            EditAction::SelectAll
        ],
        "the menu comes back once the handle lets go"
    );
    assert!(dragged.dragging().is_none());
    touch_first_for_test(false);
}

#[gpui_kit::test]
fn a_field_that_leaves_the_frame_takes_its_menu_down(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted_under(true, composer(VALUE), cx);
    long_press(&mut native, on_the_word());
    assert_eq!(offered(&told()).len(), 4);
    native.update(|_, cx| {
        tree.update(cx, |tree, cx| tree.replace(container("root", []), &[], cx));
    });
    drawn(&mut native);
    assert_eq!(told(), [EditMenu::Hide]);
    touch_first_for_test(false);
}

#[gpui_kit::test]
fn off_touch_a_long_press_tells_the_os_nothing_and_the_kit_keeps_its_menu(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (tree, mut native) = mounted_under(false, line(false), cx);
    long_press(&mut native, on_the_word());
    assert_eq!(shown(&tree, &mut native).0, "select");
    let held = live(&tree, &mut native).expect("a live touch selection");
    assert!(held.is_menu_open(), "the kit's own menu is the desktop's");
    assert_eq!(told(), Vec::new());
}
