use super::*;

const DESK: (f32, f32) = (1400., 860.);

#[test]
fn rail_selection_focuses_existing_or_replaces_only_focused() {
    let mut layout = Layout::default();
    assert!(layout.select("chat"));
    let chat = layout.panes[0].instance;
    assert!(layout.split("files"));
    assert!(layout.select("chat"));
    assert_eq!(layout.focused, 0);
    assert_eq!(layout.panes[0].instance, chat);
    assert!(!layout.select("chat"));
    layout.place(DESK);
    let frame = layout.panes[0].frame;
    assert!(layout.select("calendar"));
    assert_eq!(layout.panes.len(), 2);
    assert_eq!(layout.panes[0].module, "calendar");
    assert_eq!(
        layout.panes[0].frame, frame,
        "a replaced view keeps its window"
    );
    assert_ne!(layout.panes[0].instance, chat);
    assert_eq!(layout.panes[1].module, "files");
}

#[test]
fn split_opens_on_top_with_unique_instances_and_limits_count() {
    let mut layout = Layout::default();
    assert!(layout.split("chat"));
    assert!(layout.split("files"));
    assert!(layout.focus(0));
    assert!(layout.split("chat"));
    assert_eq!(layout.focused, 1);
    assert_eq!(layout.panes[1].module, "chat");
    assert_eq!(
        *layout.stacking().last().unwrap(),
        1,
        "the new window is on top"
    );
    assert_ne!(layout.panes[0].instance, layout.panes[1].instance);
    while layout.split("calendar") {}
    assert_eq!(layout.panes.len(), MAX_PANES);
    assert!(!layout.focus(MAX_PANES));
}

#[test]
fn focus_raises_and_closing_focuses_the_window_left_on_top() {
    let mut layout = Layout::default();
    for module in ["chat", "files", "calendar"] {
        layout.split(module);
    }
    assert!(layout.focus(0));
    assert_eq!(layout.stacking(), vec![1, 2, 0]);
    for (index, pane) in layout.panes.iter_mut().enumerate() {
        pane.frame = Some(Frame::fill((800., 600.)));
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
fn popout_and_popin_transfer_instance_and_replace_at_capacity() {
    let mut source = Layout::default();
    source.split("chat");
    let original = source.panes[0].instance;
    let pane = source.close(0).unwrap();
    assert!(source.panes.is_empty());
    let mut destination = Layout::default();
    assert!(destination.popin(pane).is_none());
    assert_eq!(destination.panes[0].instance, original);
    while destination.split("files") {}
    destination.focus(1);
    let displaced = destination.panes[1].instance;
    let pane = Pane::new("chat");
    let incoming = pane.instance;
    assert_eq!(destination.popin(pane).unwrap().instance, displaced);
    assert_eq!(destination.panes.len(), MAX_PANES);
    assert_eq!(destination.panes[1].instance, incoming);
    assert!(destination.close(MAX_PANES).is_none());
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

#[test]
fn filling_a_window_and_filling_it_again_puts_it_back() {
    let mut layout = Layout::default();
    layout.split("chat");
    layout.place(DESK);
    let small = Frame {
        x: 100.,
        y: 80.,
        w: 500.,
        h: 400.,
    };
    layout.set_frame(0, small, DESK);
    layout.toggle_fill(0, DESK);
    assert_eq!(layout.panes[0].frame, Some(Frame::fill(DESK)));
    layout.toggle_fill(0, DESK);
    assert_eq!(layout.panes[0].frame, Some(small));
}

#[test]
fn halving_splits_the_focused_frame_into_an_empty_window() {
    let mut layout = Layout::default();
    assert!(layout.halve(false, DESK), "no window: an empty one");
    layout.place(DESK);
    layout.toggle_fill(0, DESK);
    assert!(layout.panes[0].is_empty());
    assert_eq!(layout.panes[0].frame, Some(Frame::fill(DESK)));
    layout.load("chat");
    assert_eq!(layout.panes[0].module, "chat");
    let whole = layout.panes[0].frame.unwrap();
    assert!(layout.halve(false, DESK));
    let (left, right) = (
        layout.panes[0].frame.unwrap(),
        layout.panes[1].frame.unwrap(),
    );
    assert_eq!(layout.focused, 1);
    assert!(layout.panes[1].is_empty());
    assert_eq!(
        (left.x, left.w + right.w, right.x),
        (whole.x, whole.w, whole.x + left.w)
    );
    assert_eq!((left.h, right.h), (whole.h, whole.h));
    assert!(layout.halve(true, DESK));
    let (top, bottom) = (
        layout.panes[1].frame.unwrap(),
        layout.panes[2].frame.unwrap(),
    );
    assert_eq!((top.h + bottom.h, bottom.y), (right.h, right.y + top.h));
    assert_eq!((top.x, bottom.x, bottom.w), (right.x, right.x, right.w));
    // a half narrower (or lower) than the smallest window isn't made
    assert!(layout.halve(false, DESK));
    let count = layout.panes.len();
    let narrow = layout.panes[layout.focused].frame.unwrap();
    assert!(narrow.w / 2. < MIN_WIDTH, "{narrow:?}");
    assert!(!layout.halve(false, DESK), "too narrow to halve");
    assert_eq!(layout.panes.len(), count);
    while layout.split("files") {}
    assert!(!layout.halve(false, DESK), "no room for another");
}

#[test]
fn closing_a_half_gives_its_sibling_the_whole_frame_back() {
    let mut layout = Layout::default();
    layout.split("chat");
    layout.place(DESK);
    let whole = layout.panes[0].frame.unwrap();
    layout.halve(false, DESK);
    let right = layout.panes[1].frame.unwrap();
    layout.halve(true, DESK);
    // the lower right quarter goes: the upper one takes the right half
    layout.close(2);
    assert_eq!(layout.panes[1].frame, Some(right));
    // the right half goes: chat takes the whole desk again
    layout.close(1);
    assert_eq!(layout.panes[0].frame, Some(whole));
    // a window placed on its own is left alone
    layout.split("files");
    layout.place(DESK);
    let cascaded = layout.panes[1].frame;
    layout.close(0);
    assert_eq!(layout.panes[0].frame, cascaded);
}

#[test]
fn closing_a_half_gives_the_quarters_beside_it_the_whole_width() {
    let mut layout = Layout::default();
    layout.split("chat");
    layout.place(DESK);
    let whole = layout.panes[0].frame.unwrap();
    layout.halve(false, DESK);
    layout.halve(true, DESK);
    let (top, bottom) = (
        layout.panes[1].frame.unwrap(),
        layout.panes[2].frame.unwrap(),
    );
    // the left half goes: the right column's quarters take its width
    layout.close(0);
    assert_eq!(
        layout.panes[0].frame,
        Some(Frame {
            x: whole.x,
            w: whole.w,
            ..top
        })
    );
    assert_eq!(
        layout.panes[1].frame,
        Some(Frame {
            x: whole.x,
            w: whole.w,
            ..bottom
        })
    );
    // and in the other direction: a row of halves under a closed top
    let mut layout = Layout::default();
    layout.split("chat");
    layout.place(DESK);
    layout.halve(true, DESK);
    layout.halve(false, DESK);
    layout.close(0);
    let (left, right) = (
        layout.panes[0].frame.unwrap(),
        layout.panes[1].frame.unwrap(),
    );
    assert_eq!(
        (left.y, left.h, right.y, right.h),
        (whole.y, whole.h, whole.y, whole.h)
    );
    // a window that spans only part of the side is left alone
    let mut layout = Layout::default();
    layout.split("chat");
    layout.place(DESK);
    layout.halve(false, DESK);
    layout.halve(true, DESK);
    let top = layout.panes[1].frame;
    layout.panes[2].frame = None;
    layout.close(0);
    assert_eq!(layout.panes[0].frame, top);
}

#[test]
fn a_resized_desk_scales_every_window_with_it() {
    let mut layout = Layout::default();
    layout.split("chat");
    layout.measure(DESK);
    layout.place(DESK);
    layout.toggle_fill(0, DESK);
    layout.halve(false, DESK);
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
    assert_eq!(left.x + left.w, right.x, "halves stay flush");
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
    assert_eq!((layout.panes.len(), layout.panes[0].module), (1, "chat"));
    assert_ne!(layout.panes[0].instance, empty);
    assert!(layout.open("files"), "a window of its own");
    assert_eq!(layout.panes.len(), 2);
    assert!(layout.open("chat"));
    assert_eq!((layout.panes.len(), layout.focused), (2, 0));
    // shift: replaces the focused view, as a plain click once did
    assert!(layout.select("calendar"));
    assert_eq!(layout.panes[0].module, "calendar");
    // a module open twice stays in the focused window
    layout.split("files");
    assert!(!layout.select("files"));
    assert_eq!(layout.focused, 1);
}

#[test]
fn cycling_visits_every_window_and_back_returns() {
    let mut layout = Layout::default();
    for module in ["chat", "files", "calendar"] {
        layout.split(module);
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
