//! What draws the bar and the pane strips, counted: the chrome re-renders
//! for what it observes (a height, the rail, a menu's clock) and stays
//! cached under everything else (a keystroke in Spotlight, root_tests.rs;
//! a drag), and its
//! dot slot is committed from the frame's callback (docs/perf.md).
use crate::shell::layers::tests::polled;
use crate::shell::panes_tests::{console, window_count};
use crate::shell::{Message, PaneMessage};
use crate::ui::test_support::status;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Styled as _, TestAppContext, VisualTestContext, px, size};
use view_wire as wire;

/// A frame as the platform delivers one: what asked for it runs, then
/// whatever that dirtied draws.
fn frame(native: &mut VisualTestContext) {
    native.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    native.run_until_parked();
}

fn line(text: &str) -> wire::Node {
    wire::Node::RichText {
        id: Some(wire::ElementIdWire::Name("line".into())),
        style: gpui_kit::div().h(px(20.)).style().clone(),
        text: text.into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    }
}

fn tree_renders(module: &str) -> u64 {
    crate::perf::snapshot(false)["views"][module]["renders"]
        .as_u64()
        .unwrap_or_else(|| panic!("{module} counts its renders"))
}

/// A block landing moves the bar's height: the chrome draws once, and no
/// pane's view tree draws with it (the root and the pane layer still draw
/// under the beat gate until s7).
#[gpui_kit::test]
fn a_beat_that_moves_a_height_re_renders_the_chrome_once_and_no_pane(cx: &mut TestAppContext) {
    const MODULE: &str = "chrome-height-view";
    let _on = crate::perf::on_for_test();
    let (model, key, _, mut native) = console(cx);
    let send = |message: Message, native: &mut VisualTestContext| {
        model.update(native, |model, cx| model.dispatch(message, cx));
        native.run_until_parked();
    };
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    send(Message::SetMotion(false), &mut native);
    send(Message::Pane(key, PaneMessage::Select(MODULE)), &mut native);
    polled(&model, status(7), &mut native);
    for _ in 0..8 {
        frame(&mut native);
    }
    let (chrome, tree) = (window_count(key, "renders.chrome"), tree_renders(MODULE));
    assert!(chrome > 0 && tree > 0, "the bar and the tree drew");
    polled(&model, status(8), &mut native);
    for _ in 0..3 {
        frame(&mut native);
    }
    assert_eq!(
        window_count(key, "renders.chrome"),
        chrome + 1,
        "a height moved and the chrome drew other than once"
    );
    assert_eq!(tree_renders(MODULE), tree, "a height moved and a tree drew");
}

/// The rail's rows moving (a loader thread's word, not a beat) draw the
/// chrome once, with nothing dispatched.
#[gpui_kit::test]
fn a_roster_change_re_renders_the_chrome_without_a_beat(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (model, key, _, mut native) = console(cx);
    native.run_until_parked();
    let chrome = window_count(key, "renders.chrome");
    assert!(chrome > 0, "the bar never drew");
    // the rail is refreshed by hand: no loader thread reaches a test's
    // scheduler (the drain itself is `entities::rail`'s own test)
    let roster = crate::runtime::Roster::listing(&["chrome-roster-alpha"]);
    model.update(&mut native, |model, cx| {
        model
            .entities
            .rail
            .update(cx, |rail, cx| rail.read_off(roster, cx));
    });
    native.run_until_parked();
    assert_eq!(
        window_count(key, "renders.chrome"),
        chrome + 1,
        "the rail moved and the chrome drew other than once"
    );
}

/// A pane dragged to a new frame moves the desk and nothing the chrome
/// reads: the bar stays cached.
#[gpui_kit::test]
fn a_drag_frame_leaves_the_chrome_cached(cx: &mut TestAppContext) {
    const MODULE: &str = "chrome-drag-view";
    let _on = crate::perf::on_for_test();
    let (model, key, view, mut native) = console(cx);
    let send = |message: Message, native: &mut VisualTestContext| {
        model.update(native, |model, cx| model.dispatch(message, cx));
        native.run_until_parked();
    };
    crate::runtime::seat_for_test(MODULE, 400);
    send(Message::Pane(key, PaneMessage::Select(MODULE)), &mut native);
    for _ in 0..3 {
        frame(&mut native);
    }
    let chrome = window_count(key, "renders.chrome");
    let mut moved = native
        .update(|_, cx| view.read(cx).layout(cx).panes[0].frame)
        .expect("the pane has no frame");
    moved.x += 40.;
    moved.y += 30.;
    send(
        Message::Pane(key, PaneMessage::Frame(0, moved)),
        &mut native,
    );
    frame(&mut native);
    let frame_now = native.update(|_, cx| view.read(cx).layout(cx).panes[0].frame);
    assert_eq!(frame_now, Some(moved), "the drag never landed");
    assert_eq!(
        window_count(key, "renders.chrome"),
        chrome,
        "a drag frame drew the chrome"
    );
}

/// The dot's slot (where the bar laid its 12px well out) reaches `DotSlot`
/// from the frame's callback, after the frame that placed it, never from
/// the draw; and a bar that has not moved commits nothing.
#[gpui_kit::test]
fn the_dot_slot_is_committed_after_the_frame(cx: &mut TestAppContext) {
    let (model, key, _, mut native) = console(cx);
    let dot = model.read_with(&native, |model, _| {
        model.entities.by_window[&key].dot.clone()
    });
    let slot = |native: &mut VisualTestContext| native.update(|_, cx| *dot.read(cx).get());
    // the fixture delivered the first frame's callback
    let first = slot(&mut native).expect("the first frame committed no slot");
    assert_eq!(first.size, size(px(12.), px(12.)));
    assert!(
        first.origin.x > px(1000.),
        "the well is at the bar's right end"
    );
    // the same bar again: nothing to commit, no callback asked for
    native.update(|window, cx| {
        window.render_frame(cx);
    });
    assert_eq!(
        native.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "a frame with the bar still asked for a callback"
    );
    // a narrower window moves the well left: the draw shows it, the slot
    // follows from the callback
    native.simulate_resize(size(px(1000.), px(800.)));
    native.run_until_parked();
    native.update(|window, cx| {
        window.render_frame(cx);
    });
    assert_eq!(
        slot(&mut native),
        Some(first),
        "the draw itself moved the slot"
    );
    assert!(
        native.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "the frame that moved the well asked for no callback"
    );
    let moved = slot(&mut native).unwrap();
    assert_eq!(moved.size, first.size);
    assert!(
        moved.origin.x < first.origin.x - px(200.),
        "{moved:?} did not follow"
    );
}

/// The ages a menu counts (the node's last block, the bell's notices) tick
/// on the menu's own second while it is open, and only then: closed, the
/// second passes and nothing draws.
#[gpui_kit::test]
fn a_menus_ages_tick_only_while_it_is_open(cx: &mut TestAppContext) {
    use crate::shell::entities::{Overlay, Popover};
    let _on = crate::perf::on_for_test();
    let (model, key, view, mut native) = console(cx);
    let overlays = native.update(|_, cx| view.read(cx).overlays());
    let send = |message: Message, native: &mut VisualTestContext| {
        model.update(native, |model, cx| model.dispatch(message, cx));
        native.run_until_parked();
    };
    let second = |native: &mut VisualTestContext| {
        native
            .executor()
            .advance_clock(std::time::Duration::from_millis(1000));
        native.run_until_parked();
    };
    send(Message::SetMotion(false), &mut native);
    polled(&model, status(7), &mut native);
    // the beats' first second after a status settles the bar
    second(&mut native);
    second(&mut native);
    for menu in [Popover::Node, Popover::Notifications] {
        let still = window_count(key, "renders.chrome");
        second(&mut native);
        assert_eq!(
            window_count(key, "renders.chrome"),
            still,
            "a second passed with every menu shut and the chrome drew"
        );
        overlays.update(&mut native, |it, cx| it.open(Overlay::Menu(menu), cx));
        native.run_until_parked();
        let open = window_count(key, "renders.chrome");
        second(&mut native);
        assert_eq!(
            window_count(key, "renders.chrome"),
            open + 1,
            "a second passed under {menu:?} and the chrome drew other than once"
        );
        overlays.update(&mut native, |it, cx| it.close(Overlay::Menu(menu), cx));
        native.run_until_parked();
    }
    let shut = window_count(key, "renders.chrome");
    second(&mut native);
    assert_eq!(
        window_count(key, "renders.chrome"),
        shut,
        "the menus shut and their clock kept drawing"
    );
}

/// A pane's strip names its program from the rail: a row still Loading
/// that becomes Ready draws the strip once, with no beat.
#[gpui_kit::test]
fn the_strip_title_follows_the_rail(cx: &mut TestAppContext) {
    const MODULE: &str = "chrome-strip-view";
    let _on = crate::perf::on_for_test();
    let (model, key, _, mut native) = console(cx);
    let send = |message: Message, native: &mut VisualTestContext| {
        model.update(native, |model, cx| model.dispatch(message, cx));
        native.run_until_parked();
    };
    // listed, not yet seated: a Loading row
    let roster = crate::runtime::Roster::listing(&[MODULE]);
    model.update(&mut native, |model, cx| {
        model.state.roster = roster.clone();
        model
            .entities
            .rail
            .update(cx, |rail, cx| rail.read_off(roster, cx));
    });
    send(Message::Pane(key, PaneMessage::Select(MODULE)), &mut native);
    for _ in 0..3 {
        frame(&mut native);
    }
    let rows = |native: &mut VisualTestContext| {
        model.read_with(native, |model, cx| {
            model.entities.rail.read(cx).rows().to_vec()
        })
    };
    assert_eq!(rows(&mut native)[0].note, Some("Loading"));
    let strip = window_count(key, "renders.strip.0");
    assert!(strip > 0, "the strip never drew");
    // the seat lands; the rail hears it (by hand: no loader thread reaches
    // a test's scheduler) and the strip follows
    crate::runtime::seat_for_test(MODULE, 400);
    model.update(&mut native, |model, cx| {
        model.entities.rail.update(cx, |rail, cx| {
            rail.refresh(cx);
        });
    });
    native.run_until_parked();
    assert_eq!(rows(&mut native)[0].note, None, "the row is still Loading");
    assert_eq!(
        window_count(key, "renders.strip.0"),
        strip + 1,
        "the rail moved and the strip drew other than once"
    );
}
