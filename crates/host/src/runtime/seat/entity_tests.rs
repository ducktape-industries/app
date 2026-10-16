//! The seat, turned off the draw path, under a bare root that draws it as
//! `layers::PaneView` does: its tree, cached, or its standin.
use super::tests::eventually;
use super::*;
use gpui_kit::{
    Entity, IntoElement, ParentElement as _, Render, Styled as _, Subscription, TestAppContext,
    VisualTestContext, div, px, size,
};
use std::time::Duration;

/// A pane instance of this test's own, as `ui::layout::Pane::new` hands the
/// shell one: unique for the process.
fn instance() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Counts `entity`'s notifies while the subscription lives.
fn notifies<T: 'static>(
    entity: &Entity<T>,
    cx: &mut TestAppContext,
) -> (std::rc::Rc<std::cell::Cell<usize>>, gpui_kit::Subscription) {
    let seen = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = seen.clone();
    let observing = cx.update(|cx| cx.observe(entity, move |_, _| count.set(count.get() + 1)));
    (seen, observing)
}

struct Root {
    seat: Entity<Seat>,
    /// The desk's selection layer, whose sweep of a cached frame's
    /// paragraphs refreshed the window on every frame before #347.
    selection_layer: bool,
    /// `(ticks, frame_rev)` of the seated view at each draw that shows its
    /// tree: which of the view's frames the window drew, read off the log.
    drawn: Vec<(u64, u64)>,
    _watch: Subscription,
}

impl Render for Root {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::prelude::FluentBuilder as _;
        let seat = self.seat.read(cx);
        if seat.standin().is_none() && seat.tree().is_some() {
            let mounted = registry().lock().unwrap()[&(seat.program(), seat.instance())].clone();
            if let Slot::Ready(view) = &mounted.lock().unwrap().slot {
                self.drawn.push((view.ticks, view.frame_rev));
            }
        }
        let body = match (seat.standin(), seat.tree()) {
            (Some(standin), _) => standin.element(seat.program(), seat.instance(), seat.ax_mark()),
            // the cached path, no a11y reader
            (None, Some(tree)) => gpui_kit::AnyView::from(tree)
                .cached(gpui_kit::StyleRefinement::default().size_full())
                .into_any_element(),
            (None, None) => div().size_full().into_any_element(),
        };
        div()
            .size_full()
            .when(self.selection_layer, |root| {
                root.child(gpui_kit::base::TextSelectionLayer)
            })
            .child(body)
    }
}

/// A window whose root draws one seat of `program`, placed in it (so its
/// first turn is deferred, and has run once this returns).
fn open(
    cx: &mut TestAppContext,
    program: &'static str,
    selection_layer: bool,
) -> (Entity<Seat>, Entity<Root>, VisualTestContext) {
    cx.update(gpui_kit::init);
    another(cx, program, selection_layer)
}

/// [`open`] in an app already set up: one more window, one more seat.
fn another(
    cx: &mut TestAppContext,
    program: &'static str,
    selection_layer: bool,
) -> (Entity<Seat>, Entity<Root>, VisualTestContext) {
    let window = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let seat = cx.new(|cx| Seat::new(program, instance(), cx));
        seat.update(cx, |seat, cx| {
            seat.place(window.window_handle(), Some((400., 300.)), true, cx)
        });
        Root {
            seat: seat.clone(),
            selection_layer,
            drawn: Vec::new(),
            _watch: cx.observe(&seat, |_, _, cx| cx.notify()),
        }
    });
    let root = window.root(cx).unwrap();
    let seat = root.read_with(cx, |root, _| root.seat.clone());
    let native = VisualTestContext::from_window(window.into(), cx);
    native.run_until_parked();
    (seat, root, native)
}

/// The seat's registry entry.
fn mounted_of(seat: &Entity<Seat>, cx: &TestAppContext) -> Arc<Mutex<Mounted>> {
    let key = seat.read_with(cx, |seat, _| (seat.program(), seat.instance()));
    registry().lock().unwrap()[&key].clone()
}

fn ticks_of(seat: &Entity<Seat>, cx: &TestAppContext) -> u64 {
    match &mounted_of(seat, cx).lock().unwrap().slot {
        Slot::Ready(view) => view.ticks,
        _ => panic!("not seated"),
    }
}

fn turns_of(seat: &Entity<Seat>, cx: &TestAppContext) -> u64 {
    seat.read_with(cx, |seat, _| seat.turns)
}

fn renders_of(program: &str) -> u64 {
    crate::perf::snapshot(false)["views"][program]["renders"]
        .as_u64()
        .unwrap_or(0)
}

#[gpui_kit::test]
fn same_program_instances_receive_independent_props(cx: &mut TestAppContext) {
    // each in a pane with a body: a seat is not ticked before it has one
    let (first, _, _first) = open(cx, "independent-props-test", false);
    let (second, _, _second) = another(cx, "independent-props-test", false);
    // after both: it readies every seat the program has
    crate::runtime::seat_for_test("independent-props-test", 320);
    for (seat, props) in [(&first, b"channel-one"), (&second, b"channel-two")] {
        seat.update(cx, |seat, cx| {
            seat.set_props(props.to_vec(), cx);
            seat.turn(cx);
        });
    }
    cx.run_until_parked();
    // what the view is handed: `Mounted.props`, the slot `redraw` reads
    let props = |seat: &Entity<Seat>, cx: &TestAppContext| {
        mounted_of(seat, cx).lock().unwrap().props.clone()
    };
    assert_eq!(
        props(&first, cx).as_deref(),
        Some(b"channel-one".as_slice())
    );
    assert_eq!(
        props(&second, cx).as_deref(),
        Some(b"channel-two".as_slice())
    );
    cx.update(|cx| {
        let (first, second) = (first.read(cx), second.read(cx));
        assert_ne!(first.instance(), second.instance());
        let registry = registry().lock().unwrap();
        assert!(!Arc::ptr_eq(
            &registry[&(first.program(), first.instance())],
            &registry[&(second.program(), second.instance())]
        ));
    });
    first.update(cx, |seat, cx| {
        seat.set_props(b"channel-three".to_vec(), cx);
        seat.turn(cx);
    });
    cx.run_until_parked();
    assert_eq!(
        props(&first, cx).as_deref(),
        Some(b"channel-three".as_slice())
    );
    assert_eq!(
        props(&second, cx).as_deref(),
        Some(b"channel-two".as_slice())
    );
}

/// The root a view is drawn in is laid out from the view's own minimum.
#[gpui_kit::test]
fn a_view_is_laid_out_from_its_own_minimum(cx: &mut TestAppContext) {
    crate::runtime::seat_for_test("laid-out-from", 560);
    let (seat, _, _native) = open(cx, "laid-out-from", false);
    assert_eq!(seat.read_with(cx, |seat, _| seat.min_width()), 560.);
}

/// The idle rule of docs/perf.md, on the counters `GET /perf` serves: a
/// seated view at rest is drawn by the window every frame, but its tree
/// renders again only for a tick that changed it, so `renders ≤ ticks + 2`.
/// The view draws #347's tree — a paragraph in id-less boxes in a named
/// one — under the selection layer whose sweep of a cached frame's
/// paragraphs refreshed the window on every frame before #347: renders far
/// above ticks is that loop, or any other that keeps a cached tree dirty.
/// On the cached path, no a11y reader.
#[gpui_kit::test]
fn an_idle_view_renders_no_more_than_it_ticks(cx: &mut TestAppContext) {
    const PROGRAM: &str = "idle-renders-test";
    let bare = |children: Vec<wire::Node>| {
        wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: crate::render::test_style(div().p_2().style().clone()),
            interactivity: Default::default(),
            children,
        })
    };
    let paragraph = wire::Node::RichText {
        id: Some(wire::ElementIdWire::Name("line".into())),
        style: crate::render::test_style(div().h(px(20.)).style().clone()),
        text: "a line".into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    };
    let card = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("card".into())),
        style: crate::render::test_style(div().w(px(200.)).h(px(100.)).style().clone()),
        interactivity: Default::default(),
        children: vec![bare(vec![bare(vec![paragraph])])],
    });
    let _on = crate::perf::on_for_test();
    crate::runtime::seat_drawing_for_test(PROGRAM, 320, card);
    let (_, root, mut native) = open(cx, PROGRAM, true);
    // the desk redraws its panes on every frame it draws
    for _ in 0..8 {
        native.update(|window, cx| {
            root.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        });
        native.run_until_parked();
    }
    let counted = &crate::perf::snapshot(false)["views"][PROGRAM];
    let (ticks, renders) = (counted["ticks"].as_u64(), counted["renders"].as_u64());
    let (Some(ticks), Some(renders)) = (ticks, renders) else {
        panic!("the seat counts its ticks and its tree's renders: {counted}");
    };
    assert!(
        renders <= ticks + 2,
        "{renders} renders over {ticks} ticks: the tree is redrawn without a tick"
    );
}

/// A one-line field's caret blinks only while the field has the keys. gpui-base
/// 0.6.4 started the blink on the programmatic `set_value` a mount does and
/// never stopped it on a field nothing focused, so every view with a field
/// re-rendered twice a second at rest (the perf-breaches report's §1; upstream
/// gpui-kit #3138, fixed in 0.7.0 by #3139/#3140). With the window active: an
/// unfocused field renders its view 0 times over an idle 2 s, a focused one
/// renders once per caret toggle, and blurring it stops that again.
#[gpui_kit::test]
fn a_one_line_field_blinks_only_while_focused(cx: &mut TestAppContext) {
    const PROGRAM: &str = "blink-renders-test";
    let field = wire::Node::Field {
        options: Box::new(wire::InputOptions {
            label: "Filter members".into(),
            ..Default::default()
        }),
        id: wire::ElementIdWire::Name("filter".into()),
        multiline: false,
        cursor: wire::TextRange::caret("a value the mount sets".len()),
        generation: 1,
        revision: 0,
        tokens: Default::default(),
        claims: Default::default(),
        placeholder: "Filter by name".into(),
        value: "a value the mount sets".into(),
        on_change: Some(1),
        on_key: None,
        on_submit: None,
        secure: false,
        style: crate::render::test_style(div().w(px(200.)).h(px(24.)).style().clone()),
    };
    let card = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("card".into())),
        style: crate::render::test_style(div().w(px(240.)).h(px(100.)).style().clone()),
        interactivity: Default::default(),
        children: vec![field],
    });
    let _on = crate::perf::on_for_test();
    crate::runtime::seat_drawing_for_test(PROGRAM, 320, card);
    let (seat, root, mut native) = open(cx, PROGRAM, false);
    let frame = |native: &mut VisualTestContext| {
        native.update(|window, cx| {
            root.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        });
        native.run_until_parked();
    };
    // two seconds at rest, drawn every 100 ms as the desk draws its panes
    let idle = |native: &mut VisualTestContext| {
        let before = renders_of(PROGRAM);
        for _ in 0..20 {
            native.executor().advance_clock(Duration::from_millis(100));
            frame(native);
        }
        renders_of(PROGRAM) - before
    };
    for _ in 0..4 {
        frame(&mut native);
    }
    // the caret only shows in an active window
    native.update(|window, _| window.activate_window());
    frame(&mut native);
    assert!(native.update(|window, _| window.is_window_active()));
    let unfocused = idle(&mut native);

    native.update(|window, cx| window.focus_next(cx));
    frame(&mut native);
    let input = native.update(|_, cx| {
        let tree = seat.read(cx).tree().expect("mounted");
        tree.read(cx).first_input_for_test().expect("a field")
    });
    assert!(native.update(|window, cx| {
        gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window)
    }));
    let focused = idle(&mut native);

    native.update(|window, cx| window.blur(cx));
    frame(&mut native);
    let blurred = idle(&mut native);

    eprintln!("idle 2 s renders: unfocused {unfocused}, focused {focused}, blurred {blurred}");
    assert_eq!(unfocused, 0, "an unfocused field keeps an idle view still");
    assert!(
        focused >= 2,
        "a focused field's caret toggles: {focused} renders"
    );
    assert_eq!(blurred, 0, "blurring stops the blink");
}

/// A view out of budget (`frame.busy`) is turned again a frame later
/// (`BUSY_FRAME`), never back to back, and not a third time without a wake.
#[gpui_kit::test]
fn a_busy_frame_turns_once_per_frame(cx: &mut TestAppContext) {
    const PROGRAM: &str = "busy-frame-test";
    crate::runtime::seat_busy_for_test(PROGRAM, 320, 2);
    let (seat, _, _native) = open(cx, PROGRAM, false);
    assert_eq!(ticks_of(&seat, cx), 1);
    cx.run_until_parked();
    assert_eq!(
        ticks_of(&seat, cx),
        1,
        "a busy view is not turned back to back"
    );
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.run_until_parked();
    assert_eq!(ticks_of(&seat, cx), 2, "one more turn, a frame later");
    cx.run_until_parked();
    assert_eq!(ticks_of(&seat, cx), 2, "and not a third without a wake");
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.run_until_parked();
    assert_eq!(ticks_of(&seat, cx), 3, "the second busy frame's turn");
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.run_until_parked();
    assert_eq!(ticks_of(&seat, cx), 3, "a quiet frame arms nothing");
}

/// Props land as a turn only when their bytes moved.
#[gpui_kit::test]
fn a_seat_turns_when_its_props_move_and_not_otherwise(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let seat = cx.new(|cx| Seat::new("props-turn-test", instance(), cx));
    seat.update(cx, |seat, cx| seat.set_props(b"one".to_vec(), cx));
    cx.run_until_parked();
    assert_eq!(turns_of(&seat, cx), 1);
    seat.update(cx, |seat, cx| seat.set_props(b"one".to_vec(), cx));
    cx.run_until_parked();
    assert_eq!(turns_of(&seat, cx), 1, "the same bytes turn nothing");
    seat.update(cx, |seat, cx| seat.set_props(b"two".to_vec(), cx));
    cx.run_until_parked();
    assert_eq!(turns_of(&seat, cx), 2);
}

/// A load that lands wakes the seat once: the standin goes, the tree is
/// drawn on the next frame, and no frame was asked for per frame while the
/// load was on its way (the old `request_animation_frame` loop).
#[gpui_kit::test]
fn a_load_landing_replaces_the_standin_without_a_frame_loop(cx: &mut TestAppContext) {
    const PROGRAM: &str = "load-landing-test";
    let _on = crate::perf::on_for_test();
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    let draw = |native: &mut VisualTestContext| {
        native.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        native.run_until_parked();
    };
    draw(&mut native);
    assert!(
        seat.read_with(&native, |seat, _| seat.standin().is_some_and(|s| s.loading)),
        "a seat with nothing in it shows the load"
    );
    assert_eq!(
        native.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "a loading seat asks for no frame"
    );
    assert_eq!(renders_of(PROGRAM), 0);
    crate::runtime::seat_for_test(PROGRAM, 320);
    native.run_until_parked();
    assert!(
        seat.read_with(&native, |seat, _| seat.standin().is_none()
            && seat.tree().is_some()),
        "the landing woke the seat: the standin is gone and the tree is up"
    );
    assert_eq!(
        native.update(|window, cx| window.simulate_next_frame(cx)),
        0
    );
    draw(&mut native);
    draw(&mut native);
    assert_eq!(
        renders_of(PROGRAM),
        1,
        "the tree rendered once and is cached"
    );
}

/// A load that lands wakes the seat: `Load::land`'s install signals
/// `Mounted.wake`, which is what replaces the per-frame redraw loop in the
/// running app. No gpui task is involved: the loader's signal is read off
/// the watch directly.
#[test]
fn a_load_that_lands_wakes_the_seat() {
    const PROGRAM: &str = "install-wake-test";
    let mounted = Mounted::seat();
    let woke = mounted.lock().unwrap().wake.subscribe();
    let generation = mounted.lock().unwrap().start();
    let snapshot = connection().lock().unwrap().clone();
    queue(Load {
        program: PROGRAM,
        seat: mounted.clone(),
        generation,
        asked_of: snapshot,
        code: None,
    });
    eventually("the install signalled the seat's wake", || {
        woke.has_changed().unwrap()
    });
}

/// A stage the loader shows and a Retry each turn the seat once: the
/// stage words move the standin, the retry re-asks and turns.
#[gpui_kit::test]
fn a_stage_change_and_a_retry_each_turn_the_seat_once(cx: &mut TestAppContext) {
    const PROGRAM: &str = "stage-retry-test";
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    let (notifies, _watch) = notifies(&seat, &mut native);
    let mounted = mounted_of(&seat, &native);
    let turns = turns_of(&seat, &native);
    let words = |native: &TestAppContext| {
        seat.read_with(native, |seat, _| seat.standin().map(|s| s.words.clone()))
    };
    assert_eq!(words(&native).as_deref(), Some("Loading the view…"));

    mounted.lock().unwrap().show(
        0,
        Slot::Fetching {
            received: 12_000,
            total: None,
        },
    );
    native.run_until_parked();
    assert_eq!(turns_of(&seat, &native), turns + 1);
    assert_eq!(notifies.get(), 1);
    assert_eq!(words(&native).as_deref(), Some("Fetching the view — 12 KB"));

    mounted
        .lock()
        .unwrap()
        .show(0, Slot::Listed(Default::default()));
    native.run_until_parked();
    assert_eq!(turns_of(&seat, &native), turns + 2);
    assert_eq!(notifies.get(), 2);
    assert_eq!(words(&native).as_deref(), Some("Opening the view…"));

    mounted
        .lock()
        .unwrap()
        .show(0, Slot::Compiling(Default::default()));
    native.run_until_parked();
    assert_eq!(turns_of(&seat, &native), turns + 3);
    assert_eq!(notifies.get(), 3);
    assert_eq!(words(&native).as_deref(), Some("Compiling the view…"));

    // Retry: the load it starts fails at once (no node), so its signal and
    // the install's coalesce into the one turn the retry is
    let instance = seat.read_with(&native, |seat, _| seat.instance());
    retry(PROGRAM, instance);
    eventually("the retried load failed without a node", || {
        matches!(lock(&mounted).slot, Slot::Failed(_))
    });
    native.run_until_parked();
    assert_eq!(
        turns_of(&seat, &native),
        turns + 4,
        "a retry turns the seat once"
    );
    assert!(
        matches!(mounted.lock().unwrap().slot, Slot::Failed(_)),
        "the retried load failed without a node"
    );
}

/// A view whose frame both mounts a `TextInput` and asks `Focus` on it:
/// the input is created in the draw that mounts the tree, so the command
/// runs a frame after that draw, and the input has the keys after two
/// frames with no "focus handle is not mounted" refusal. Then the same
/// with the turn entered from an `on_next_frame` callback, where the
/// window is borrowed: `wake` defers it to app level.
#[gpui_kit::test]
fn widget_commands_run_after_the_tree_mounted(cx: &mut TestAppContext) {
    const PROGRAM: &str = "widget-commands-test";
    let field = wire::Node::Field {
        options: Box::new(wire::InputOptions {
            label: "Filter".into(),
            ..Default::default()
        }),
        id: wire::ElementIdWire::Name("filter".into()),
        multiline: false,
        cursor: Default::default(),
        generation: 1,
        revision: 0,
        tokens: Default::default(),
        claims: Default::default(),
        placeholder: String::new(),
        value: String::new(),
        on_change: Some(1),
        on_key: None,
        on_submit: None,
        secure: false,
        style: crate::render::test_style(div().w(px(200.)).h(px(24.)).style().clone()),
    };
    let focus_it = |instance| {
        let registry = registry().lock().unwrap();
        let Slot::Ready(view) = &mut registry[&(PROGRAM, instance)].lock().unwrap().slot else {
            panic!("seated");
        };
        view.widget_commands.push((
            7,
            wire::WidgetCommand::Focus {
                target: vec![wire::ElementIdWire::Name("filter".into())],
            },
        ));
    };
    crate::runtime::seat_drawing_for_test(PROGRAM, 320, field);
    // queued before the first turn: that turn's frame mounts the input
    focus_it(0);
    // the turn and the frame's first callbacks in one App update, before
    // the harness's flush-time draw: only a callback nested in the first
    // waits for the draw that mounts the input. A single callback answers
    // the Focus `Ok` and loses the focus silently, so the assertion is on
    // the focus itself, not the reply.
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
        let seat = cx.new(|cx| Seat::new(PROGRAM, instance(), cx));
        Root {
            seat: seat.clone(),
            selection_layer: false,
            drawn: Vec::new(),
            _watch: cx.observe(&seat, |_, _, cx| cx.notify()),
        }
    });
    let root = window.root(cx).unwrap();
    let seat = root.read_with(cx, |root, _| root.seat.clone());
    let handle: gpui_kit::AnyWindowHandle = window.into();
    cx.update(|cx| {
        seat.update(cx, |seat, cx| {
            // the pane in front, as the pane layer would say
            seat.set_keys_free(true, cx);
            seat.place(handle, Some((400., 300.)), true, cx);
            seat.turn(cx);
        });
        handle
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
    });
    let mut native = VisualTestContext::from_window(handle, cx);
    native.run_until_parked();
    let input_focused = |native: &mut VisualTestContext| {
        native.update(|window, cx| {
            let tree = seat.read(cx).tree().expect("mounted");
            let input = tree.read(cx).first_input_for_test().expect("a field");
            gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window)
        })
    };
    // each frame's callbacks and its draw in their own updates
    let two_frames = |native: &mut VisualTestContext| {
        for _ in 0..2 {
            native.update(|window, cx| {
                window.simulate_next_frame(cx);
            });
            native.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
            native.run_until_parked();
        }
    };
    two_frames(&mut native);
    assert!(
        input_focused(&mut native),
        "the Focus ran before the draw that mounts its input"
    );

    native.update(|window, cx| window.blur(cx));
    assert!(!input_focused(&mut native));
    let instance = seat.read_with(&native, |seat, _| seat.instance());
    focus_it(instance);
    let woken = seat.clone();
    native.update(|window, _| {
        window.on_next_frame(move |_, cx| {
            woken.update(cx, |seat, cx| seat.wake(cx));
        });
    });
    // the wake's callback, then the deferred turn at app level
    native.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    native.run_until_parked();
    two_frames(&mut native);
    assert!(
        input_focused(&mut native),
        "a turn woken from a window callback still runs its commands"
    );
}

/// A view moves the keys only while its keys are free (`Seat::keys_free`,
/// as `PaneLayer` says it: its pane in front, nothing open over the desk,
/// no hold), on the real command path, and is judged as the command RUNS:
/// a `Focus` or `FocusHandle` asked while the keys were free and run once
/// Spotlight or Approve opened over the desk takes nothing.
#[gpui_kit::test]
fn a_view_moves_the_keys_only_while_its_keys_are_free(cx: &mut TestAppContext) {
    const PROGRAM: &str = "focus-gate-test";
    let field = wire::Node::Field {
        options: Box::new(wire::InputOptions {
            label: "Filter".into(),
            ..Default::default()
        }),
        id: wire::ElementIdWire::Name("filter".into()),
        multiline: false,
        cursor: Default::default(),
        generation: 1,
        revision: 0,
        tokens: Default::default(),
        claims: Default::default(),
        placeholder: String::new(),
        value: String::new(),
        on_change: Some(1),
        on_key: None,
        on_submit: None,
        secure: false,
        style: crate::render::test_style(div().w(px(200.)).h(px(24.)).style().clone()),
    };
    let mut button = view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("button".into())),
        style: crate::render::test_style(div().w(px(200.)).h(px(24.)).style().clone()),
        interactivity: Default::default(),
        children: Vec::new(),
    };
    button.interactivity.get_or_insert_default().focus_handle = Some(9);
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("root".into())),
        style: crate::render::plain_style(),
        interactivity: Default::default(),
        children: vec![field, wire::Node::Container(button)],
    });
    crate::runtime::seat_drawing_for_test(PROGRAM, 320, root);
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    let queue = |seat: &Entity<Seat>, native: &mut VisualTestContext, command| {
        let mounted = mounted_of(seat, native);
        let mut locked = mounted.lock().unwrap();
        let Slot::Ready(view) = &mut locked.slot else {
            panic!("seated")
        };
        view.widget_commands.push((7, command));
    };
    let run = |seat: &Entity<Seat>, native: &mut VisualTestContext| {
        let woken = seat.clone();
        native.update(|window, _| {
            window.on_next_frame(move |_, cx| {
                woken.update(cx, |seat, cx| seat.wake(cx));
            });
        });
        native.update(|window, cx| window.simulate_next_frame(cx));
        native.run_until_parked();
        for _ in 0..2 {
            native.update(|window, cx| window.simulate_next_frame(cx));
            native.update(|window, cx| window.draw(cx).clear(cx));
            native.run_until_parked();
        }
    };
    let focus = wire::WidgetCommand::Focus {
        target: vec![
            wire::ElementIdWire::Name("root".into()),
            wire::ElementIdWire::Name("filter".into()),
        ],
    };
    let handle = wire::WidgetCommand::FocusHandle { handle: 9 };
    let input_focused = |native: &mut VisualTestContext| {
        native.update(|window, cx| {
            let tree = seat.read(cx).tree().expect("mounted");
            let input = tree.read(cx).first_input_for_test().expect("a field");
            gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window)
        })
    };
    let button_focused = |native: &mut VisualTestContext| {
        native.update(|window, cx| {
            let tree = seat.read(cx).tree().expect("mounted");
            tree.read(cx)
                .view_focus_for_test(9)
                .expect("drawn")
                .is_focused(window)
        })
    };
    let keys_free = |native: &mut VisualTestContext, free: bool| {
        seat.update(native, |seat, cx| seat.set_keys_free(free, cx));
    };
    // a fresh seat's keys are not free until the pane layer says so
    assert!(!seat.read_with(&native, |seat, _| seat.keys_free()));
    queue(&seat, &mut native, focus.clone());
    run(&seat, &mut native);
    assert!(
        !input_focused(&mut native),
        "Focus took the keys from a back pane"
    );
    queue(&seat, &mut native, handle.clone());
    run(&seat, &mut native);
    assert!(
        !button_focused(&mut native),
        "FocusHandle took the keys from a back pane"
    );
    keys_free(&mut native, true);
    queue(&seat, &mut native, handle);
    run(&seat, &mut native);
    assert!(
        button_focused(&mut native),
        "a front pane's FocusHandle was refused"
    );
    native.update(|window, cx| window.blur(cx));
    queue(&seat, &mut native, focus.clone());
    run(&seat, &mut native);
    assert!(
        input_focused(&mut native),
        "a front pane's Focus was refused"
    );
    native.update(|window, cx| window.blur(cx));
    // asked with the keys free, run after something opened over the desk
    queue(&seat, &mut native, focus);
    keys_free(&mut native, false);
    run(&seat, &mut native);
    assert!(
        !input_focused(&mut native),
        "a Focus asked before Spotlight opened took the keys from it"
    );
}

/// The host's stamp reaches the view: a press or key the tree received
/// (`ViewTree::activate`) is the view's activation at its next turn, and
/// the tree's is taken (one input, one stamp).
#[gpui_kit::test]
fn a_trees_activation_reaches_its_view_on_the_next_turn(cx: &mut TestAppContext) {
    const PROGRAM: &str = "activation-carry-test";
    crate::runtime::seat_for_test(PROGRAM, 320);
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    let tree = seat.read_with(&native, |seat, _| seat.tree().expect("mounted"));
    let view_activation = |native: &VisualTestContext| {
        let mounted = mounted_of(&seat, native);
        let locked = mounted.lock().unwrap();
        let Slot::Ready(view) = &locked.slot else {
            panic!("seated")
        };
        view.activation
    };
    assert!(
        view_activation(&native).is_none(),
        "activated before any input"
    );
    tree.update(&mut native, |tree, _| tree.activate());
    seat.update(&mut native, |seat, cx| seat.wake(cx));
    native.run_until_parked();
    assert!(
        view_activation(&native).is_some(),
        "the tree's activation did not reach the view"
    );
    assert!(
        tree.read_with(&native, |tree, _| tree.take_activation().is_none()),
        "the tree kept the stamp it handed over"
    );
}

/// A view's own `host.widget` cursor command on its field is no input: the
/// view ends its turn with no activation to spend on the clipboard or a link.
#[gpui_kit::test]
fn a_views_own_cursor_command_grants_it_no_activation(cx: &mut TestAppContext) {
    const PROGRAM: &str = "self-stamp-test";
    let target = vec![wire::ElementIdWire::Name("document".into())];
    let root = wire::Node::Field {
        id: target[0].clone(),
        multiline: true,
        value: "some words".into(),
        cursor: Default::default(),
        generation: 1,
        revision: 0,
        tokens: Default::default(),
        claims: Default::default(),
        options: Default::default(),
        placeholder: String::new(),
        secure: false,
        on_change: Some(1),
        on_key: None,
        on_submit: None,
        style: crate::render::test_style(div().w(px(240.)).h(px(80.)).style().clone()),
    };
    crate::runtime::seat_drawing_for_test(PROGRAM, 320, root);
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    let with_view = |native: &VisualTestContext, f: &mut dyn FnMut(&mut View)| {
        let mounted = mounted_of(&seat, native);
        let mut locked = mounted.lock().unwrap();
        let Slot::Ready(view) = &mut locked.slot else {
            panic!("seated")
        };
        f(view)
    };
    native.run_until_parked();
    let mut activation = None;
    with_view(&native, &mut |view| {
        view.widget_commands.push((
            7,
            wire::WidgetCommand::SelectAll {
                target: target.clone(),
            },
        ));
    });
    seat.update(&mut native, |seat, cx| seat.wake(cx));
    for _ in 0..3 {
        native.update(|window, cx| window.simulate_next_frame(cx));
        native.update(|window, cx| window.draw(cx).clear(cx));
        native.run_until_parked();
    }
    with_view(&native, &mut |view| {
        assert!(view.widget_commands.is_empty(), "SelectAll was never run");
        activation = view.activation;
    });
    assert!(
        activation.is_none(),
        "the view's own SelectAll gave it an activation"
    );
}

/// A program that leaves the roster gives up its seat: the pane shows the
/// "no view" standin, not its frozen tree: the retire itself wakes the
/// seat, since no clock redraws the window.
#[gpui_kit::test]
fn a_retired_program_shows_no_view(cx: &mut TestAppContext) {
    const PROGRAM: &str = "retired-test";
    crate::runtime::seat_for_test(PROGRAM, 320);
    let (seat, _, native) = open(cx, PROGRAM, false);
    assert!(seat.read_with(&native, |s, _| s.tree().is_some() && s.standin().is_none()));
    mounted_of(&seat, &native).lock().unwrap().retire();
    native.run_until_parked();
    let words = seat.read_with(&native, |s, _| s.standin().map(|s| s.words.clone()));
    assert!(
        words.as_deref().is_some_and(|w| w.contains("has no")),
        "the retired seat still shows its tree: {words:?}"
    );
}

/// A load that panics fails as any failed load does: the pane names the
/// failure and offers Retry, where it showed "Loading" for good, and the
/// panic stops at the loader, which goes on to the next load.
#[gpui_kit::test]
fn a_load_that_panics_shows_the_failure_with_retry(cx: &mut TestAppContext) {
    const PROGRAM: &str = "panicking-load-test";
    let (seat, _, native) = open(cx, PROGRAM, false);
    let mounted = mounted_of(&seat, &native);
    // asked as `retry` asks: the seat's wake is signalled here, on the
    // test's thread, so the loader's signal finds no waker to run off-thread
    let generation = {
        let mut locked = lock(&mounted);
        locked.wake.send_replace(());
        locked.start()
    };
    let load = Load {
        program: PROGRAM,
        seat: mounted,
        generation,
        asked_of: connection().lock().unwrap().clone(),
        code: None,
    };
    let landed = std::thread::spawn(move || load.land(|_, _| panic!("a loader bug"))).join();
    assert!(landed.is_ok(), "the panic got past the loader");
    native.run_until_parked();
    let standin = seat.read_with(&native, |s, _| s.standin().cloned());
    assert!(
        standin
            .as_ref()
            .is_some_and(|s| s.retry && s.words.contains("a loader bug")),
        "{standin:?}"
    );
}

/// A loader that panics holding a seat's lock poisons it. The window
/// thread's next turn still takes the lock and draws what the seat holds,
/// where an `expect` on it panicked, and took every window with it.
#[gpui_kit::test]
fn a_poisoned_seat_still_draws(cx: &mut TestAppContext) {
    const PROGRAM: &str = "poisoned-seat-test";
    crate::runtime::seat_for_test(PROGRAM, 320);
    let (seat, _, native) = open(cx, PROGRAM, false);
    let mounted = mounted_of(&seat, &native);
    let held = mounted.clone();
    let _ = std::thread::spawn(move || {
        let _held = held.lock();
        panic!("a loader panicked holding the seat");
    })
    .join();
    assert!(mounted.is_poisoned());
    lock(&mounted).retire();
    native.run_until_parked();
    let words = seat.read_with(&native, |s, _| s.standin().map(|s| s.words.clone()));
    assert!(
        words.as_deref().is_some_and(|w| w.contains("has no")),
        "the poisoned seat drew nothing new: {words:?}"
    );
}

/// A link's route to a view that is already open (`Layout::open` only
/// focuses it): the seat is woken, so the route does not wait for an
/// unrelated turn.
#[gpui_kit::test]
fn a_route_reaches_an_open_view(cx: &mut TestAppContext) {
    const PROGRAM: &str = "route-test";
    crate::runtime::seat_for_test(PROGRAM, 320);
    let (seat, _, native) = open(cx, PROGRAM, false);
    let before = ticks_of(&seat, &native);
    {
        let mounted = mounted_of(&seat, &native);
        let mut locked = mounted.lock().unwrap();
        let Slot::Ready(view) = &mut locked.slot else {
            panic!("seated")
        };
        view.route_subscriptions.push(99);
    }
    let instance = seat.read_with(&native, |seat, _| seat.instance());
    crate::runtime::route_to(PROGRAM, instance, "somewhere".into());
    native.run_until_parked();
    assert_eq!(
        ticks_of(&seat, &native),
        before + 1,
        "the route was not delivered to the open view"
    );
}

/// A clipboard answer is pushed by `clipboard::mount` after `redraw` has
/// decided whether the view wants another frame: the answer still counts.
#[gpui_kit::test]
fn a_clipboard_answer_reaches_the_view(cx: &mut TestAppContext) {
    const PROGRAM: &str = "clipboard-test";
    let frame = |requests| wire::Frame {
        root: Some(wire::Node::empty()),
        requests,
        ..Default::default()
    };
    let ask = frame(vec![wire::Request {
        id: 5,
        kind: "clipboard.read".into(),
        payload: Vec::new(),
    }]);
    let code = crate::runtime::frames_code(&ask, 1, &frame(Vec::new()));
    crate::runtime::seat_code_for_test(PROGRAM, 320, code);
    {
        let registry = registry().lock().unwrap();
        let Slot::Ready(view) = &mut registry[&(PROGRAM, 0)].lock().unwrap().slot else {
            panic!("seated")
        };
        view.capabilities.push(Capability::Clipboard);
        // the read is asked on the first redraw, under a fresh activation
        view.activation = Some(std::time::Instant::now());
    }
    let (seat, _, native) = open(cx, PROGRAM, false);
    native.run_until_parked();
    native.executor().advance_clock(Duration::from_millis(100));
    native.run_until_parked();
    assert_eq!(
        ticks_of(&seat, &native),
        2,
        "the clipboard answer was delivered in a second tick"
    );
}

/// One tick per draw: two wakes in one flush (moved props and a route)
/// tick twice, but the second tick waits until the first's frame has
/// drawn, so every frame the view produced is drawn once, in order, and
/// none is skipped past on the way to the draw.
#[gpui_kit::test]
fn a_seat_ticks_once_per_draw_of_its_tree(cx: &mut TestAppContext) {
    const PROGRAM: &str = "fresh-draws-first-test";
    let _on = crate::perf::on_for_test();
    crate::runtime::seat_for_test(PROGRAM, 320);
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    native.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    native.run_until_parked();
    assert_eq!((ticks_of(&seat, &native), renders_of(PROGRAM)), (1, 1));
    {
        let mounted = mounted_of(&seat, &native);
        let mut locked = mounted.lock().unwrap();
        let Slot::Ready(view) = &mut locked.slot else {
            panic!("seated")
        };
        view.props_subscription = Some(98);
        view.route_subscriptions.push(99);
    }
    // the props turn is deferred; the route lands after it has run, still
    // before any draw, and asks for a turn of its own
    let routed = seat.clone();
    native.update(|_, cx| {
        seat.update(cx, |seat, cx| seat.set_props(b"moved".to_vec(), cx));
        let instance = routed.read(cx).instance();
        cx.defer(move |cx| {
            crate::runtime::route_to(PROGRAM, instance, "somewhere".into());
            routed.update(cx, |seat, cx| seat.turn(cx));
        });
    });
    native.run_until_parked();
    for _ in 0..2 {
        native.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        native.run_until_parked();
    }
    assert_eq!(
        ticks_of(&seat, &native),
        3,
        "the props and the route each ticked the view"
    );
    assert_eq!(
        renders_of(PROGRAM),
        3,
        "each fresh frame drew before the next turn replaced it"
    );
}

/// A tick the view answers `unchanged` gives the tree nothing to draw: the
/// tree is not re-rendered for it (before, every tick notified the tree, so
/// an idle view's whole tree was rebuilt per clock item and per reply with
/// nothing to show), and the seat holds nothing for it, so the next wake
/// still turns.
#[gpui_kit::test]
fn an_unchanged_tick_redraws_nothing_and_holds_nothing(cx: &mut TestAppContext) {
    const PROGRAM: &str = "unchanged-tick-test";
    let _on = crate::perf::on_for_test();
    crate::runtime::seat_ticking_for_test(
        PROGRAM,
        320,
        &[
            wire::Frame {
                root: Some(wire::Node::empty()),
                ..Default::default()
            },
            wire::Frame {
                unchanged: true,
                ..Default::default()
            },
        ],
    );
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    native.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    native.run_until_parked();
    assert_eq!((ticks_of(&seat, &native), renders_of(PROGRAM)), (1, 1));
    // a props move ticks only a view subscribed to them (`redraw`'s
    // `quiet` gate): subscribe it, as `a_seat_ticks_once_per_draw_of_its_tree`
    // does
    {
        let mounted = mounted_of(&seat, &native);
        let mut locked = mounted.lock().unwrap();
        let Slot::Ready(view) = &mut locked.slot else {
            panic!("seated")
        };
        view.props_subscription = Some(98);
    }
    // no draw asked for here: the harness draws a window only once
    // something dirtied it (a notified tree did, before)
    for props in [b"one".as_slice(), b"two".as_slice()] {
        native.update(|_, cx| {
            seat.update(cx, |seat, cx| seat.set_props(props.to_vec(), cx));
        });
        native.run_until_parked();
    }
    assert_eq!(
        ticks_of(&seat, &native),
        3,
        "moved props tick the view; an unchanged tick holds no turn back"
    );
    assert_eq!(renders_of(PROGRAM), 1, "an unchanged tick dirties nothing");
    // a draw asked for by something else leaves the cached tree alone too
    native.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    native.run_until_parked();
    assert_eq!(
        renders_of(PROGRAM),
        1,
        "an unchanged tick does not re-render the tree"
    );
}

/// A 24 px row with nothing in it.
fn row() -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: crate::render::test_style(div().h(px(24.)).w_full().style().clone()),
        interactivity: Default::default(),
        children: vec![],
    })
}

/// A 2,000-row uniform list filling the view, carrying `rows` and, as the
/// view always does, the measurement row 0.
fn uniform(rows: std::ops::Range<u32>) -> wire::Node {
    let id = wire::ElementIdWire::Name("rows".into());
    let mut indices: Vec<u32> = rows.collect();
    if !indices.contains(&0) {
        indices.insert(0, 0);
    }
    wire::Node::UniformList {
        id: id.clone(),
        path: vec![id],
        route: 1,
        style: crate::render::test_style(div().size_full().style().clone()),
        interactivity: Default::default(),
        count: 2_000,
        measure_index: 0,
        sizing: wire::list::UniformListSizing::Auto,
        horizontal_sizing: wire::list::UniformListHorizontalSizing::FitList,
        y_flipped: false,
        scroll_request: None,
        revision: 0,
        children: indices.iter().map(|_| row()).collect(),
        indices,
    }
}

/// A uniform list's first frame carries one row. The layout that draws it
/// asks for the rows (a tick, drawn next) and reports the list's scroll
/// state (`UniformListState`), a third tick the view answers `unchanged`:
/// that one draws nothing, so the open costs two draws, the one-row frame
/// and the rows. Before, every tick notified the tree, and the state tick
/// drew the same rows a third time.
#[gpui_kit::test]
fn a_uniform_lists_open_draws_its_two_frames_and_nothing_for_its_state(cx: &mut TestAppContext) {
    const PROGRAM: &str = "uniform-open-test";
    let _on = crate::perf::on_for_test();
    let full = |root| wire::Frame {
        root: Some(root),
        ..Default::default()
    };
    crate::runtime::seat_ticking_for_test(
        PROGRAM,
        320,
        &[
            full(uniform(0..1)),
            full(uniform(0..16)),
            wire::Frame {
                unchanged: true,
                ..Default::default()
            },
        ],
    );
    let (seat, root, native) = open(cx, PROGRAM, false);
    native.run_until_parked();
    assert_eq!(
        root.read_with(&native, |root, _| root.drawn.clone()),
        [(1, 1), (2, 2)],
        "draw 1 showed the one-row frame, draw 2 the rows asked for, and nothing drew for the state tick"
    );
    assert_eq!((ticks_of(&seat, &native), renders_of(PROGRAM)), (3, 2));
}

/// A seat claimed or dropped moves what `Roster::rail` reads, so each wakes
/// the rail; so do a retry (its row reads Loading again), the load it
/// starts (its row leaves Loading) and a roster read that moved the list. The only test that installs the app's
/// channel: another test's seat may send into it meanwhile, which can pass
/// this by accident but never fail it.
#[gpui_kit::test]
fn a_seat_claimed_or_dropped_wakes_the_rail(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let mut changes = crate::runtime::changes_channel();
    let mut woken = move || {
        let mut n = 0;
        while changes.try_recv().is_ok() {
            n += 1;
        }
        n
    };
    let seat = cx.new(|cx| Seat::new("rail-wake-seat", instance(), cx));
    assert!(woken() > 0, "a claimed seat told the rail nothing");
    let key = seat.read_with(cx, |seat, _| (seat.program(), seat.instance()));
    // a retry (Loading again) and the load it starts (no node: a failure
    // installed) each tell the rail
    let mounted = registry().lock().unwrap()[&key].clone();
    retry(key.0, key.1);
    eventually("the retried load failed without a node", || {
        matches!(lock(&mounted).slot, Slot::Failed(_))
    });
    assert!(
        woken() >= 2,
        "a retry and its load did not both tell the rail"
    );
    drop(seat);
    cx.update(|_| {});
    cx.run_until_parked();
    assert!(!registry().lock().unwrap().contains_key(&key));
    assert!(woken() > 0, "a dropped seat told the rail nothing");
    // a roster read that lists something else tells the rail too
    let listed = crate::backend::views::Program {
        name: "rail-wake-listed".into(),
        code: abi::BlobId::Sha256([0; 32]),
        bare: false,
    };
    crate::runtime::roster::relist(&Roster::default(), vec![listed]);
    assert!(woken() > 0, "a changed roster told the rail nothing");
}

// ---------- the landed-later gate (SPEC §13.3) ----------
//
// The seat's one path for a tick that lands later than its turn, run on
// Linux through `DeferredRunner`: the wasmtime runner whose answers land
// when the test's pump releases them (at once, on a later release, or
// never), as a JSC page's do across its process boundary.

use crate::runner::deferred::{DeferredRunner, Pump};

/// A frame whose tree is a box named `key`: each one a tree of its own.
fn keyed(key: &str) -> wire::Frame {
    wire::Frame {
        root: Some(wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name(key.into())),
            style: crate::render::test_style(div().size_full().style().clone()),
            interactivity: Default::default(),
            children: Vec::new(),
        })),
        ..Default::default()
    }
}

/// Five frames, each a tree of its own, the last on every tick after.
fn five_trees() -> Vec<wire::Frame> {
    ["one", "two", "three", "four", "five"]
        .into_iter()
        .map(keyed)
        .collect()
}

/// A seat of `program` placed in its window, its view answering `frames`
/// in order on a [`DeferredRunner`], which lands at once until its pump is
/// told to hold.
fn deferred(
    cx: &mut TestAppContext,
    program: &'static str,
    frames: &[wire::Frame],
) -> (Entity<Seat>, Entity<Root>, VisualTestContext, Pump) {
    let runner = DeferredRunner::default();
    let pump = runner.pump.clone();
    let runner: Arc<dyn ViewRunner> = Arc::new(runner);
    crate::runtime::seat_ticking_on_for_test(&runner, program, 320, frames);
    let (seat, root, native) = open(cx, program, false);
    (seat, root, native, pump)
}

/// [`deferred`]'s seat on the installed (wasmtime) runner, in the app
/// [`deferred`] set up.
fn on_wasmtime(
    cx: &mut TestAppContext,
    program: &'static str,
    frames: &[wire::Frame],
) -> (Entity<Seat>, Entity<Root>, VisualTestContext) {
    crate::runtime::seat_ticking_for_test(program, 320, frames);
    another(cx, program, false)
}

/// The window draws, and whatever the draw lets go of runs.
fn draw_now(native: &mut VisualTestContext) {
    native.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    native.run_until_parked();
}

fn in_flight(seat: &Entity<Seat>, cx: &TestAppContext) -> bool {
    seat.read_with(cx, |seat, _| seat.in_flight())
}

fn frame_rev_of(seat: &Entity<Seat>, cx: &TestAppContext) -> u64 {
    match &mounted_of(seat, cx).lock().unwrap().slot {
        Slot::Ready(view) => view.frame_rev,
        _ => panic!("not seated"),
    }
}

/// Moves the seat's props: a turn with something to deliver.
fn nudge(seat: &Entity<Seat>, props: &[u8], native: &mut VisualTestContext) {
    native.update(|_, cx| seat.update(cx, |seat, cx| seat.set_props(props.to_vec(), cx)));
    native.run_until_parked();
}

/// The view listens for its props, so moved props are a tick's worth.
fn subscribe_props(seat: &Entity<Seat>, cx: &TestAppContext) {
    let mounted = mounted_of(seat, cx);
    let mut locked = mounted.lock().unwrap();
    let Slot::Ready(view) = &mut locked.slot else {
        panic!("seated")
    };
    view.props_subscription = Some(98);
}

/// A turn asked for while a tick is in flight waits, however often it is
/// asked, and runs once after the frame lands (and draws); the landing
/// finishes the turn that began the tick, the tree shows the landed frame.
#[gpui_kit::test]
fn a_turn_queued_while_a_tick_is_in_flight_runs_once_after_the_landing(cx: &mut TestAppContext) {
    const PROGRAM: &str = "deferred-queued-turn";
    let (seat, _, mut native, pump) = deferred(cx, PROGRAM, &five_trees());
    draw_now(&mut native);
    assert_eq!(
        ticks_of(&seat, &native),
        1,
        "the first frame landed at once"
    );
    subscribe_props(&seat, &native);
    pump.hold();
    nudge(&seat, b"one", &mut native);
    assert!(in_flight(&seat, &native), "the tick waits for its frame");
    assert_eq!((pump.held(), ticks_of(&seat, &native)), (1, 1));
    let turns = turns_of(&seat, &native);
    let rev = frame_rev_of(&seat, &native);
    // asked for three times while the frame is out: no turn runs
    nudge(&seat, b"two", &mut native);
    native.update(|_, cx| {
        seat.update(cx, |seat, cx| {
            seat.turn(cx);
            seat.turn(cx);
        })
    });
    native.run_until_parked();
    assert_eq!(
        turns_of(&seat, &native),
        turns,
        "a turn ran with a tick in flight"
    );
    assert_eq!(
        pump.held(),
        1,
        "a second tick began before the first landed"
    );
    // the frame lands: the turn finishes, the tree draws it, the owed turn
    // runs once and begins the next tick (moved props to deliver)
    assert_eq!(pump.release_all(), 1);
    native.run_until_parked();
    assert_eq!(ticks_of(&seat, &native), 2);
    assert!(
        frame_rev_of(&seat, &native) > rev,
        "the landed frame was not taken in"
    );
    draw_now(&mut native);
    assert_eq!(
        turns_of(&seat, &native),
        turns + 1,
        "the owed turn ran once"
    );
    assert_eq!(pump.held(), 1, "the owed turn delivered the moved props");
    assert_eq!(pump.release_all(), 1);
    draw_now(&mut native);
    assert_eq!(ticks_of(&seat, &native), 3);
    assert!(!in_flight(&seat, &native));
    assert_eq!(pump.held(), 0, "nothing was left to deliver");
}

/// The desktop's order: a seat on the wasmtime runner, and one on the
/// deferred runner with every answer landing at once, draw the frames dev
/// drew before the runner seam (recorded off dev 309ea554's seat by this
/// same script: `DEV`), in that order, with its turns, ticks and renders.
#[gpui_kit::test]
fn the_desktop_order_is_unchanged_when_every_answer_lands_at_once(cx: &mut TestAppContext) {
    fn script(
        program: &str,
        seat: &Entity<Seat>,
        root: &Entity<Root>,
        native: &mut VisualTestContext,
    ) -> (Vec<(u64, u64)>, u64, u64, u64) {
        draw_now(native);
        subscribe_props(seat, native);
        for props in [b"a", b"b", b"c"] {
            nudge(seat, props, native);
            draw_now(native);
        }
        // two wakes before one draw: one ticks, the other waits for it
        nudge(seat, b"d", native);
        nudge(seat, b"e", native);
        draw_now(native);
        draw_now(native);
        (
            root.read_with(native, |root, _| root.drawn.clone()),
            turns_of(seat, native),
            ticks_of(seat, native),
            renders_of(program),
        )
    }
    // (drawn (ticks, frame_rev) per draw, turns, ticks, renders) on dev
    const DEV: ([(u64, u64); 12], u64, u64, u64) = (
        [
            (1, 1),
            (1, 1),
            (2, 2),
            (2, 2),
            (3, 3),
            (3, 3),
            (4, 4),
            (4, 4),
            (5, 5),
            (6, 6),
            (6, 6),
            (6, 6),
        ],
        6,
        6,
        6,
    );
    let dev = (DEV.0.to_vec(), DEV.1, DEV.2, DEV.3);
    let _on = crate::perf::on_for_test();
    let (seat, root, mut native, pump) = deferred(cx, "order-on-deferred", &five_trees());
    let at_once = script("order-on-deferred", &seat, &root, &mut native);
    assert_eq!(pump.held(), 0);
    let wasmtime = {
        let (seat, root, mut native) = on_wasmtime(cx, "order-on-wasmtime", &five_trees());
        script("order-on-wasmtime", &seat, &root, &mut native)
    };
    assert_eq!(wasmtime, dev, "the desktop's order moved from dev's");
    assert_eq!(at_once, dev, "answers landing at once moved the order");
}

/// On the desktop a tick lands inside the turn that began it
/// (`landed_now`): the turn takes its frame in before it returns, and
/// nothing is left in flight for a later poll.
#[gpui_kit::test]
fn a_desktop_tick_lands_inside_its_turn(cx: &mut TestAppContext) {
    const PROGRAM: &str = "desktop-inside-turn";
    crate::runtime::seat_ticking_for_test(PROGRAM, 320, &five_trees());
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    draw_now(&mut native);
    let mounted = mounted_of(&seat, &native);
    let ticks = ticks_of(&seat, &native);
    native.update(|_, cx| {
        match &mut mounted.lock().unwrap().slot {
            Slot::Ready(view) => view.pending.push(wire::Event::Theme { dark: true }),
            _ => panic!("not seated"),
        }
        seat.update(cx, |seat, cx| seat.turn(cx));
        assert!(
            !seat.read(cx).in_flight(),
            "the frame did not land inside its turn"
        );
    });
    assert_eq!(ticks_of(&seat, &native), ticks + 1);
}

/// A seat of `program` placed by the session's `Seats` in a window of its
/// own, its view answering `frames` in order on a [`DeferredRunner`]: the
/// door's `settle` is then `Seats::settle`, as the shell's door has it.
fn placed_by_seats(
    cx: &mut TestAppContext,
    program: &'static str,
    frames: &[wire::Frame],
) -> (
    Entity<crate::session::Seats>,
    Entity<Seat>,
    VisualTestContext,
    Pump,
) {
    let runner = DeferredRunner::default();
    let pump = runner.pump.clone();
    let runner: Arc<dyn ViewRunner> = Arc::new(runner);
    crate::runtime::seat_ticking_on_for_test(&runner, program, 320, frames);
    let seats = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::session::Shared::for_test(Default::default(), Default::default(), cx).seats
    });
    let pane = instance();
    let placing = seats.clone();
    let window = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let place = (
            program,
            Some((window.window_handle(), Some((400., 300.)), true)),
        );
        placing.update(cx, |seats, cx| {
            seats.place(std::collections::BTreeMap::from([(pane, place)]), cx)
        });
        let seat = placing.read(cx).seat(pane).expect("placed");
        Root {
            seat: seat.clone(),
            selection_layer: false,
            drawn: Vec::new(),
            _watch: cx.observe(&seat, |_, _, cx| cx.notify()),
        }
    });
    let seat = window
        .root(cx)
        .unwrap()
        .read_with(cx, |root, _| root.seat.clone());
    let native = VisualTestContext::from_window(window.into(), cx);
    native.run_until_parked();
    (seats, seat, native, pump)
}

/// The AX door's reads park while a seat's tick is in flight (the session's
/// `Seats::settle` says not every tick landed) and go on once the frame
/// lands, on the polls `/wait` parks on; a read whose client's deadline
/// passed goes on with the tick still out.
#[gpui_kit::test]
fn the_doors_settle_parks_a_read_until_the_frame_lands(cx: &mut TestAppContext) {
    const PROGRAM: &str = "deferred-door-settle";
    let (seats, seat, mut native, pump) = placed_by_seats(cx, PROGRAM, &five_trees());
    draw_now(&mut native);
    subscribe_props(&seat, &native);
    pump.hold();
    nudge(&seat, b"moved", &mut native);
    assert!(in_flight(&seat, &native));
    let read = std::rc::Rc::new(std::cell::Cell::new(0));
    for until in [Some(std::time::Instant::now()), None] {
        let done = read.clone();
        let seats = seats.clone();
        native
            .spawn(async move |mut cx| {
                let settle =
                    move |cx: &mut gpui_kit::App| seats.update(cx, |seats, cx| seats.settle(cx));
                let door = crate::ax::Door {
                    windows: &|_| Vec::new(),
                    settle: &settle,
                    keep: &|_, _| None,
                    chords: &Vec::new,
                };
                crate::ax::settled(&door, until, &mut cx).await;
                done.set(done.get() + 1);
            })
            .detach();
    }
    for _ in 0..3 {
        native.executor().advance_clock(Duration::from_millis(100));
        native.run_until_parked();
    }
    assert_eq!(
        read.get(),
        1,
        "the read past its client's deadline went on; the other waits"
    );
    assert!(in_flight(&seat, &native));
    assert_eq!(pump.release_all(), 1);
    native.run_until_parked();
    native.executor().advance_clock(Duration::from_millis(100));
    native.run_until_parked();
    assert_eq!(
        read.get(),
        2,
        "the read stayed parked after the frame landed"
    );
    assert_eq!(ticks_of(&seat, &native), 2);
}

/// A frame that lands after `hide` is kept on the view (its tick counted,
/// its tree taken in) and never drawn: the tree the pane last drew stays,
/// and no turn follows.
#[gpui_kit::test]
fn a_landing_after_hide_is_kept_not_drawn(cx: &mut TestAppContext) {
    const PROGRAM: &str = "deferred-hide";
    let _on = crate::perf::on_for_test();
    let (seat, _, mut native, pump) = deferred(cx, PROGRAM, &five_trees());
    draw_now(&mut native);
    subscribe_props(&seat, &native);
    pump.hold();
    nudge(&seat, b"moved", &mut native);
    assert!(in_flight(&seat, &native));
    let renders = renders_of(PROGRAM);
    let turns = turns_of(&seat, &native);
    let rev = frame_rev_of(&seat, &native);
    let tree = seat.read_with(&native, |seat, _| seat.tree());
    let intents = native.update(|_, cx| seat.update(cx, |seat, cx| seat.hide(cx)));
    assert!(intents.is_empty());
    assert_eq!(pump.release_all(), 1);
    draw_now(&mut native);
    draw_now(&mut native);
    assert_eq!(ticks_of(&seat, &native), 2, "the landed frame was not kept");
    assert!(
        frame_rev_of(&seat, &native) > rev,
        "the landed tree was not taken in"
    );
    assert_eq!(
        renders_of(PROGRAM),
        renders,
        "a frame landing after hide was drawn"
    );
    assert_eq!(
        seat.read_with(&native, |seat, _| seat.tree())
            .map(|tree| tree.entity_id()),
        tree.map(|tree| tree.entity_id()),
        "the hidden seat mounted a new tree"
    );
    nudge(&seat, b"again", &mut native);
    assert_eq!(turns_of(&seat, &native), turns, "a hidden seat turned");
    assert_eq!(pump.held(), 0);
}

/// A deployment swap on a runner that answers later (SPEC §4.6): the seat's
/// thread asks the drawn view for its snapshot, seats the new code,
/// restores it and takes its first frame, each step begun only once the
/// one before landed. Meanwhile the drawn view is not turned (a turn asked
/// for waits), its tree stays up, and the AX door parks; the new tree is
/// drawn once the last answer lands.
#[gpui_kit::test]
fn the_hot_swap_sequence_lands_in_order(cx: &mut TestAppContext) {
    use super::first_frame_tests::frames_wat;
    const PROGRAM: &str = "deferred-swap";
    let _on = crate::perf::on_for_test();
    let runner = DeferredRunner::default();
    let pump = runner.pump.clone();
    let runner: Arc<dyn ViewRunner> = Arc::new(runner);
    let old = crate::runtime::view::prepared_on_for_test(&runner, frames_wat(&[keyed("old")]));
    crate::runtime::seat_code_on_for_test(&runner, PROGRAM, 320, old);
    let (seat, root, mut native) = open(cx, PROGRAM, false);
    draw_now(&mut native);
    subscribe_props(&seat, &native);
    let mounted = mounted_of(&seat, &native);
    let alive = match &mounted.lock().unwrap().slot {
        Slot::Ready(view) => view.alive.clone(),
        _ => panic!("not seated"),
    };
    let tree = seat.read_with(&native, |seat, _| seat.tree().map(|tree| tree.entity_id()));
    let renders = renders_of(PROGRAM);
    let turns = turns_of(&seat, &native);
    // a new deployment's code, prepared on the same runner and parked as
    // a load parks it
    let new = crate::runtime::view::prepared_on_for_test(&runner, frames_wat(&[keyed("new")]));
    pump.hold();
    park_swap(&mounted, PROGRAM, &runner, new);
    native.run_until_parked();
    let started = turns_of(&seat, &native);
    assert_eq!(
        started,
        turns + 1,
        "the parked swap's wake turned the seat once"
    );
    for step in ["Snapshot", "Seated", "Restored", "Frame"] {
        assert_eq!(
            pump.held_kinds(),
            [step],
            "one call out at a time, in order"
        );
        assert!(in_flight(&seat, &native), "{step}: the door parks");
        assert!(
            matches!(&mounted.lock().unwrap().slot, Slot::Ready(view) if Arc::ptr_eq(&view.alive, &alive)),
            "{step}: the drawn view keeps its seat"
        );
        assert_eq!(
            seat.read_with(&native, |seat, _| seat.tree().map(|tree| tree.entity_id())),
            tree,
            "{step}: the drawn tree stays up"
        );
        assert_eq!(renders_of(PROGRAM), renders, "{step}: nothing drew");
        // a turn asked for meanwhile waits for the swap
        nudge(&seat, step.as_bytes(), &mut native);
        assert_eq!(
            turns_of(&seat, &native),
            started,
            "{step}: the drawn view turned"
        );
        assert!(pump.release_next());
        native.run_until_parked();
    }
    // what the new view asks after its first frame lands as it comes
    pump.at_once();
    pump.release_all();
    draw_now(&mut native);
    assert!(!in_flight(&seat, &native));
    let locked = mounted.lock().unwrap();
    let Slot::Ready(fresh) = &locked.slot else {
        panic!("not seated");
    };
    assert!(
        !Arc::ptr_eq(&fresh.alive, &alive),
        "the new code took the seat"
    );
    assert!(locked.swap.is_none() && !locked.owned && locked.retry.is_none());
    drop(locked);
    assert!(renders_of(PROGRAM) > renders, "the new tree was drawn");
    let (ticks, rev) = *root
        .read_with(&native, |root, _| root.drawn.clone())
        .last()
        .unwrap();
    assert_eq!(
        rev,
        frame_rev_of(&seat, &native),
        "the window drew the new view's tree"
    );
    assert!(ticks >= 1);
    assert_eq!(pump.held(), 0);
}

/// Parks a swap of `program`'s drawn view to the code `new`, prepared on
/// `runner`, on its seat as a load parks it (`Loaded::Swap`).
fn park_swap(
    mounted: &Arc<Mutex<Mounted>>,
    program: &'static str,
    runner: &Arc<dyn ViewRunner>,
    new: [u8; 32],
) {
    let generation = mounted.lock().unwrap().start();
    // read before the load: its install reads the connection too
    let asked_of = connection().lock().unwrap().clone();
    Load {
        program,
        seat: mounted.clone(),
        generation,
        asked_of,
        code: Some((::abi::BlobId::Sha256([9; 32]), false)),
    }
    .land(|load, _| {
        let mut code = ViewCode::for_test("", "", 320);
        code.runner = runner.clone();
        code.hash = new;
        Ok(Loaded::Swap {
            code,
            shown: load.program.into(),
        })
    });
}

/// The drawn view of a seated swap, on [`DeferredRunner`] answering at
/// once: the seat, its window, its registry entry, the pump, the runner,
/// and the drawn instance's `alive`.
#[allow(clippy::type_complexity)]
fn swappable(
    cx: &mut TestAppContext,
    program: &'static str,
) -> (
    Entity<Seat>,
    VisualTestContext,
    Arc<Mutex<Mounted>>,
    Pump,
    Arc<dyn ViewRunner>,
    Arc<()>,
) {
    use super::first_frame_tests::frames_wat;
    let runner = DeferredRunner::default();
    let pump = runner.pump.clone();
    let runner: Arc<dyn ViewRunner> = Arc::new(runner);
    let old = crate::runtime::view::prepared_on_for_test(&runner, frames_wat(&[keyed("old")]));
    crate::runtime::seat_code_on_for_test(&runner, program, 320, old);
    let (seat, _, mut native) = open(cx, program, false);
    draw_now(&mut native);
    let mounted = mounted_of(&seat, &native);
    let alive = match &mounted.lock().unwrap().slot {
        Slot::Ready(view) => view.alive.clone(),
        _ => panic!("not seated"),
    };
    (seat, native, mounted, pump, runner, alive)
}

/// One call in flight per instance: a swap whose task first runs after a
/// tree event already began a tick on the drawn view waits for that tick's
/// frame, and the turn the frame finishes runs it then. Without the wait
/// the swap asked the same instance for its snapshot with the tick out,
/// the tick took the snapshot's answer, and the view stopped.
#[gpui_kit::test]
fn a_swap_waits_for_a_tick_in_flight_on_the_drawn_view(cx: &mut TestAppContext) {
    use super::first_frame_tests::frames_wat;
    const PROGRAM: &str = "deferred-swap-after-tick";
    let (seat, mut native, mounted, pump, runner, alive) = swappable(cx, PROGRAM);
    let new = crate::runtime::view::prepared_on_for_test(&runner, frames_wat(&[keyed("new")]));
    native.update(|_, cx| {
        pump.hold();
        park_swap(&mounted, PROGRAM, &runner, new);
        // a turn finds the swap parked and the view settled: `swap_in` is
        // spawned, and has not run yet
        seat.update(cx, |seat, cx| seat.turn(cx));
        // a tree event, as the tree's subscription delivers it, and the turn
        // it asks for, before `swap_in` first runs: a tick begins
        match &mut mounted.lock().unwrap().slot {
            Slot::Ready(view) => view.pending.push(wire::Event::Theme { dark: true }),
            _ => panic!("not seated"),
        }
        seat.update(cx, |seat, cx| seat.turn(cx));
    });
    native.run_until_parked();
    assert_eq!(
        pump.held_kinds(),
        ["Frame"],
        "one call in flight on the drawn instance"
    );
    {
        let locked = mounted.lock().unwrap();
        assert!(
            locked.swap.is_some() && !locked.owned,
            "the swap waits, parked"
        );
    }
    pump.at_once();
    assert_eq!(pump.release_all(), 1);
    native.run_until_parked();
    draw_now(&mut native);
    let locked = mounted.lock().unwrap();
    let Slot::Ready(fresh) = &locked.slot else {
        panic!("not seated");
    };
    assert!(
        !Arc::ptr_eq(&fresh.alive, &alive),
        "the new code took the seat once the tick landed"
    );
    assert_eq!(fresh.fault, None);
    assert!(locked.swap.is_none() && !locked.owned && locked.retry.is_none());
}

/// The page dies while the swap's snapshot is out (SPEC §6.5): only the
/// swap fails (asked again at the next roster read); the drawn view keeps
/// its seat with no fault of its own, and the death is taken up where it
/// lands: the next tick goes to a new page, not the dead one.
#[gpui_kit::test]
fn a_death_under_the_swaps_snapshot_fails_only_the_swap(cx: &mut TestAppContext) {
    use super::first_frame_tests::frames_wat;
    const PROGRAM: &str = "deferred-swap-death";
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    let (seat, mut native, mounted, pump, runner, alive) = swappable(cx, PROGRAM);
    let new = crate::runtime::view::prepared_on_for_test(&runner, frames_wat(&[keyed("new")]));
    pump.hold();
    park_swap(&mounted, PROGRAM, &runner, new);
    native.run_until_parked();
    assert_eq!(pump.held_kinds(), ["Snapshot"]);
    assert_eq!(pump.die(), 1);
    native.run_until_parked();
    draw_now(&mut native);
    crate::contract::testing::snapshot_every_for_test(None);
    let locked = mounted.lock().unwrap();
    let Slot::Ready(view) = &locked.slot else {
        panic!("not seated");
    };
    assert!(
        Arc::ptr_eq(&view.alive, &alive),
        "the drawn view kept its seat"
    );
    assert_eq!(view.fault, None, "the death is no fault of the view's");
    assert!(locked.retry.is_some(), "the swap is asked again");
    assert!(locked.swap.is_none() && !locked.owned);
    drop(locked);
    assert!(!in_flight(&seat, &native));
    let ticks = ticks_of(&seat, &native);
    assert_eq!(
        next_tick_kinds(&seat, &mounted, &pump, &mut native),
        ["Frame"],
        "the next tick went into the dead page"
    );
    assert_eq!(ticks_of(&seat, &native), ticks + 1);
}

/// One tick of the drawn view on `seat` (a tree event, a turn), its answer
/// held and seen, then landed: what the instance answered, by kind.
fn next_tick_kinds(
    seat: &Entity<Seat>,
    mounted: &Arc<Mutex<Mounted>>,
    pump: &Pump,
    native: &mut VisualTestContext,
) -> Vec<String> {
    pump.hold();
    native.update(|_, cx| {
        match &mut mounted.lock().unwrap().slot {
            Slot::Ready(view) => view.pending.push(wire::Event::Theme { dark: true }),
            _ => panic!("not seated"),
        }
        seat.update(cx, |seat, cx| seat.turn(cx));
    });
    native.run_until_parked();
    let kinds = pump.held_kinds();
    pump.at_once();
    pump.release_all();
    native.run_until_parked();
    kinds
}

/// The page dies under the swap's restore, the state taken and in hand
/// (SPEC §6.5): the swap fails, as at the snapshot, and the drawn view
/// keeps its seat and its state; the new instance is not started clean
/// in its place.
#[gpui_kit::test]
fn a_death_under_the_swaps_restore_fails_only_the_swap(cx: &mut TestAppContext) {
    use super::first_frame_tests::frames_wat;
    const PROGRAM: &str = "deferred-swap-restore-death";
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    let (seat, mut native, mounted, pump, runner, alive) = swappable(cx, PROGRAM);
    let new = crate::runtime::view::prepared_on_for_test(&runner, frames_wat(&[keyed("new")]));
    pump.hold();
    park_swap(&mounted, PROGRAM, &runner, new);
    for step in ["Snapshot", "Seated"] {
        native.run_until_parked();
        assert_eq!(pump.held_kinds(), [step]);
        assert!(pump.release_next());
    }
    native.run_until_parked();
    assert_eq!(pump.held_kinds(), ["Restored"]);
    assert_eq!(pump.die(), 1);
    native.run_until_parked();
    draw_now(&mut native);
    crate::contract::testing::snapshot_every_for_test(None);
    {
        let locked = mounted.lock().unwrap();
        let Slot::Ready(view) = &locked.slot else {
            panic!("not seated");
        };
        assert!(
            Arc::ptr_eq(&view.alive, &alive),
            "a replacement started clean took the seat: the state is lost"
        );
        assert_eq!(view.fault, None);
        assert!(locked.retry.is_some() && locked.swap.is_none() && !locked.owned);
    }
    assert_eq!(
        next_tick_kinds(&seat, &mounted, &pump, &mut native),
        ["Frame"],
        "the drawn view's own page is alive"
    );
}

/// An idle view whose page died under a swap's snapshot is taken up where
/// the death lands: the next roster read's swap asks a live page for the
/// snapshot, and goes through. Before, nothing ticked the idle view, and
/// every block's swap asked the dead page again.
#[gpui_kit::test]
fn an_idle_view_whose_page_died_swaps_at_the_next_read(cx: &mut TestAppContext) {
    use super::first_frame_tests::frames_wat;
    const PROGRAM: &str = "deferred-idle-dead-swap";
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    let (_seat, mut native, mounted, pump, runner, alive) = swappable(cx, PROGRAM);
    let new = crate::runtime::view::prepared_on_for_test(&runner, frames_wat(&[keyed("new")]));
    pump.hold();
    park_swap(&mounted, PROGRAM, &runner, new);
    native.run_until_parked();
    assert_eq!(pump.held_kinds(), ["Snapshot"]);
    assert_eq!(pump.die(), 1);
    native.run_until_parked();
    draw_now(&mut native);
    // the next block's read
    pump.hold();
    park_swap(&mounted, PROGRAM, &runner, new);
    native.run_until_parked();
    assert_eq!(pump.held_kinds(), ["Snapshot"], "asked of a live page");
    pump.at_once();
    pump.release_all();
    native.run_until_parked();
    draw_now(&mut native);
    crate::contract::testing::snapshot_every_for_test(None);
    let locked = mounted.lock().unwrap();
    let Slot::Ready(fresh) = &locked.slot else {
        panic!("not seated");
    };
    assert!(
        !Arc::ptr_eq(&fresh.alive, &alive),
        "the second read's swap went through"
    );
    assert_eq!(fresh.fault, None);
    assert!(locked.swap.is_none() && !locked.owned);
}

/// The frames view of `five_trees`, its `snapshot` always refusing (the
/// view's own word: it has work in hand, a write in flight).
fn refusing_frames_wat() -> String {
    use super::first_frame_tests::frames_wat;
    let wat = frames_wat(&five_trees());
    let answer = format!("i64.const {}", wire::abi::pack(32, 2));
    assert!(wat.contains(&answer));
    wat.replace(
        "(i32.store8 (i32.const 33) (global.get $n))",
        "(i32.store8 (i32.const 32) (i32.const 1))",
    )
    .replace(&answer, &format!("i64.const {}", wire::abi::pack(32, 1)))
}

/// A kept snapshot the view refuses (its own word, while work is in hand)
/// is asked again after the next tick, not at once: the turn its refusal
/// wakes asks nothing, and the main thread is not kept busy asking.
#[gpui_kit::test]
fn a_refused_kept_snapshot_is_asked_again_after_a_tick(cx: &mut TestAppContext) {
    const PROGRAM: &str = "deferred-keep-refused";
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    let runner = DeferredRunner::default();
    let pump = runner.pump.clone();
    let runner: Arc<dyn ViewRunner> = Arc::new(runner);
    let code = crate::runtime::view::prepared_on_for_test(&runner, refusing_frames_wat());
    crate::runtime::seat_code_on_for_test(&runner, PROGRAM, 320, code);
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    draw_now(&mut native);
    let mounted = mounted_of(&seat, &native);
    pump.hold();
    crate::contract::testing::snapshot_every_for_test(Some(Duration::ZERO));
    native.update(|_, cx| seat.update(cx, |seat, cx| seat.turn(cx)));
    native.run_until_parked();
    assert_eq!(pump.held_kinds(), ["SnapshotRefused"]);
    assert!(pump.release_next());
    native.run_until_parked();
    assert_eq!(pump.held(), 0, "a refused snapshot was asked again at once");
    // a tick: due again after it
    native.update(|_, cx| {
        match &mut mounted.lock().unwrap().slot {
            Slot::Ready(view) => view.pending.push(wire::Event::Theme { dark: true }),
            _ => panic!("not seated"),
        }
        seat.update(cx, |seat, cx| seat.turn(cx));
    });
    native.run_until_parked();
    assert_eq!(pump.held_kinds(), ["Frame"]);
    assert!(pump.release_next());
    native.run_until_parked();
    assert_eq!(pump.held_kinds(), ["SnapshotRefused"], "asked after the tick");
    crate::contract::testing::snapshot_every_for_test(None);
    pump.at_once();
    pump.release_all();
    native.run_until_parked();
}

/// The page dies under a kept snapshot (SPEC §6.5): the death is taken up
/// where it lands, a new instance replaying every kept batch with nothing
/// left in flight, and the dead page is not asked again; the next tick
/// goes to the new page and lands the frame an uninterrupted run lands.
#[gpui_kit::test]
fn a_death_under_a_kept_snapshot_is_taken_up_where_it_lands(cx: &mut TestAppContext) {
    use super::first_frame_tests::frames_wat;
    const PROGRAM: &str = "deferred-keep-death";
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    let runner = DeferredRunner::default();
    let pump = runner.pump.clone();
    let runner: Arc<dyn ViewRunner> = Arc::new(runner);
    let code = crate::runtime::view::prepared_on_for_test(&runner, frames_wat(&five_trees()));
    crate::runtime::seat_code_on_for_test(&runner, PROGRAM, 320, code);
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    draw_now(&mut native);
    subscribe_props(&seat, &native);
    for props in [b"a".as_slice(), b"b".as_slice()] {
        nudge(&seat, props, &mut native);
        draw_now(&mut native);
    }
    let mounted = mounted_of(&seat, &native);
    let ticks = ticks_of(&seat, &native);
    pump.hold();
    crate::contract::testing::snapshot_every_for_test(Some(Duration::ZERO));
    native.update(|_, cx| seat.update(cx, |seat, cx| seat.turn(cx)));
    native.run_until_parked();
    assert_eq!(pump.held_kinds(), ["Snapshot"]);
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    // the revive's calls land at once; anything after it is held, and seen
    assert_eq!(pump.die(), 1);
    native.run_until_parked();
    pump.hold();
    native.run_until_parked();
    assert_eq!(pump.held(), 0, "the dead page was asked again");
    {
        let locked = mounted.lock().unwrap();
        let Slot::Ready(view) = &locked.slot else {
            panic!("not seated");
        };
        assert!(view.fault.is_none() && !view.ticking() && !locked.owned);
        assert_eq!(view.ticks, ticks);
    }
    pump.at_once();
    assert_eq!(
        next_tick_kinds(&seat, &mounted, &pump, &mut native),
        ["Frame"],
        "the next tick went into the dead page"
    );
    let locked = mounted.lock().unwrap();
    let Slot::Ready(view) = &locked.slot else {
        panic!("not seated");
    };
    crate::contract::testing::snapshot_every_for_test(None);
    assert_eq!(view.fault, None);
    assert_eq!(view.ticks, ticks + 1);
    assert_eq!(
        wire::encode(&view.frame.root),
        wire::encode(&keyed("four").root),
        "the replay left the view where it was: tick 4 draws the fourth tree"
    );
}

/// A death on the page a revive seats is no fault of the view's either
/// (SPEC §6.5): the revive starts over on another page, and the tick that
/// died lands its frame there.
#[gpui_kit::test]
fn a_death_during_a_revive_starts_it_over(cx: &mut TestAppContext) {
    const PROGRAM: &str = "deferred-revive-death";
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    let (seat, _, mut native, pump) = deferred(cx, PROGRAM, &five_trees());
    draw_now(&mut native);
    subscribe_props(&seat, &native);
    let ticks = ticks_of(&seat, &native);
    pump.hold();
    nudge(&seat, b"moved", &mut native);
    assert!(in_flight(&seat, &native));
    assert_eq!(pump.die(), 1);
    pump.hold();
    native.run_until_parked();
    assert_eq!(pump.held_kinds(), ["Seated"], "the revive seats a new page");
    assert!(pump.release_next());
    native.run_until_parked();
    assert_eq!(pump.held_kinds(), ["Inited"]);
    // that page dies too
    assert_eq!(pump.die(), 1);
    native.run_until_parked();
    draw_now(&mut native);
    crate::contract::testing::snapshot_every_for_test(None);
    assert!(!in_flight(&seat, &native));
    assert_eq!(ticks_of(&seat, &native), ticks + 1, "the tick that died landed");
    assert!(
        matches!(&mounted_of(&seat, &native).lock().unwrap().slot, Slot::Ready(view) if view.fault.is_none()),
        "a second death faulted the view"
    );
}

/// In the background `hide` begins no call into the view (SPEC §4.3): its
/// last update (the view told it is hidden) is not run.
#[gpui_kit::test]
fn hide_in_the_background_begins_no_tick(cx: &mut TestAppContext) {
    const PROGRAM: &str = "hide-background";
    let (seat, _, mut native, pump) = deferred(cx, PROGRAM, &five_trees());
    draw_now(&mut native);
    let ticks = ticks_of(&seat, &native);
    pump.hold();
    native.update(|_, cx| cx.set_global(crate::runtime::Backgrounded(true)));
    let intents = native.update(|_, cx| seat.update(cx, |seat, cx| seat.hide(cx)));
    assert!(intents.is_empty());
    assert_eq!(pump.held(), 0, "hide began a tick in the background");
    assert_eq!(ticks_of(&seat, &native), ticks);
}

/// A death replay (SPEC §6.5): the instance dies with a tick in flight,
/// and where the host keeps a replay the seat takes the view up again on a
/// new instance, from its kept start (here `init`), replays every tick's
/// events since with their frames discarded, and begins the tick that died
/// again. The frame that tick lands is the one an uninterrupted run lands
/// at the same tick, byte for byte, and nothing else draws or turns twice:
/// the replayed frames are thrown away, never taken in again (the tree's
/// revision moves once, for the tick that died).
#[gpui_kit::test]
fn a_death_replay_reproduces_the_frame_of_tick_n(cx: &mut TestAppContext) {
    use sha2::Digest as _;
    // a replay kept from `init`, never snapshotted again in this test
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    let frame_hash = |seat: &Entity<Seat>, cx: &TestAppContext| -> [u8; 32] {
        match &mounted_of(seat, cx).lock().unwrap().slot {
            Slot::Ready(view) => sha2::Sha256::digest(wire::encode(&view.frame.root)).into(),
            _ => panic!("not seated"),
        }
    };
    let script = |seat: &Entity<Seat>, native: &mut VisualTestContext, pump: &Pump, die: bool| {
        draw_now(native);
        subscribe_props(seat, native);
        for props in [b"a".as_slice(), b"b".as_slice()] {
            nudge(seat, props, native);
            draw_now(native);
        }
        // tick 4: in flight when the page dies
        pump.hold();
        nudge(seat, b"c", native);
        assert!(in_flight(seat, native));
        match die {
            true => assert_eq!(pump.die(), 1),
            false => {
                pump.at_once();
                assert_eq!(pump.release_all(), 1);
            }
        }
        native.run_until_parked();
        draw_now(native);
        (
            ticks_of(seat, native),
            turns_of(seat, native),
            frame_rev_of(seat, native),
            frame_hash(seat, native),
        )
    };
    let (seat, _, mut native, pump) = deferred(cx, "replay-steady", &five_trees());
    let steady = script(&seat, &mut native, &pump, false);
    let runner = DeferredRunner::default();
    let dying = runner.pump.clone();
    let runner: Arc<dyn ViewRunner> = Arc::new(runner);
    crate::runtime::seat_ticking_on_for_test(&runner, "replay-dying", 320, &five_trees());
    let (seat, _, mut native) = another(cx, "replay-dying", false);
    let revived = script(&seat, &mut native, &dying, true);
    crate::contract::testing::snapshot_every_for_test(None);
    assert_eq!(steady.0, 4, "four ticks: the first frame, a, b, c");
    assert_eq!(
        revived, steady,
        "(ticks, turns, tree revision, frame hash at tick 4) after the death"
    );
    assert!(
        matches!(&mounted_of(&seat, &native).lock().unwrap().slot, Slot::Ready(view) if view.fault.is_none()),
        "the death is no fault of the view's"
    );
    assert_eq!(dying.held(), 0);
}

/// A kept replay starts again from each new snapshot (SPEC §6.5): the
/// batches before it go, so a death replays nothing from that snapshot but
/// the tick that died, whose frame is the one an uninterrupted run lands.
/// A batch kept past its snapshot would be replayed on top of the state
/// that snapshot restores. A turn takes the snapshot once one is due
/// (`keep_in`), and only after a tick: a replay of none has nothing to
/// shorten.
#[gpui_kit::test]
fn a_kept_snapshot_starts_the_replay_again(cx: &mut TestAppContext) {
    use super::first_frame_tests::frames_wat;
    use sha2::Digest as _;
    const NOT_DUE: Duration = Duration::from_secs(3600);
    fn kept_of(mounted: &Arc<Mutex<Mounted>>) -> (Option<Vec<u8>>, usize) {
        match &mounted.lock().unwrap().slot {
            Slot::Ready(view) => {
                let kept = view.kept.as_ref().expect("a replay is kept");
                (kept.snapshot.clone(), kept.batches.len())
            }
            _ => panic!("not seated"),
        }
    }
    #[allow(clippy::type_complexity)]
    fn run(
        cx: &mut TestAppContext,
        program: &'static str,
        die: bool,
    ) -> (
        (u64, u64, u64, [u8; 32]),
        Arc<Mutex<Mounted>>,
        Entity<Seat>,
        VisualTestContext,
    ) {
        let runner = DeferredRunner::default();
        let pump = runner.pump.clone();
        let runner: Arc<dyn ViewRunner> = Arc::new(runner);
        let code = crate::runtime::view::prepared_on_for_test(&runner, frames_wat(&five_trees()));
        crate::runtime::seat_code_on_for_test(&runner, program, 320, code);
        let (seat, _, mut native) = open(cx, program, false);
        draw_now(&mut native);
        subscribe_props(&seat, &native);
        for props in [b"a".as_slice(), b"b".as_slice()] {
            nudge(&seat, props, &mut native);
            draw_now(&mut native);
        }
        let mounted = mounted_of(&seat, &native);
        // a snapshot due after tick 3, as a turn finds one due
        crate::contract::testing::snapshot_every_for_test(Some(Duration::ZERO));
        futures::executor::block_on(keep_in(mounted.clone()));
        crate::contract::testing::snapshot_every_for_test(Some(NOT_DUE));
        native.run_until_parked();
        assert_eq!(
            kept_of(&mounted),
            (Some(vec![3]), 0),
            "kept at tick 3, its batches gone"
        );
        // tick 4: in flight when the page dies, the only batch kept
        pump.hold();
        nudge(&seat, b"c", &mut native);
        assert!(in_flight(&seat, &native));
        assert_eq!(kept_of(&mounted).1, 1);
        match die {
            true => assert_eq!(pump.die(), 1),
            false => {
                pump.at_once();
                assert_eq!(pump.release_all(), 1);
            }
        }
        native.run_until_parked();
        draw_now(&mut native);
        let at_four = match &mounted.lock().unwrap().slot {
            Slot::Ready(view) => {
                assert_eq!(view.fault, None);
                (
                    view.ticks,
                    turns_of(&seat, &native),
                    view.frame_rev,
                    sha2::Sha256::digest(wire::encode(&view.frame.root)).into(),
                )
            }
            _ => panic!("not seated"),
        };
        (at_four, mounted, seat, native)
    }
    crate::contract::testing::snapshot_every_for_test(Some(NOT_DUE));
    let (steady, ..) = run(cx, "kept-steady", false);
    let (revived, mounted, seat, mut native) = run(cx, "kept-dying", true);
    assert_eq!(steady.0, 4, "four ticks: the first frame, a, b, c");
    assert_eq!(
        revived, steady,
        "(ticks, turns, tree revision, frame hash at tick 4) after the death"
    );
    // due after every turn: the turn after tick 5 takes it, and the one
    // after that, with no tick between, takes none
    crate::contract::testing::snapshot_every_for_test(Some(Duration::ZERO));
    nudge(&seat, b"d", &mut native);
    draw_now(&mut native);
    crate::contract::testing::snapshot_every_for_test(None);
    assert_eq!(
        kept_of(&mounted),
        (Some(vec![5]), 0),
        "kept by the turn at tick 5"
    );
}

/// A death in the background is not taken up there (SPEC §4.3: no call
/// into a view is begun): it waits on the view, and the turn in front again
/// takes it up, seating the new instance then.
#[gpui_kit::test]
fn a_death_in_the_background_is_taken_up_in_front(cx: &mut TestAppContext) {
    const PROGRAM: &str = "background-death";
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    let (seat, _, mut native, pump) = deferred(cx, PROGRAM, &five_trees());
    draw_now(&mut native);
    subscribe_props(&seat, &native);
    pump.hold();
    nudge(&seat, b"moved", &mut native);
    assert!(in_flight(&seat, &native));
    native.update(|_, cx| cx.set_global(Backgrounded(true)));
    assert_eq!(pump.die(), 1);
    // anything begun from here on is held, and seen
    pump.hold();
    native.run_until_parked();
    assert_eq!(pump.held(), 0, "a call was begun in the background");
    assert!(!in_flight(&seat, &native));
    {
        let mounted = mounted_of(&seat, &native);
        let locked = mounted.lock().unwrap();
        let Slot::Ready(view) = &locked.slot else {
            panic!("not seated");
        };
        assert!(view.fault.is_none() && view.ticking() && view.parked.is_some());
    }
    pump.at_once();
    native.update(|_, cx| cx.set_global(Backgrounded(false)));
    native.run_until_parked();
    draw_now(&mut native);
    crate::contract::testing::snapshot_every_for_test(None);
    assert!(!in_flight(&seat, &native));
    assert_eq!(ticks_of(&seat, &native), 2, "the tick that died, landed");
    assert!(
        matches!(&mounted_of(&seat, &native).lock().unwrap().slot, Slot::Ready(view) if view.fault.is_none() && view.parked.is_none()),
        "taken up again in front"
    );
}

/// A frame that lands in the background begins nothing after it: no kept
/// snapshot, however due (SPEC §4.3); the turn in front again takes it.
#[gpui_kit::test]
fn a_frame_landing_in_the_background_begins_no_snapshot(cx: &mut TestAppContext) {
    use super::first_frame_tests::frames_wat;
    const PROGRAM: &str = "background-keep";
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    let runner = DeferredRunner::default();
    let pump = runner.pump.clone();
    let runner: Arc<dyn ViewRunner> = Arc::new(runner);
    let code = crate::runtime::view::prepared_on_for_test(&runner, frames_wat(&five_trees()));
    crate::runtime::seat_code_on_for_test(&runner, PROGRAM, 320, code);
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    draw_now(&mut native);
    subscribe_props(&seat, &native);
    pump.hold();
    nudge(&seat, b"moved", &mut native);
    assert!(in_flight(&seat, &native));
    native.update(|_, cx| cx.set_global(Backgrounded(true)));
    // due from here on
    crate::contract::testing::snapshot_every_for_test(Some(Duration::ZERO));
    assert!(pump.release_next());
    native.run_until_parked();
    assert!(!in_flight(&seat, &native));
    assert_eq!(pump.held(), 0, "a snapshot was begun in the background");
    native.update(|_, cx| cx.set_global(Backgrounded(false)));
    native.run_until_parked();
    assert_eq!(
        pump.held_kinds(),
        ["Snapshot"],
        "in front, the due snapshot"
    );
    pump.at_once();
    assert_eq!(pump.release_all(), 1);
    native.run_until_parked();
    crate::contract::testing::snapshot_every_for_test(None);
}

/// A death `hide`'s last update lands at once, where a replay is kept, is
/// no fault of the view's (SPEC §6.5): the update begins no call to take it
/// up again, and it stays on the view, which goes with the seat.
#[gpui_kit::test]
fn a_death_under_hide_is_no_fault_of_the_views(cx: &mut TestAppContext) {
    const PROGRAM: &str = "hide-death";
    crate::contract::testing::snapshot_every_for_test(Some(Duration::from_secs(3600)));
    let (seat, _, mut native, pump) = deferred(cx, PROGRAM, &five_trees());
    draw_now(&mut native);
    subscribe_props(&seat, &native);
    let mounted = mounted_of(&seat, &native);
    // the props the hidden pane's last update delivers
    mounted.lock().unwrap().props = Some(b"last".to_vec());
    pump.dying(true);
    let intents = native.update(|_, cx| seat.update(cx, |seat, cx| seat.hide(cx)));
    pump.dying(false);
    crate::contract::testing::snapshot_every_for_test(None);
    assert!(intents.is_empty());
    let locked = mounted.lock().unwrap();
    let Slot::Ready(view) = &locked.slot else {
        panic!("not seated");
    };
    assert_eq!(
        view.fault, None,
        "the page's death taken as the view's fault"
    );
    assert!(view.ticking() && view.parked.is_some());
}

/// In the background no seat turns, so no call into a view is begun: the
/// moved props wait, and the seat turns once, delivering them, when the app
/// is in front again (SPEC §4.3).
#[gpui_kit::test]
fn no_seat_turns_in_the_background(cx: &mut TestAppContext) {
    const PROGRAM: &str = "background-turns";
    crate::runtime::seat_ticking_for_test(PROGRAM, 320, &five_trees());
    let (seat, _, mut native) = open(cx, PROGRAM, false);
    draw_now(&mut native);
    subscribe_props(&seat, &native);
    let (turns, ticks) = (turns_of(&seat, &native), ticks_of(&seat, &native));
    native.update(|_, cx| cx.set_global(Backgrounded(true)));
    nudge(&seat, b"moved", &mut native);
    native.update(|_, cx| seat.update(cx, |seat, cx| seat.turn(cx)));
    native.run_until_parked();
    assert_eq!(
        (turns_of(&seat, &native), ticks_of(&seat, &native)),
        (turns, ticks),
        "a seat turned in the background"
    );
    native.update(|_, cx| cx.set_global(Backgrounded(false)));
    draw_now(&mut native);
    assert_eq!(
        turns_of(&seat, &native),
        turns + 1,
        "one turn in front again"
    );
    assert_eq!(
        ticks_of(&seat, &native),
        ticks + 1,
        "the props it held, delivered"
    );
}
