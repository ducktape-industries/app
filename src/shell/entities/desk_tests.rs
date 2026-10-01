//! One window's desk moved by its methods (desk.rs): each a compared edit
//! of the layout, framed and settled.
use super::tests::{DESK, notifies};
use super::{Desk, Slice};
use crate::ui::layout::Layout;
use gpui_kit::{AppContext as _, Entity, TestAppContext};

/// A measured desk with "chat" on it, as the console's first frame lands
/// it: sized, then seeded with the active program.
fn desk(cx: &mut TestAppContext) -> Entity<Desk> {
    let desk = cx.new(|_| Slice::new(Layout::default()));
    desk.update(cx, |desk, cx| {
        desk.resize(DESK, cx);
        desk.seed("chat", cx);
    });
    desk
}

fn modules(desk: &Entity<Desk>, cx: &TestAppContext) -> Vec<&'static str> {
    desk.read_with(cx, |desk, _| {
        desk.get().panes.iter().map(|pane| pane.module).collect()
    })
}

fn layout(desk: &Entity<Desk>, cx: &TestAppContext) -> Layout {
    desk.read_with(cx, |desk, _| desk.get().clone())
}

#[gpui_kit::test]
fn an_untouched_desk_opens_its_seed_once_and_every_window_has_a_frame(cx: &mut TestAppContext) {
    let desk = desk(cx);
    assert_eq!(modules(&desk, cx), ["chat"]);
    assert!(layout(&desk, cx).panes[0].frame.is_some());
    desk.update(cx, |desk, cx| _ = desk.close(0, cx));
    desk.update(cx, |desk, cx| desk.seed("files", cx));
    assert!(modules(&desk, cx).is_empty(), "a closed desk reopened");
}

/// A move that changes nothing notifies nobody: the pointer that stays
/// put, a hold dropped where there is none, a settle with nothing to
/// widen.
#[gpui_kit::test]
fn a_move_that_changes_nothing_notifies_nobody(cx: &mut TestAppContext) {
    let desk = desk(cx);
    let frame = layout(&desk, cx).panes[0].frame.unwrap();
    let (seen, _seen) = notifies(&desk, cx);
    desk.update(cx, |desk, cx| {
        desk.set_frame(0, frame, cx);
        desk.drop_hold(cx);
        desk.settle(cx);
        desk.resize(DESK, cx);
    });
    assert_eq!(seen.get(), 0);
    let mut moved = frame;
    moved.x += 10.;
    desk.update(cx, |desk, cx| desk.set_frame(0, moved, cx));
    assert_eq!(seen.get(), 1);
}

/// A window is placed before its view comes (60% of the desk, centred);
/// once the view is seated, it widens to the view's minimum and its
/// border, pulled left as far as it takes to stay on the desk, and on a
/// desk narrower than that it is the desk.
#[gpui_kit::test]
fn a_window_widens_to_its_view_once_the_view_is_seated(cx: &mut TestAppContext) {
    for (module, width, min_width, placed, widened) in [
        ("seated-wide-view", 1000., 680, (200., 600.), (200., 682.)),
        // the console at its smallest, forge opening in it
        ("seated-console-view", 720., 640, (144., 432.), (78., 642.)),
        ("seated-cramped-view", 600., 680, (120., 360.), (0., 600.)),
    ] {
        let desk = cx.new(|_| Slice::new(Layout::default()));
        desk.update(cx, |desk, cx| {
            desk.resize((width, 700.), cx);
            desk.seed(module, cx);
        });
        let frame = layout(&desk, cx).panes[0].frame.unwrap();
        assert_eq!((frame.x, frame.w), placed, "{module}");
        crate::runtime::seat_for_test(module, min_width);
        // `Seats` routes the seat's `Seated` to every desk holding it
        assert!(desk.read_with(cx, |desk, _| desk.holds(module)));
        desk.update(cx, |desk, cx| desk.settle(cx));
        let frame = layout(&desk, cx).panes[0].frame.unwrap();
        assert_eq!((frame.x, frame.w), widened, "{module}");
        assert!(frame.x + frame.w <= width, "{module}: {frame:?}");
    }
}

/// Leaving the network empties the desk, keeping its measure; it opens
/// the active program again once there is one.
#[gpui_kit::test]
fn a_cleared_desk_keeps_its_measure_and_seeds_again(cx: &mut TestAppContext) {
    let desk = desk(cx);
    desk.update(cx, |desk, cx| desk.clear(cx));
    let cleared = layout(&desk, cx);
    assert!(cleared.panes.is_empty());
    assert_eq!(cleared.desk, Some(DESK));
    assert!(!cleared.initialized);
    desk.update(cx, |desk, cx| desk.seed("files", cx));
    assert_eq!(modules(&desk, cx), ["files"]);
}
