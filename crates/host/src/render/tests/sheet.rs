//! The Sheet node: a bottom sheet under a Compact body, a centred modal at
//! a wider one; the view decides whether it is open.
use super::*;

const DISMISS: u32 = 7;

fn page(open: bool) -> wire::Node {
    let mut children = vec![container("base", [button("open", "Actions")])];
    if open {
        children.push(wire::Node::Sheet {
            id: named_id("actions"),
            label: Some("Message actions".into()),
            style: crate::render::plain_style(),
            detents: vec![0.5, 1.],
            on_dismiss: Some(DISMISS),
            child: Box::new(container("rows", [button("copy", "Copy text")])),
        });
    }
    sized(
        "root",
        container("page", children),
        Some(fill()),
        Some(fill()),
    )
}

fn open(
    cx: &mut gpui_kit::TestAppContext,
    width: f32,
    height: f32,
) -> (
    Entity<ViewTree>,
    gpui_kit::VisualTestContext,
    Rc<RefCell<Vec<wire::Event>>>,
    Subscription,
) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(width), px(height)), |_, _| {
        ViewTree::new(page(true)).with_keys_grant(true)
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    (tree, native, events, subscription)
}

fn dismissals(events: &RefCell<Vec<wire::Event>>) -> usize {
    events
        .borrow()
        .iter()
        .filter(|event| matches!(event, wire::Event::Message(DISMISS)))
        .count()
}

fn rest(tree: &Entity<ViewTree>, native: &gpui_kit::VisualTestContext) -> Option<usize> {
    tree.read_with(native, |tree, _| {
        tree.sheets.values().next().map(|state| state.rest())
    })
}

fn show(tree: &Entity<ViewTree>, native: &mut gpui_kit::VisualTestContext, open: bool) {
    tree.update(native, |tree, cx| tree.replace(page(open), &[], cx));
    native.update(crate::render::draw_frame);
}

fn press(native: &mut gpui_kit::VisualTestContext, x: f32, y: f32) {
    let at = point(px(x), px(y));
    native.simulate_mouse_move(at, None, Default::default());
    native.simulate_mouse_down(at, MouseButton::Left, Default::default());
    native.simulate_mouse_up(at, MouseButton::Left, Default::default());
}

fn drag(native: &mut gpui_kit::VisualTestContext, x: f32, from: f32, to: f32) {
    let (from, to) = (point(px(x), px(from)), point(px(x), px(to)));
    native.simulate_mouse_move(from, None, Default::default());
    native.simulate_mouse_down(from, MouseButton::Left, Default::default());
    native.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
    native.simulate_mouse_up(to, MouseButton::Left, Default::default());
    native.update(crate::render::draw_frame);
}

#[gpui_kit::test]
fn a_sheet_closed_by_its_scrim_opens_again(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native, events, _subscription) = open(cx, 390., 844.);
    assert_eq!(rest(&tree, &native), Some(0), "opened at the first detent");
    press(&mut native, 195., 600.);
    assert_eq!(
        dismissals(&events),
        0,
        "a press on the panel is the panel's"
    );
    press(&mut native, 195., 100.);
    assert_eq!(
        dismissals(&events),
        1,
        "the scrim asks the view to close it"
    );
    show(&tree, &mut native, false);
    assert_eq!(rest(&tree, &native), None, "the view closed it");
    show(&tree, &mut native, true);
    assert_eq!(
        rest(&tree, &native),
        Some(0),
        "open again, at its first detent"
    );
    press(&mut native, 195., 100.);
    assert_eq!(dismissals(&events), 2, "and its scrim works again");
}

#[gpui_kit::test]
fn the_handle_drags_between_detents_and_down_to_dismiss(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native, events, _subscription) = open(cx, 390., 844.);
    drag(&mut native, 195., 432., 40.);
    assert_eq!(
        rest(&tree, &native),
        Some(1),
        "dragged up to the second detent"
    );
    assert_eq!(dismissals(&events), 0);
    drag(&mut native, 195., 10., 500.);
    assert_eq!(
        rest(&tree, &native),
        Some(0),
        "short of the bottom: back at the first"
    );
    assert_eq!(dismissals(&events), 0);
    drag(&mut native, 195., 432., 800.);
    assert_eq!(
        dismissals(&events),
        1,
        "let go near the bottom: the view is asked to close it"
    );
    assert_eq!(rest(&tree, &native), Some(0));
}

#[gpui_kit::test]
fn a_wide_body_centres_the_sheet(cx: &mut gpui_kit::TestAppContext) {
    let (_tree, mut native, events, _subscription) = open(cx, 1000., 700.);
    press(&mut native, 500., 350.);
    assert_eq!(dismissals(&events), 0, "the panel is in the middle");
    press(&mut native, 20., 350.);
    assert_eq!(dismissals(&events), 1, "beside it is the scrim");
}

#[gpui_kit::test]
fn escape_asks_to_close_the_sheet(cx: &mut gpui_kit::TestAppContext) {
    let (_tree, mut native, events, _subscription) = open(cx, 390., 844.);
    native.run_until_parked();
    native.simulate_keystrokes("escape");
    assert_eq!(dismissals(&events), 1);
}

#[gpui_kit::test]
fn escape_closes_the_sheet_under_touch_too(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    let (_tree, mut native, events, _subscription) = open(cx, 390., 844.);
    native.run_until_parked();
    native.simulate_keystrokes("escape");
    crate::contract::testing::touch_first_for_test(false);
    assert_eq!(dismissals(&events), 1);
}

#[gpui_kit::test]
fn a_centred_sheet_under_touch_keeps_the_handle_band(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    let (tree, mut native, _events, _subscription) = open(cx, 1000., 700.);
    let below_centre = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, _| {
            let rows = window.find("rows").bounds();
            f32::from(rows.top() - (px(700.) - rows.size.height) / 2.)
        })
    };
    let band = crate::render::sheet::HANDLE_BAND;
    assert!(
        (below_centre(&mut native) - band / 2.).abs() < 1.,
        "the band is over the rows"
    );
    crate::contract::testing::touch_first_for_test(false);
    show(&tree, &mut native, true);
    assert!(
        below_centre(&mut native).abs() < 1.,
        "a desktop sheet is the view's own"
    );
}

fn fitted(rows: f32) -> wire::Node {
    let sheet = wire::Node::Sheet {
        id: named_id("actions"),
        label: Some("Agent".into()),
        style: crate::render::plain_style(),
        detents: Vec::new(),
        on_dismiss: Some(DISMISS),
        child: Box::new(sized(
            "rows",
            button("rename", "Rename"),
            Some(fill()),
            Some(fixed(rows)),
        )),
    };
    sized(
        "root",
        container(
            "page",
            [container("base", [button("open", "Agents")]), sheet],
        ),
        Some(fill()),
        Some(fill()),
    )
}

#[gpui_kit::test]
fn a_tap_on_a_fitted_handle_never_dismisses(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(390.), px(844.)), |_, _| {
        ViewTree::new(fitted(300.)).with_keys_grant(true)
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    let band = crate::render::sheet::HANDLE_BAND;
    press(&mut native, 195., 844. - 300. - band / 2.);
    assert_eq!(dismissals(&events), 0);
}

#[gpui_kit::test]
fn a_sheet_with_no_detents_is_as_tall_as_its_child(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(390.), px(844.)), |_, _| {
        ViewTree::new(fitted(300.)).with_keys_grant(true)
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    native.update(crate::render::draw_frame);
    let top = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, _| f32::from(window.find("rows").bounds().top()))
    };
    let band = crate::render::sheet::HANDLE_BAND;
    assert!((top(&mut native) - (844. - 300.)).abs() < 1., "fitted");
    let handle = 844. - 300. - band / 2.;
    drag(&mut native, 195., handle, handle + 60.);
    native.update(crate::render::draw_frame);
    assert!(
        (top(&mut native) - (844. - 300.)).abs() < 1.,
        "a short drag snaps back to the fitted height"
    );
    assert_eq!(dismissals(&events), 0);
    drag(&mut native, 195., handle, 830.);
    assert_eq!(dismissals(&events), 1, "near the bottom it asks to close");
    tree.update(&mut native, |tree, cx| tree.replace(fitted(2000.), &[], cx));
    native.update(crate::render::draw_frame);
    let capped = top(&mut native);
    assert!(
        (capped - (1. + band)).abs() < 1.,
        "capped at the body: {capped}"
    );
}

#[gpui_kit::test]
fn under_touch_a_sheet_slides_up_and_down_and_then_stays_still(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    let (tree, mut native, events, _subscription) = open(cx, 390., 844.);
    let rows = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, _| {
            window
                .try_find("rows")
                .map(|rows| f32::from(rows.bounds().top()))
        })
    };
    let frames = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| window.simulate_next_frame(cx))
    };
    let coming = rows(&mut native).expect("drawn");
    assert!(frames(&mut native) > 0, "a sheet coming asked for no frame");
    native
        .executor()
        .advance_clock(crate::motion::idiom().sheet.duration);
    native.update(crate::render::draw_frame);
    let up = rows(&mut native).expect("drawn");
    assert!(coming > up + 100., "it did not come up: {coming} -> {up}");
    frames(&mut native);
    assert_eq!(frames(&mut native), 0, "a sheet at rest asked for a frame");

    show(&tree, &mut native, false);
    let going = rows(&mut native).expect("a sheet going is drawn");
    assert!(frames(&mut native) > 0, "a sheet going asked for no frame");
    press(&mut native, 195., 100.);
    assert_eq!(dismissals(&events), 0, "a sheet going took a press");
    native
        .executor()
        .advance_clock(crate::motion::idiom().sheet.duration);
    native.update(crate::render::draw_frame);
    assert_eq!(rows(&mut native), None, "a sheet gone is still drawn");
    assert!(going >= up, "it went up as it went");
    frames(&mut native);
    assert_eq!(frames(&mut native), 0, "a frame asked for after it went");

    crate::contract::testing::touch_first_for_test(false);
    show(&tree, &mut native, true);
    assert_eq!(
        rows(&mut native),
        Some(up),
        "the touch sheet landed elsewhere"
    );
    assert_eq!(frames(&mut native), 0, "a pointer's sheet moved");
}

#[gpui_kit::test]
fn under_touch_a_release_settles_at_its_detent(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    let (tree, mut native, _events, _subscription) = open(cx, 390., 844.);
    native
        .executor()
        .advance_clock(crate::motion::idiom().sheet.duration);
    native.update(crate::render::draw_frame);
    let top = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, _| f32::from(window.find("rows").bounds().top()))
    };
    let half = top(&mut native);
    drag(&mut native, 195., 432., 150.);
    assert_eq!(rest(&tree, &native), Some(1));
    let let_go = top(&mut native);
    native
        .executor()
        .advance_clock(crate::motion::idiom().sheet.duration);
    native.update(crate::render::draw_frame);
    let settled = top(&mut native);
    crate::contract::testing::touch_first_for_test(false);
    assert!(
        settled < let_go && let_go < half,
        "{half} {let_go} {settled}"
    );
    show(&tree, &mut native, true);
    assert_eq!(top(&mut native), settled, "it settled off its detent");
}

#[gpui_kit::test]
fn under_touch_a_sheet_turns_back_from_where_it_is(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    let (tree, mut native, _events, _subscription) = open(cx, 390., 844.);
    let rows = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            f32::from(window.find("rows").bounds().top())
        })
    };
    let bottom = rows(&mut native);
    native
        .executor()
        .advance_clock(crate::motion::idiom().sheet.duration / 8);
    let coming = rows(&mut native);
    assert!(coming < bottom, "it did not start up: {bottom} -> {coming}");

    show(&tree, &mut native, false);
    let going = rows(&mut native);
    assert!(
        (going - coming).abs() < 1.,
        "it jumped as it went: {coming} -> {going}"
    );
    native
        .executor()
        .advance_clock(crate::motion::idiom().sheet.duration / 8);
    let lower = rows(&mut native);
    assert!(lower > going, "it did not go down: {going} -> {lower}");

    show(&tree, &mut native, true);
    let again = rows(&mut native);
    crate::contract::testing::touch_first_for_test(false);
    assert!(
        (again - lower).abs() < 1.,
        "opened again from elsewhere: {lower} -> {again}"
    );
}
