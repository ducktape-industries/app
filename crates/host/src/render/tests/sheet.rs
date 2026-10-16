//! The Sheet node: a bottom sheet under a Compact body, a centred modal at
//! a wider one; the view decides whether it is open.
use super::*;

const DISMISS: u32 = 7;

/// A page with, when `open`, a sheet over it opened at half the body and
/// dragged up to all of it.
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

/// The tree in a `width` x `height` window, its events, and the window.
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
    native.update(|window, cx| window.render_frame(cx));
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
    native.update(|window, cx| window.render_frame(cx));
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
    native.update(|window, cx| window.render_frame(cx));
}

/// The UX QA's first finding on both phones: a sheet closed by its scrim
/// would not open again. Here the scrim asks the view to close it, the
/// view stops drawing it, and drawn again it opens at its first detent and
/// takes the scrim's press as before. A press on the panel is the panel's.
#[gpui_kit::test]
fn a_sheet_closed_by_its_scrim_opens_again(cx: &mut gpui_kit::TestAppContext) {
    // 390 x 844: a phone, Compact; the panel is the bottom half
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

/// The handle drags the panel between its detents; let go nearer the
/// bottom than the first detent, the sheet asks to be closed and rests at
/// its first detent until the view's next frame drops it.
#[gpui_kit::test]
fn the_handle_drags_between_detents_and_down_to_dismiss(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native, events, _subscription) = open(cx, 390., 844.);
    // the panel's top edge is at half the body; its handle is the band below it
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

/// At a wide body the sheet is a centred modal over the same scrim: a
/// press beside the panel closes it, one in it does not.
#[gpui_kit::test]
fn a_wide_body_centres_the_sheet(cx: &mut gpui_kit::TestAppContext) {
    let (_tree, mut native, events, _subscription) = open(cx, 1000., 700.);
    press(&mut native, 500., 350.);
    assert_eq!(dismissals(&events), 0, "the panel is in the middle");
    press(&mut native, 20., 350.);
    assert_eq!(dismissals(&events), 1, "beside it is the scrim");
}

/// Escape closes the sheet the keys are in.
#[gpui_kit::test]
fn escape_asks_to_close_the_sheet(cx: &mut gpui_kit::TestAppContext) {
    let (_tree, mut native, events, _subscription) = open(cx, 390., 844.);
    native.run_until_parked();
    native.simulate_keystrokes("escape");
    assert_eq!(dismissals(&events), 1);
}

/// A centred modal does not drag, so it has no handle; under touch the
/// view leaves the panel's top to the host's band, which stays, empty, so
/// the view's first row is never flush with the panel's top. On a desktop
/// the view pads its own sheet and the host adds nothing.
#[gpui_kit::test]
fn a_centred_sheet_under_touch_keeps_the_handle_band(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    let (tree, mut native, _events, _subscription) = open(cx, 1000., 700.);
    // the panel is centred in the 700-tall body and ends where the rows
    // end: a band over the rows puts them half the band below centre
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

/// A page with a sheet that names no detents over it, holding rows
/// `rows` tall.
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

/// A sheet that names no detents is as tall as what it holds (the Expo
/// kit's `Sheet` without snaps), at most the body. Its handle drags it,
/// a short drag snaps it back to that height, and a release near the
/// bottom asks the view to close it.
#[gpui_kit::test]
fn a_sheet_with_no_detents_is_as_tall_as_its_child(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(390.), px(844.)), |_, _| {
        ViewTree::new(fitted(300.)).with_keys_grant(true)
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| window.render_frame(cx));
    let top = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, _| f32::from(window.find("rows").bounds().top()))
    };
    let band = crate::render::sheet::HANDLE_BAND;
    // the rows sit on the bottom edge, the handle's band over them
    assert!((top(&mut native) - (844. - 300.)).abs() < 1., "fitted");
    let handle = 844. - 300. - band / 2.;
    drag(&mut native, 195., handle, handle + 60.);
    native.update(|window, cx| window.render_frame(cx));
    assert!(
        (top(&mut native) - (844. - 300.)).abs() < 1.,
        "a short drag snaps back to the fitted height"
    );
    assert_eq!(dismissals(&events), 0);
    drag(&mut native, 195., handle, 830.);
    assert_eq!(dismissals(&events), 1, "near the bottom it asks to close");
    // what outgrows the body is held to it: the panel's top at the body's,
    // the rows under its hairline and the handle's band
    tree.update(&mut native, |tree, cx| tree.replace(fitted(2000.), &[], cx));
    native.update(|window, cx| window.render_frame(cx));
    let capped = top(&mut native);
    assert!(
        (capped - (1. + band)).abs() < 1.,
        "capped at the body: {capped}"
    );
}
