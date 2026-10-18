//! The pane model, moved by hand: selection, splitting, focus, pop-in,
//! placement, clamping, rescaling, cycling.

use super::*;

const DESK: (f32, f32) = (1400., 860.);

#[test]
fn rail_selection_focuses_existing_or_replaces_only_focused() {
    let mut layout = Layout::default();
    assert!(layout.select("chat"));
    let chat = layout.panes[0].instance;
    layout.split("files");
    assert!(layout.select("chat"));
    assert_eq!(layout.focused, 0);
    assert_eq!(layout.panes[0].instance, chat);
    assert!(!layout.select("chat"));
    layout.place(DESK);
    let frame = layout.panes[0].frame;
    assert!(layout.select("calendar"));
    assert_eq!(layout.panes.len(), 2);
    assert_eq!(layout.panes[0].program, "calendar");
    assert_eq!(
        layout.panes[0].frame, frame,
        "a replaced view keeps its window"
    );
    assert_ne!(layout.panes[0].instance, chat);
    assert_eq!(layout.panes[1].program, "files");
}

#[test]
fn split_opens_on_top_with_unique_instances_and_any_count() {
    let mut layout = Layout::default();
    layout.split("chat");
    layout.split("files");
    assert!(layout.focus(0));
    layout.split("chat");
    assert_eq!(layout.focused, 1);
    assert_eq!(layout.panes[1].program, "chat");
    assert_eq!(
        *layout.stacking().last().unwrap(),
        1,
        "the new window is on top"
    );
    assert_ne!(layout.panes[0].instance, layout.panes[1].instance);
    for _ in 0..64 {
        layout.split("calendar");
    }
    assert_eq!(layout.panes.len(), 67);
    assert!(!layout.focus(67));
}

#[test]
fn focus_raises_and_closing_focuses_the_window_left_on_top() {
    let mut layout = Layout::default();
    for program in ["chat", "files", "calendar"] {
        layout.split(program);
    }
    assert!(layout.focus(0));
    assert_eq!(layout.stacking(), vec![1, 2, 0]);
    layout.measure(DESK);
    for (index, pane) in layout.panes.iter_mut().enumerate() {
        pane.frame = part(INSET, INSET, 776., 576.);
        pane.frame.as_mut().unwrap().x += index as f32 * 100.;
    }
    assert_eq!(layout.under((50., 50.)), Some(0), "the top of three");
    assert_eq!(layout.under((250., 50.)), Some(0));
    assert_eq!(
        layout.under((850., 50.)),
        Some(2),
        "only the last reaches here"
    );
    assert_eq!(layout.under((990., 50.)), Some(2), "just past its border");
    assert_eq!(layout.under((5., 5.)), None, "the desk's inset");
    // a window hanging off the desk's left edge, under the sidebar
    layout.panes[0].frame.as_mut().unwrap().x = -200.;
    assert_eq!(layout.under((-50., 50.)), None, "off the desk");
    assert_eq!(layout.under((50., 50.)), Some(0), "on the desk");
    layout.panes[0].frame.as_mut().unwrap().x = INSET;
    assert!(!layout.focus(0), "already focused and on top");
    assert!(layout.close(0).is_some());
    // "calendar" (now index 1) was raised after "files"
    assert_eq!(layout.focused, 1);
    assert!(layout.close(1).is_some());
    assert!(layout.close(0).is_some());
    assert!(layout.panes.is_empty());
    assert!(layout.close(0).is_none());
    assert!(!layout.focus(0));
}

#[test]
fn popout_and_popin_transfer_instance_and_add_a_window() {
    let mut source = Layout::default();
    source.split("chat");
    let original = source.panes[0].instance;
    let pane = source.close(0).unwrap();
    assert!(source.panes.is_empty());
    let mut destination = Layout::default();
    destination.popin(pane);
    assert_eq!(destination.panes[0].instance, original);
    for _ in 0..16 {
        destination.split("files");
    }
    destination.focus(1);
    let pane = Pane::new("chat");
    let incoming = pane.instance;
    destination.popin(pane);
    assert_eq!(destination.panes.len(), 18);
    assert_eq!(destination.panes[17].instance, incoming);
    assert_eq!(destination.focused, 17);
    assert!(destination.close(18).is_none());
}

#[test]
fn the_first_window_opens_centred_and_later_ones_cascade_inside_it() {
    let mut layout = Layout::default();
    layout.split("chat");
    layout.place(DESK);
    let first = layout.panes[0].frame.unwrap();
    assert_eq!(first.x * 2. + first.w, DESK.0);
    assert_eq!(first.y * 2. + first.h, DESK.1);
    assert!(first.w < DESK.0 && first.h < DESK.1);
    layout.split("files");
    layout.place(DESK);
    let second = layout.panes[1].frame.unwrap();
    assert_eq!((second.x, second.y), (first.x + CASCADE, first.y + CASCADE));
    assert!(second.x + second.w <= DESK.0 && second.y + second.h <= DESK.1);
}

#[test]
fn frames_stay_grabbable_and_reject_invalid_input() {
    let mut layout = Layout::default();
    layout.split("chat");
    layout.place(DESK);
    let far = Frame {
        x: 5000.,
        y: -300.,
        w: 10.,
        h: 10.,
    };
    assert!(layout.set_frame(0, far, DESK));
    let kept = layout.panes[0].frame.unwrap();
    assert_eq!((kept.w, kept.h), (MIN_WIDTH, MIN_HEIGHT));
    assert!(kept.x <= DESK.0 - KEEP && kept.y >= 0.);
    // the left and top edges stop at the desk's
    let left = Frame {
        x: -300.,
        y: -300.,
        ..kept
    };
    assert!(layout.set_frame(0, left, DESK));
    let kept = layout.panes[0].frame.unwrap();
    assert_eq!((kept.x, kept.y), (0., 0.));
    let nan = Frame {
        x: f32::NAN,
        ..kept
    };
    assert!(!layout.set_frame(0, nan, DESK));
    assert!(!layout.set_frame(3, kept, DESK));
    assert_eq!(layout.panes[0].frame, Some(kept));
    // a shrinking desk pulls windows back onto it
    layout.place((600., 400.));
    let shrunk = layout.panes[0].frame.unwrap();
    assert!(shrunk.x <= 600. - KEEP);
}

/// A window holding a drawn view is never narrower than the view's
/// own minimum and its border, unless the desk is: then it is the desk.
#[test]
fn a_window_is_never_narrower_than_its_view() {
    let narrow = Frame {
        x: 100.,
        y: 100.,
        w: 10.,
        h: 400.,
    };
    assert_eq!(narrow.clamped(DESK, 682.).w, 682.);
    assert_eq!(narrow.clamped((600., 400.), 682.).w, 600., "the desk");
    crate::runtime::seat_for_test("layout-wide-view", 680);
    let mut layout = Layout::default();
    layout.split("layout-wide-view");
    layout.place(DESK);
    assert_eq!(layout.panes[0].min_width(), 682.);
    assert!(layout.set_frame(0, narrow, DESK));
    assert_eq!(layout.panes[0].frame.unwrap().w, 682.);
    // a narrow view still goes down to the smallest window
    crate::runtime::seat_for_test("layout-narrow-view", 280);
    assert_eq!(Pane::new("layout-narrow-view").min_width(), MIN_WIDTH);
}

/// A view that asks a width opens its window at it and the frame's
/// border, centred, where the desk's share is narrower; at the desk's
/// width on a desk narrower than that; never under the view's own
/// minimum. A view that asks none opens at the share.
#[test]
fn a_window_opens_at_the_width_its_view_asks() {
    let opened = |program: &'static str, desk: (f32, f32)| {
        let mut layout = Layout::default();
        layout.split(program);
        layout.place(desk);
        layout
    };
    crate::runtime::seat_for_test("layout-asks-wide", 560);
    crate::runtime::width_for_test("layout-asks-wide", 1200);
    let mut layout = opened("layout-asks-wide", DESK);
    let first = layout.panes[0].frame.unwrap();
    assert_eq!(first.w, 1202., "the width and its border, past the share");
    assert_eq!(first.x * 2. + first.w, DESK.0, "centred");
    layout.split("layout-asks-wide");
    layout.place(DESK);
    let second = layout.panes[1].frame.unwrap();
    assert_eq!(
        (second.x, second.y, second.w),
        (first.x + CASCADE, first.y + CASCADE, 1202.),
        "cascaded"
    );
    let narrow = (1000., 700.);
    let frame = opened("layout-asks-wide", narrow).panes[0].frame.unwrap();
    assert_eq!((frame.x, frame.w), (0., narrow.0), "the desk");
    crate::runtime::seat_for_test("layout-asks-under-its-min", 900);
    crate::runtime::width_for_test("layout-asks-under-its-min", 600);
    let frame = opened("layout-asks-under-its-min", DESK).panes[0]
        .frame
        .unwrap();
    assert_eq!(frame.w, 902., "the view's minimum and its border");
    crate::runtime::seat_for_test("layout-asks-nothing", 560);
    let frame = opened("layout-asks-nothing", DESK).panes[0].frame.unwrap();
    assert_eq!(frame.w, (DESK.0 * NEW_SHARE.0).round(), "the share");
}

/// A window placed before its view came grows to the width the view asks
/// once it is seated (`Seated` settles the desk), about its own centre and
/// on the desk; one the person moved or sized stays as they left it.
#[test]
fn a_late_width_widens_only_a_window_the_person_left_alone() {
    let programs = [
        "layout-late-alone",
        "layout-late-sized",
        "layout-late-right",
    ];
    let mut layout = Layout::default();
    for program in programs {
        layout.split(program);
    }
    layout.measure(DESK);
    layout.settle();
    let left = layout.panes[0].frame.unwrap();
    assert_eq!(
        (left.w, left.x * 2. + left.w),
        ((DESK.0 * NEW_SHARE.0).round(), DESK.0),
        "no view yet: the share, centred"
    );
    assert!(layout.set_frame(0, left, DESK));
    assert!(!layout.panes[0].by_hand, "a press that moves nothing");
    let by_hand = Frame {
        w: 700.,
        ..layout.panes[1].frame.unwrap()
    };
    assert!(layout.set_frame(1, by_hand, DESK));
    // the app's frame off the desk's centre, as a cascade or a resized
    // desk leaves one: grown about its centre it would pass the right edge
    layout.panes[2].frame.as_mut().unwrap().x = 500.;
    for program in programs {
        crate::runtime::seat_for_test(program, 560);
        crate::runtime::width_for_test(program, 1200);
    }
    layout.settle();
    let widened = layout.panes[0].frame.unwrap();
    assert_eq!(
        (widened.w, widened.y),
        (1202., left.y),
        "the width and its border"
    );
    assert_eq!(widened.x * 2. + widened.w, DESK.0, "about its own centre");
    let right = layout.panes[2].frame.unwrap();
    assert_eq!((right.x, right.w), (DESK.0 - 1202., 1202.), "on the desk");
    assert_eq!(layout.panes[1].frame, Some(by_hand), "the person's");
}

/// Fill is the desk's: on and off again, every frame is where it was.
#[test]
fn filling_the_desk_and_filling_it_again_leaves_every_frame_as_it_was() {
    let mut layout = held_pair();
    let frames = |layout: &Layout| {
        layout
            .panes
            .iter()
            .map(|pane| pane.frame)
            .collect::<Vec<_>>()
    };
    let before = frames(&layout);
    layout.toggle_fill();
    assert!(layout.filled);
    assert_eq!(frames(&layout), before, "fill rewrites no frame");
    layout.toggle_fill();
    assert!(!layout.filled);
    assert_eq!(frames(&layout), before);
    layout.clear();
    layout.toggle_fill();
    assert!(!layout.filled, "an empty desk has nothing to fill");
}

/// Whatever comes forward fills: a focus, a cycle, another window, one
/// popped in, the one beneath a closed front. The last window closed
/// turns fill off.
#[test]
fn fill_stays_on_as_windows_come_forward_until_the_last_one_closes() {
    let mut layout = held_pair();
    layout.toggle_fill();
    assert!(layout.focus(0));
    assert!(layout.cycle(true));
    layout.split("calendar");
    layout.popin(Pane::new("files"));
    assert!(layout.filled, "kept through focus, cycle, split, pop-in");
    layout.close(layout.focused);
    assert!(layout.filled, "the window beneath the front fills");
    while layout.panes.len() > 1 {
        layout.close(0);
    }
    assert!(layout.filled);
    layout.close(0);
    assert!(!layout.filled, "the last window closed");
}

/// A filled desk is all the window in front: a press anywhere on it is
/// on that window, never on one covered at its frame.
#[test]
fn a_filled_desk_is_the_window_in_front_wherever_it_is_pressed() {
    let mut layout = tiled(&[
        ("chat", part(700., 430., 600., 400.)),
        ("files", part(20., 20., 400., 300.)),
    ]);
    layout.measure(DESK);
    assert_eq!(layout.under((800., 500.)), Some(0), "the window there");
    layout.toggle_fill();
    assert_eq!(layout.under((800., 500.)), Some(1), "the window in front");
    assert_eq!(layout.under((DESK.0 + 5., 500.)), None, "off the desk");
}

/// A view's title is kept per pane, empty as none; only a change counts.
#[test]
fn a_title_is_set_once_and_empty_is_none() {
    let mut layout = held_pair();
    let instance = layout.panes[1].instance;
    assert!(layout.set_title(instance, Some("#general".into())));
    assert_eq!(layout.panes[1].title.as_deref(), Some("#general"));
    assert_eq!(layout.panes[0].title, None, "the other pane's own");
    assert!(
        !layout.set_title(instance, Some("#general".into())),
        "the same"
    );
    assert!(layout.set_title(instance, Some(String::new())));
    assert_eq!(layout.panes[1].title, None, "empty is none");
    assert!(!layout.set_title(instance, None));
    assert!(
        !layout.set_title(u64::MAX, Some("gone".into())),
        "no such pane"
    );
}

/// A frame on the desk, for windows laid edge to edge by hand.
fn part(x: f32, y: f32, w: f32, h: f32) -> Option<Frame> {
    Some(Frame { x, y, w, h })
}

/// `programs` in windows at `frames`, the last one focused.
fn tiled(frames: &[(&'static str, Option<Frame>)]) -> Layout {
    let mut layout = Layout::default();
    for &(program, frame) in frames {
        layout.split(program);
        layout.panes[layout.focused].frame = frame;
    }
    layout
}

/// A closed window's space stays empty: its neighbours keep their frames.
#[test]
fn closing_a_window_leaves_the_others_where_they_are() {
    let (left, top) = (part(12., 12., 688., 836.), part(700., 12., 688., 418.));
    let mut layout = tiled(&[
        ("chat", left),
        ("files", top),
        ("calendar", part(700., 430., 688., 418.)),
    ]);
    layout.close(2);
    assert_eq!(layout.panes[0].frame, left);
    assert_eq!(layout.panes[1].frame, top);
    layout.close(0);
    assert_eq!(layout.panes[0].frame, top);
}

#[test]
fn a_resized_desk_scales_every_window_with_it() {
    let mut layout = tiled(&[
        ("chat", part(12., 12., 688., 836.)),
        (EMPTY, part(700., 12., 688., 836.)),
    ]);
    layout.measure(DESK);
    let small = Frame {
        x: 100.,
        y: 80.,
        w: 500.,
        h: 400.,
    };
    layout.split("files");
    layout.place(DESK);
    layout.set_frame(2, small, DESK);
    let big = (DESK.0 * 1.6, DESK.1 * 1.5);
    layout.measure(big);
    layout.settle();
    let (left, right) = (
        layout.panes[0].frame.unwrap(),
        layout.panes[1].frame.unwrap(),
    );
    assert_eq!((left.x, left.y), (INSET, INSET));
    assert_eq!(left.x + left.w, right.x, "edge to edge stays flush");
    assert_eq!(right.x + right.w, big.0 - INSET);
    assert_eq!(left.h, big.1 - 2. * INSET);
    let floating = layout.panes[2].frame.unwrap();
    let share = |at: f32, of: f32, desk: f32| {
        (at - INSET) / (desk - 2. * INSET) - (of - INSET) / (DESK.0 - 2. * INSET)
    };
    assert!(share(floating.x, small.x, big.0).abs() < 1e-4);
    // and back: the frames it had
    layout.measure(DESK);
    layout.settle();
    let back = layout.panes[2].frame.unwrap();
    for (a, b) in [
        (back.x, small.x),
        (back.y, small.y),
        (back.w, small.w),
        (back.h, small.h),
    ] {
        assert!((a - b).abs() < 1e-3, "{back:?} != {small:?}");
    }
    assert_eq!(
        layout.panes[0].frame.unwrap().w + layout.panes[1].frame.unwrap().w,
        DESK.0 - 2. * INSET
    );
}

#[test]
fn the_menu_bar_fills_an_empty_window_focuses_an_open_one_else_opens_another() {
    let mut layout = Layout::default();
    layout.split(EMPTY);
    let empty = layout.panes[0].instance;
    assert!(layout.open("chat"));
    assert_eq!((layout.panes.len(), layout.panes[0].program), (1, "chat"));
    assert_ne!(layout.panes[0].instance, empty);
    assert!(layout.open("files"), "a window of its own");
    assert_eq!((layout.panes.len(), layout.focused), (2, 1), "and the keys");
    assert!(layout.open("chat"));
    assert_eq!((layout.panes.len(), layout.focused), (2, 0));
}

#[test]
fn cycling_visits_every_window_and_back_returns() {
    let mut layout = Layout::default();
    for program in ["chat", "files", "calendar"] {
        layout.split(program);
    }
    let mut seen = vec![];
    for _ in 0..3 {
        assert!(layout.cycle(true));
        assert_eq!(*layout.stacking().last().unwrap(), layout.focused);
        seen.push(layout.focused);
    }
    assert_eq!(seen, vec![0, 1, 2]);
    assert!(layout.cycle(false));
    assert_eq!(layout.focused, 1);
    assert!(layout.cycle(true));
    assert_eq!(layout.focused, 2);
    layout.close(layout.focused);
    assert_eq!(layout.panes.len(), 2);
    assert_eq!(layout.focused, 1, "the one beneath takes focus");
    layout.close(0);
    assert!(!layout.cycle(true), "one window has nowhere to go");
}

fn held_pair() -> Layout {
    let mut layout = Layout::default();
    layout.split("chat");
    layout.split("files");
    layout.place(DESK);
    layout
}

#[test]
fn the_keyboard_holds_a_window_once_it_has_a_frame() {
    let mut layout = Layout::default();
    layout.split("chat");
    layout.hold(0);
    assert_eq!(layout.held, None, "no frame yet: nothing to move");
    layout.place(DESK);
    layout.hold(5);
    assert_eq!(layout.held, None, "no such window");
    layout.hold(0);
    assert_eq!(
        layout.held.map(|held| held.instance),
        Some(layout.panes[0].instance)
    );
    layout.hold(0);
    assert_eq!(layout.held, None, "holding it again lets it go");
}

#[test]
fn letting_go_keeps_the_window_or_puts_it_back_as_it_was() {
    let mut layout = held_pair();
    let before = layout.panes[1].frame.unwrap();
    layout.hold(1);
    let moved = Frame {
        x: before.x + 40.,
        ..before
    };
    layout.set_frame(1, moved, DESK);
    layout.release(true, DESK);
    assert_eq!(layout.held, None);
    assert_eq!(layout.panes[1].frame, Some(moved), "kept");

    layout.hold(1);
    layout.set_frame(1, Frame { w: 500., ..moved }, DESK);
    layout.release(false, DESK);
    assert_eq!(layout.panes[1].frame, Some(moved), "put back");
}

/// A hold on a filled desk turns fill off first; Escape puts the window
/// back and fill on again, Return keeps it off.
#[test]
fn a_hold_leaves_fill_and_escape_puts_it_back() {
    let mut layout = held_pair();
    layout.toggle_fill();
    let frame = layout.panes[1].frame;
    layout.hold(1);
    assert!(!layout.filled, "held: the window at its frame, to move");
    let now = frame.unwrap();
    layout.set_frame(
        1,
        Frame {
            x: now.x + 20.,
            ..now
        },
        DESK,
    );
    layout.release(false, DESK);
    assert_eq!((layout.panes[1].frame, layout.filled), (frame, true));
    layout.hold(1);
    layout.release(true, DESK);
    assert!(!layout.filled, "kept: fill stays off");
}

#[test]
fn a_hold_ends_when_another_window_comes_to_the_front() {
    let mut layout = held_pair();
    layout.hold(1);
    layout.settle();
    assert!(layout.held.is_some());
    layout.focus(0);
    layout.settle();
    assert_eq!(layout.held, None);
}
