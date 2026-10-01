//! The seat, turned off the draw path, under a bare root that draws it as
//! `layers::PaneView` does: its tree, cached, or its standin.
use super::entity::native_root;
use super::*;
use gpui_kit::{
    Entity, IntoElement, ParentElement as _, Render, Styled as _, Subscription, TestAppContext,
    VisualTestContext, div, px, size,
};
use std::time::Duration;

struct Root {
    seat: Entity<Seat>,
    /// The desk's selection layer, whose sweep of a cached frame's
    /// paragraphs refreshed the window on every frame before #347.
    selection_layer: bool,
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
        let body = match (seat.standin(), seat.tree()) {
            (Some(standin), _) => standin.element(seat.module(), seat.instance(), seat.ax_mark()),
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

/// A window whose root draws one seat of `module`, placed in it (so its
/// first turn is deferred, and has run once this returns).
fn open(
    cx: &mut TestAppContext,
    module: &'static str,
    selection_layer: bool,
) -> (Entity<Seat>, Entity<Root>, VisualTestContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let seat = cx.new(|cx| Seat::new(module, cx));
        seat.update(cx, |seat, cx| seat.place(window.window_handle(), cx));
        Root {
            seat: seat.clone(),
            selection_layer,
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
    let key = seat.read_with(cx, |seat, _| (seat.module(), seat.instance()));
    registry().lock().unwrap()[&key].clone()
}

fn ticks_of(seat: &Entity<Seat>, cx: &TestAppContext) -> u64 {
    match &mounted_of(seat, cx).lock().unwrap().slot {
        Slot::Ready(guest) => guest.ticks,
        _ => panic!("not seated"),
    }
}

fn turns_of(seat: &Entity<Seat>, cx: &TestAppContext) -> u64 {
    seat.read_with(cx, |seat, _| seat.turns)
}

fn renders_of(module: &str) -> u64 {
    crate::perf::snapshot(false)["views"][module]["renders"]
        .as_u64()
        .unwrap_or(0)
}

/// `EditorStore` keys every editor by the `AuthoredPath` walked from the
/// guest's OWN root (`guest/requests.rs`'s `tick`, never wrapped). The
/// native widget tree is built from `native_root(root)` instead — the
/// wrapper `native_root` adds around that same root so an unsized guest
/// root still fills the seat. If that wrapper carried an id, every
/// descendant's `AuthoredPath` as walked from the RENDERED tree would
/// carry one extra leading segment the store never indexed under, and a
/// native editor field could never find its `EditorStore` entry: it would
/// keep typing locally (GPUI's own default text handling on an
/// editable-by-default field) while the guest's document — and everything
/// gated on it, like a claimed Enter or the Send button — never moved.
/// Reproduces that class of bug directly against the two real tree walks,
/// with no gpui window needed.
#[test]
fn native_root_does_not_shift_the_authored_path_editor_store_indexes_by() {
    let editor = wire::Node::Editor {
        binding: None,
        id: wire::ElementIdWire::Name("editor".into()),
        style: Default::default(),
        placeholder: String::new(),
        label: None,
        document: wire::editor_document::EditorDocumentRef {
            document: "doc".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: wire::EditorCursor::default(),
            byte_len: 0,
        },
        on_document: 0,
        editable: true,
    };
    let panel = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("panel".into())),
        style: Default::default(),
        interactivity: Default::default(),
        children: vec![editor],
    });

    // The guest's own root, unwrapped: what `EditorStore::replace` indexes,
    // exactly as `guest/requests.rs`'s `tick` calls it.
    let store = EditorStore::new(0);
    store
        .replace(&panel)
        .expect("a valid editor tree validates");

    // The tree the renderer actually walks to mount native widgets — the
    // one and only tree `native_root` ever produces for it.
    let rendered = native_root(panel);
    let mounted_key = editor_authored_path(&rendered).expect("the editor is still in the tree");

    assert!(
        store.projection(&mounted_key).is_some(),
        "a native editor's own mounted path must resolve in the EditorStore \
         the guest's unwrapped tree populated; native_root must add no identity"
    );
}

/// The `AuthoredPath` to the first `Editor` node in `root`, walked the same
/// way `crate::render`'s node lowering (and `EditorStore::collect`) do: by
/// `crate::render::enter_scope`, which is what decides whether a node
/// contributes a path segment at all.
fn editor_authored_path(root: &wire::Node) -> Option<crate::render::AuthoredPath> {
    fn walk(
        node: &wire::Node,
        path: &mut crate::render::AuthoredPath,
    ) -> Option<crate::render::AuthoredPath> {
        let entered = crate::render::enter_scope(node, path);
        let found = matches!(node, wire::Node::Editor { .. })
            .then(|| path.clone())
            .or_else(|| node.children().iter().find_map(|child| walk(child, path)));
        if entered {
            path.pop();
        }
        found
    }
    walk(root, &mut crate::render::AuthoredPath::new())
}

#[gpui_kit::test]
fn same_module_instances_receive_independent_props(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let first = cx.new(|cx| Seat::new("independent-props-test", cx));
    let second = cx.new(|cx| Seat::new("independent-props-test", cx));
    first.update(cx, |seat, cx| seat.set_props(b"channel-one".to_vec(), cx));
    second.update(cx, |seat, cx| seat.set_props(b"channel-two".to_vec(), cx));
    cx.update(|cx| {
        let (first, second) = (first.read(cx), second.read(cx));
        assert_ne!(first.instance(), second.instance());
        let registry = registry().lock().unwrap();
        assert!(!Arc::ptr_eq(
            &registry[&(first.module(), first.instance())],
            &registry[&(second.module(), second.instance())]
        ));
        drop(registry);
        assert_eq!(first.props(), Some(b"channel-one".as_slice()));
        assert_eq!(second.props(), Some(b"channel-two".as_slice()));
    });
    first.update(cx, |seat, cx| seat.set_props(b"channel-three".to_vec(), cx));
    cx.update(|cx| {
        assert_eq!(second.read(cx).props(), Some(b"channel-two".as_slice()));
    });
}

/// The root a view is drawn in is laid out from the view's own minimum.
#[gpui_kit::test]
fn a_view_is_laid_out_from_its_own_minimum(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    crate::runtime::seat_for_test("laid-out-from", 560);
    let seat = cx.new(|cx| Seat::new("laid-out-from", cx));
    seat.update(cx, |seat, cx| seat.turn(cx));
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
    const MODULE: &str = "idle-renders-test";
    let bare = |children: Vec<wire::Node>| {
        wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: div().p_2().style().clone(),
            interactivity: Default::default(),
            children,
        })
    };
    let paragraph = wire::Node::RichText {
        id: Some(wire::ElementIdWire::Name("line".into())),
        style: div().h(px(20.)).style().clone(),
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
        style: div().w(px(200.)).h(px(100.)).style().clone(),
        interactivity: Default::default(),
        children: vec![bare(vec![bare(vec![paragraph])])],
    });
    let _on = crate::perf::on_for_test();
    crate::runtime::seat_drawing_for_test(MODULE, 320, card);
    let (_, root, mut native) = open(cx, MODULE, true);
    // the desk redraws its panes on every frame it draws
    for _ in 0..8 {
        native.update(|window, cx| {
            root.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        });
        native.run_until_parked();
    }
    let counted = &crate::perf::snapshot(false)["views"][MODULE];
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
    const MODULE: &str = "blink-renders-test";
    let field = wire::Node::Input {
        options: wire::InputOptions {
            label: "Filter members".into(),
            ..Default::default()
        },
        id: wire::ElementIdWire::Name("filter".into()),
        placeholder: "Filter by name".into(),
        value: "a value the mount sets".into(),
        on_input: Some(1),
        on_submit: None,
        secure: false,
        style: div().w(px(200.)).h(px(24.)).style().clone(),
    };
    let card = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("card".into())),
        style: div().w(px(240.)).h(px(100.)).style().clone(),
        interactivity: Default::default(),
        children: vec![field],
    });
    let _on = crate::perf::on_for_test();
    crate::runtime::seat_drawing_for_test(MODULE, 320, card);
    let (seat, root, mut native) = open(cx, MODULE, false);
    let frame = |native: &mut VisualTestContext| {
        native.update(|window, cx| {
            root.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        });
        native.run_until_parked();
    };
    // two seconds at rest, drawn every 100 ms as the desk draws its panes
    let idle = |native: &mut VisualTestContext| {
        let before = renders_of(MODULE);
        for _ in 0..20 {
            native.executor().advance_clock(Duration::from_millis(100));
            frame(native);
        }
        renders_of(MODULE) - before
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

/// A guest out of budget (`frame.busy`) is turned again a frame later
/// (`BUSY_FRAME`), never back to back, and not a third time without a wake.
#[gpui_kit::test]
fn a_busy_frame_turns_once_per_frame(cx: &mut TestAppContext) {
    const MODULE: &str = "busy-frame-test";
    cx.update(gpui_kit::init);
    crate::runtime::seat_busy_for_test(MODULE, 320, 2);
    let seat = cx.new(|cx| Seat::new(MODULE, cx));
    seat.update(cx, |seat, cx| seat.turn(cx));
    assert_eq!(ticks_of(&seat, cx), 1);
    cx.run_until_parked();
    assert_eq!(
        ticks_of(&seat, cx),
        1,
        "a busy guest is not turned back to back"
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
    let seat = cx.new(|cx| Seat::new("props-turn-test", cx));
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
    const MODULE: &str = "load-landing-test";
    let _on = crate::perf::on_for_test();
    let (seat, _, mut native) = open(cx, MODULE, false);
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
    assert_eq!(renders_of(MODULE), 0);
    crate::runtime::seat_for_test(MODULE, 320);
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
        renders_of(MODULE),
        1,
        "the tree rendered once and is cached"
    );
}

/// A load that lands wakes the seat: `spawn_load`'s install signals
/// `Mounted.wake`, which is what replaces the per-frame redraw loop in the
/// running app. No gpui task is involved: the load thread's signal is read
/// off the watch directly.
#[test]
fn a_load_that_lands_wakes_the_seat() {
    const MODULE: &str = "install-wake-test";
    let mounted = Mounted::seat();
    let woke = mounted.lock().unwrap().wake.subscribe();
    let generation = mounted.lock().unwrap().start();
    let snapshot = connection().lock().unwrap().clone();
    spawn_load(MODULE, &mounted, generation, snapshot)
        .join()
        .unwrap();
    assert!(
        woke.has_changed().unwrap(),
        "the install signalled the seat's wake"
    );
}

/// A stage the loader shows and a Retry each turn the seat once: the
/// stage words move the standin, the retry re-asks and turns.
#[gpui_kit::test]
fn a_stage_change_and_a_retry_each_turn_the_seat_once(cx: &mut TestAppContext) {
    const MODULE: &str = "stage-retry-test";
    let (seat, _, mut native) = open(cx, MODULE, false);
    let (notifies, _watch) = crate::shell::entities::tests::notifies(&seat, &mut native);
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

    mounted.lock().unwrap().show(0, Slot::Compiling);
    native.run_until_parked();
    assert_eq!(turns_of(&seat, &native), turns + 2);
    assert_eq!(notifies.get(), 2);
    assert_eq!(words(&native).as_deref(), Some("Compiling the view…"));

    // Retry: the load it starts fails at once (no node), so its signal and
    // the install's coalesce into the one turn the retry is
    let instance = seat.read_with(&native, |seat, _| seat.instance());
    let loads = retry(MODULE, instance);
    for thread in loads._threads {
        thread.join().unwrap();
    }
    native.run_until_parked();
    assert_eq!(
        turns_of(&seat, &native),
        turns + 3,
        "a retry turns the seat once"
    );
    assert!(
        matches!(mounted.lock().unwrap().slot, Slot::Failed(_)),
        "the retried load failed without a node"
    );
}

/// A guest whose frame both mounts a `TextInput` and asks `Focus` on it:
/// the input is created in the draw that mounts the tree, so the command
/// runs a frame after that draw, and the input has the keys after two
/// frames with no "focus handle is not mounted" refusal. Then the same
/// with the turn entered from an `on_next_frame` callback, where the
/// window is borrowed: `wake` defers it to app level.
#[gpui_kit::test]
fn widget_commands_run_after_the_tree_mounted(cx: &mut TestAppContext) {
    const MODULE: &str = "widget-commands-test";
    let field = wire::Node::Input {
        options: wire::InputOptions {
            label: "Filter".into(),
            ..Default::default()
        },
        id: wire::ElementIdWire::Name("filter".into()),
        placeholder: String::new(),
        value: String::new(),
        on_input: Some(1),
        on_submit: None,
        secure: false,
        style: div().w(px(200.)).h(px(24.)).style().clone(),
    };
    let focus_it = |instance| {
        let registry = registry().lock().unwrap();
        let Slot::Ready(guest) = &mut registry[&(MODULE, instance)].lock().unwrap().slot else {
            panic!("seated");
        };
        guest.widget_commands.push((
            7,
            wire::WidgetCommand::Focus {
                target: vec![wire::ElementIdWire::Name("filter".into())],
            },
        ));
    };
    crate::runtime::seat_drawing_for_test(MODULE, 320, field);
    // queued before the first turn: that turn's frame mounts the input
    focus_it(0);
    // the turn and the frame's first callbacks in one App update, before
    // the harness's flush-time draw: only a callback nested in the first
    // waits for the draw that mounts the input. A single callback answers
    // the Focus `Ok` and loses the focus silently, so the assertion is on
    // the focus itself, not the reply.
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
        let seat = cx.new(|cx| Seat::new(MODULE, cx));
        Root {
            seat: seat.clone(),
            selection_layer: false,
            _watch: cx.observe(&seat, |_, _, cx| cx.notify()),
        }
    });
    let root = window.root(cx).unwrap();
    let seat = root.read_with(cx, |root, _| root.seat.clone());
    let handle: gpui_kit::AnyWindowHandle = window.into();
    cx.update(|cx| {
        seat.update(cx, |seat, cx| {
            seat.place(handle, cx);
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

/// A program that leaves the roster gives up its seat: the pane shows the
/// "no view" standin, not its frozen tree: the retire itself wakes the
/// seat, since no clock redraws the window.
#[gpui_kit::test]
fn a_retired_program_shows_no_view(cx: &mut TestAppContext) {
    const MODULE: &str = "retired-test";
    crate::runtime::seat_for_test(MODULE, 320);
    let (seat, _, native) = open(cx, MODULE, false);
    assert!(seat.read_with(&native, |s, _| s.tree().is_some() && s.standin().is_none()));
    mounted_of(&seat, &native).lock().unwrap().retire();
    native.run_until_parked();
    let words = seat.read_with(&native, |s, _| s.standin().map(|s| s.words.clone()));
    assert!(
        words.as_deref().is_some_and(|w| w.contains("has no")),
        "the retired seat still shows its tree: {words:?}"
    );
}

/// A link's route to a view that is already open (`Layout::open` only
/// focuses it): the seat is woken, so the route does not wait for an
/// unrelated turn.
#[gpui_kit::test]
fn a_route_reaches_an_open_view(cx: &mut TestAppContext) {
    const MODULE: &str = "route-test";
    crate::runtime::seat_for_test(MODULE, 320);
    let (seat, _, native) = open(cx, MODULE, false);
    let before = ticks_of(&seat, &native);
    {
        let mounted = mounted_of(&seat, &native);
        let mut locked = mounted.lock().unwrap();
        let Slot::Ready(guest) = &mut locked.slot else {
            panic!("seated")
        };
        guest.route_subscriptions.push(99);
    }
    crate::runtime::route_to(MODULE, "somewhere".into());
    native.run_until_parked();
    assert_eq!(
        ticks_of(&seat, &native),
        before + 1,
        "the route was not delivered to the open view"
    );
}

/// A clipboard answer is pushed by `clipboard::mount` after `redraw` has
/// decided whether the guest wants another frame: the answer still counts.
#[gpui_kit::test]
fn a_clipboard_answer_reaches_the_view(cx: &mut TestAppContext) {
    const MODULE: &str = "clipboard-test";
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
    crate::runtime::seat_code_for_test(MODULE, 320, code);
    {
        let registry = registry().lock().unwrap();
        let Slot::Ready(guest) = &mut registry[&(MODULE, 0)].lock().unwrap().slot else {
            panic!("seated")
        };
        guest.capabilities.push(Capability::Clipboard);
    }
    let (seat, _, native) = open(cx, MODULE, false);
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
/// drawn, so every frame the guest produced is drawn once, in order, and
/// none is skipped past on the way to the draw.
#[gpui_kit::test]
fn a_seat_ticks_once_per_draw_of_its_tree(cx: &mut TestAppContext) {
    const MODULE: &str = "fresh-draws-first-test";
    let _on = crate::perf::on_for_test();
    crate::runtime::seat_for_test(MODULE, 320);
    let (seat, _, mut native) = open(cx, MODULE, false);
    native.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    native.run_until_parked();
    assert_eq!((ticks_of(&seat, &native), renders_of(MODULE)), (1, 1));
    {
        let mounted = mounted_of(&seat, &native);
        let mut locked = mounted.lock().unwrap();
        let Slot::Ready(guest) = &mut locked.slot else {
            panic!("seated")
        };
        guest.props_subscription = Some(98);
        guest.route_subscriptions.push(99);
    }
    // the props turn is deferred; the route lands after it has run, still
    // before any draw, and asks for a turn of its own
    let routed = seat.clone();
    native.update(|_, cx| {
        seat.update(cx, |seat, cx| seat.set_props(b"moved".to_vec(), cx));
        cx.defer(move |cx| {
            crate::runtime::route_to(MODULE, "somewhere".into());
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
        "the props and the route each ticked the guest"
    );
    assert_eq!(
        renders_of(MODULE),
        3,
        "each fresh frame drew before the next turn replaced it"
    );
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
    let seat = cx.new(|cx| Seat::new("rail-wake-seat", cx));
    assert!(woken() > 0, "a claimed seat told the rail nothing");
    let key = seat.read_with(cx, |seat, _| (seat.module(), seat.instance()));
    // a retry (Loading again) and the load it starts (no node: a failure
    // installed) each tell the rail
    for thread in retry(key.0, key.1)._threads {
        thread.join().unwrap();
    }
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
