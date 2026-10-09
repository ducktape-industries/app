//! What draws the bar and the pane strips, counted: the chrome re-renders
//! for what it observes (a height, the rail, a menu's clock) and stays
//! cached under everything else (a keystroke in Spotlight, root_tests.rs;
//! a drag), and its
//! dot slot is committed from the frame's callback (docs/perf.md).
use crate::shell::PaneMessage;
use crate::shell::entities::tests::status;
use crate::shell::layers::tests::{frame, line, pane, polled, set_motion, tree_renders};
use crate::shell::panes_tests::{console, window_count};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext, px, size};

/// A block landing moves the bar's height: the chrome draws once, and no
/// pane's view tree draws with it (the root and the pane layer, uncached,
/// draw with it).
#[gpui_kit::test]
fn a_beat_that_moves_a_height_re_renders_the_chrome_once_and_no_pane(cx: &mut TestAppContext) {
    const MODULE: &str = "chrome-height-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    crate::runtime::seat_drawing_for_test(MODULE, 400, line("a line"));
    set_motion(&app, false, &mut native);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    polled(&app, status(7), &mut native);
    for _ in 0..8 {
        frame(&mut native);
    }
    let (chrome, tree) = (window_count(key, "renders.chrome"), tree_renders(MODULE));
    assert!(chrome > 0 && tree > 0, "the bar and the tree drew");
    polled(&app, status(8), &mut native);
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

/// The rail's rows moving (a loader thread's word, no clock) draw the
/// chrome once, with nothing dispatched.
#[gpui_kit::test]
fn a_roster_change_re_renders_the_chrome_without_a_beat(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (app, key, _, mut native) = console(cx);
    native.run_until_parked();
    let chrome = window_count(key, "renders.chrome");
    assert!(chrome > 0, "the bar never drew");
    // the rail is refreshed by hand: no loader thread reaches a test's
    // scheduler (the drain itself is `entities::rail`'s own test)
    let roster = crate::runtime::Roster::listing(&["chrome-roster-alpha"]);
    app.rail
        .update(&mut native, |rail, cx| rail.read_off(roster, cx));
    native.run_until_parked();
    assert_eq!(
        window_count(key, "renders.chrome"),
        chrome + 1,
        "the rail moved and the chrome drew other than once"
    );
}

/// A pane dragged to a new frame (the pointer's move on a held title)
/// moves the desk (`Desk::set_frame`, one notify) and nothing else:
/// nothing the chrome reads, so the bar stays cached. The press draws the
/// chrome once (gpui's `div` calls `window.refresh()` for a pending click,
/// and again on the release that clears its active state); the frames
/// between them are the drag's, and the move is measured alone.
#[gpui_kit::test]
fn a_drag_frame_moves_the_desk_once_and_leaves_the_chrome_cached(cx: &mut TestAppContext) {
    use crate::shell::layers;
    use gpui_kit::{MouseButton, MouseDownEvent, MouseMoveEvent, PlatformInput};
    const MODULE: &str = "chrome-drag-view";
    let _on = crate::perf::on_for_test();
    let (_, key, view, mut native) = console(cx);
    crate::runtime::seat_for_test(MODULE, 400);
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    for _ in 0..3 {
        frame(&mut native);
    }
    let mut moved = native
        .update(|_, cx| view.read(cx).layout(cx).panes[0].frame)
        .expect("the pane has no frame");
    let title = gpui_kit::point(px(moved.x + 100.), px(layers::BAR + moved.y + 15.));
    let to = gpui_kit::point(title.x + px(40.), title.y + px(30.));
    moved.x += 40.;
    moved.y += 30.;
    let desk = native.update(|_, cx| view.read(cx).desk.clone());
    // the press takes hold of the title (and draws the chrome once, as any
    // press does: gpui's `div` refreshes the window for the pending click)
    native.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                position: title,
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        )
    });
    native.run_until_parked();
    frame(&mut native);
    let chrome = window_count(key, "renders.chrome");
    let (told, _told) = crate::shell::entities::tests::notifies(&desk, &mut native);
    // the pointer's move, as `pane_drag::follow` lands it
    native.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: to,
                pressed_button: Some(MouseButton::Left),
                modifiers: Default::default(),
            }),
            cx,
        )
    });
    native.run_until_parked();
    frame(&mut native);
    let frame_now = native.update(|_, cx| view.read(cx).layout(cx).panes[0].frame);
    assert_eq!(frame_now, Some(moved), "the drag never landed");
    assert_eq!(told.get(), 1, "the desk notified other than once");
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
    let (app, key, _, mut native) = console(cx);
    let dot = app
        .windows
        .read_with(&native, |windows, _| windows.own(key).unwrap().dot.clone());
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
    let (app, key, view, mut native) = console(cx);
    let overlays = native.update(|_, cx| view.read(cx).overlays());
    let second = |native: &mut VisualTestContext| {
        native
            .executor()
            .advance_clock(std::time::Duration::from_millis(1000));
        native.run_until_parked();
    };
    set_motion(&app, false, &mut native);
    polled(&app, status(7), &mut native);
    // the first seconds after a status settle the bar
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
/// that becomes Ready draws the strip once, on no clock.
#[gpui_kit::test]
fn the_strip_title_follows_the_rail(cx: &mut TestAppContext) {
    const MODULE: &str = "chrome-strip-view";
    let _on = crate::perf::on_for_test();
    let (app, key, view, mut native) = console(cx);
    // listed, not yet seated: a Loading row
    let roster = crate::runtime::Roster::listing(&[MODULE]);
    app.rail
        .update(&mut native, |rail, cx| rail.read_off(roster, cx));
    pane(&view, PaneMessage::Select(MODULE), &mut native);
    for _ in 0..3 {
        frame(&mut native);
    }
    let rows =
        |native: &mut VisualTestContext| app.rail.read_with(native, |rail, _| rail.rows().to_vec());
    assert_eq!(rows(&mut native)[0].note, Some("Loading"));
    let strip = window_count(key, "renders.strip.0");
    assert!(strip > 0, "the strip never drew");
    // the seat lands; the rail hears it (by hand: no loader thread reaches
    // a test's scheduler) and the strip follows
    crate::runtime::seat_for_test(MODULE, 400);
    app.rail.update(&mut native, |rail, cx| {
        rail.refresh(cx);
    });
    native.run_until_parked();
    assert_eq!(rows(&mut native)[0].note, None, "the row is still Loading");
    assert_eq!(
        window_count(key, "renders.strip.0"),
        strip + 1,
        "the rail moved and the strip drew other than once"
    );
}

/// A folded tab whose view declares an icon the app has no file for is its
/// name's initial, as the tab of a view that declares none: a test's app
/// bundles no file, so neither tab is an icon's 16px box. (The tab that
/// draws its icon is `chrome::tests`': no test's app has a file to draw.)
#[gpui_kit::test]
fn a_folded_tab_with_no_icon_to_draw_is_its_initial(cx: &mut TestAppContext) {
    const DECLARED: &str = "chrome-fold-declared";
    const PLAIN: &str = "chrome-fold-plain";
    let (app, _, _, mut native) = console(cx);
    crate::runtime::seat_compiled_for_test(DECLARED, "Folded", "icons/hammer.svg");
    crate::runtime::seat_compiled_for_test(PLAIN, "Folded", "");
    let roster = crate::runtime::Roster::listing(&[DECLARED, PLAIN]);
    app.rail
        .update(&mut native, |rail, cx| rail.read_off(roster, cx));
    // how wide the door's tree has each tab, once the bar has measured itself
    let widths = |native: &mut VisualTestContext| {
        for _ in 0..4 {
            frame(native);
        }
        let nodes = native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            serde_json::to_value(crate::ax::snapshot("console", window, true)).unwrap()
        });
        let nodes = nodes.as_array().unwrap();
        [DECLARED, PLAIN].map(|module| {
            let id = format!("console:rail/{module}");
            let tab = nodes.iter().find(|node| node["id"] == id);
            let tab = tab.unwrap_or_else(|| panic!("no {id}: {nodes:?}"));
            assert_eq!(tab["name"], "Folded", "a folded tab keeps its whole name");
            tab["bounds"][2].as_i64().unwrap() - tab["bounds"][0].as_i64().unwrap()
        })
    };
    let whole = widths(&mut native);
    native.simulate_resize(size(px(480.), px(800.)));
    native.run_until_parked();
    let folded = widths(&mut native);
    assert!(folded[1] < whole[1], "the bar did not fold: {folded:?}");
    assert_eq!(
        folded[0], folded[1],
        "a tab with no icon to draw is not its initial"
    );
}
