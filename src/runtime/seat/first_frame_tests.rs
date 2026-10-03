//! A view's first frame, in the seat harness: what the guest knew when it
//! drew it. Every test logs the guest's `ticks` and `frame_rev` at each
//! render of its tree (`render::Drawn`), so "which frame was drawn, and
//! when" is read off the log, not reasoned, and reads the events each
//! tick carried (`Guest::ticked_with`).
use super::*;
use gpui_kit::{
    Entity, IntoElement, ParentElement as _, Render, Styled as _, Subscription, TestAppContext,
    VisualTestContext, div, px, size,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// `(ticks, frame_rev)` of the seated guest as the window draws: logged by
/// the root's render, which gpui runs first in every draw, so it is the
/// state the seat's tree renders from in that draw.
type DrawnLog = Rc<RefCell<Vec<(u64, u64)>>>;

/// The window a seat draws in, and the body its view is placed in.
const WINDOW: (f32, f32) = (400., 300.);

struct Root {
    seat: Entity<Seat>,
    mounted: Arc<Mutex<Mounted>>,
    drawn: DrawnLog,
    _watch: Subscription,
}

impl Render for Root {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl IntoElement {
        let seat = self.seat.read(cx);
        if let Slot::Ready(guest) = &lock(&self.mounted).slot
            && seat.tree().is_some()
            && seat.standin().is_none()
        {
            self.drawn.borrow_mut().push((guest.ticks, guest.frame_rev));
        }
        let body = match (seat.standin(), seat.tree()) {
            (Some(standin), _) => standin.element(seat.module(), seat.instance(), seat.ax_mark()),
            (None, Some(tree)) => gpui_kit::AnyView::from(tree)
                .cached(gpui_kit::StyleRefinement::default().size_full())
                .into_any_element(),
            (None, None) => div().size_full().into_any_element(),
        };
        div().size_full().child(body)
    }
}

/// A [`WINDOW`] whose root draws one seat of `module`, placed in it with
/// the window as its body.
fn open(
    cx: &mut TestAppContext,
    module: &'static str,
) -> (Entity<Seat>, DrawnLog, Arc<Mutex<Mounted>>, Opened) {
    cx.update(gpui_kit::init);
    let drawn: DrawnLog = Default::default();
    let log = drawn.clone();
    let window = cx.open_window(size(px(WINDOW.0), px(WINDOW.1)), |window, cx| {
        let seat = cx.new(|cx| Seat::new(module, cx));
        let key = seat.read(cx);
        let mounted = registry().lock().unwrap()[&(key.module(), key.instance())].clone();
        seat.update(cx, |seat, cx| {
            seat.place(window.window_handle(), Some(WINDOW), cx)
        });
        Root {
            seat: seat.clone(),
            mounted,
            drawn: log,
            _watch: cx.observe(&seat, |_, _, cx| cx.notify()),
        }
    });
    let root = window.root(cx).unwrap();
    let (seat, mounted) = root.read_with(cx, |root, _| (root.seat.clone(), root.mounted.clone()));
    let handle: gpui_kit::AnyWindowHandle = window.into();
    let native = VisualTestContext::from_window(handle, cx);
    native.run_until_parked();
    (seat, drawn, mounted, Opened { native, handle })
}

/// The opened window: its test context and its handle.
struct Opened {
    native: VisualTestContext,
    handle: gpui_kit::AnyWindowHandle,
}

fn ticks_of(mounted: &Mutex<Mounted>) -> u64 {
    match &lock(mounted).slot {
        Slot::Ready(guest) => guest.ticks,
        _ => panic!("not seated"),
    }
}

/// The events each tick of the seated guest carried.
fn ticked_with(mounted: &Mutex<Mounted>) -> Vec<Vec<wire::Event>> {
    match &lock(mounted).slot {
        Slot::Ready(guest) => guest.ticked_with.clone(),
        _ => panic!("not seated"),
    }
}

/// The host's facts among `events`: the theme, the viewport, the offset.
fn facts(events: &[wire::Event]) -> (Option<bool>, Option<(f32, f32)>, Option<i32>) {
    let mut facts = (None, None, None);
    for event in events {
        match event {
            wire::Event::Theme { dark } => facts.0 = Some(*dark),
            wire::Event::Viewport { width, height } => facts.1 = Some((*width, *height)),
            wire::Event::Offset { minutes } => facts.2 = Some(*minutes),
            _ => {}
        }
    }
    facts
}

/// A view answering `frames` in order, one per tick, the last on every
/// tick after. `snapshot` hands over one empty state and `restore` takes
/// any, so the same code serves as a swap's old and new instance.
fn frames_in_order(module: &'static str, min_width: u32, frames: &[wire::Frame]) {
    let code = Module::new(crate::runtime::guest::engine(), frames_wat(frames)).unwrap();
    crate::runtime::seat_code_for_test(module, min_width, code);
}

fn full(root: wire::Node) -> wire::Frame {
    wire::Frame {
        root: Some(root),
        ..Default::default()
    }
}

fn boxed(key: &str) -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name(key.into())),
        style: crate::render::test_style(div().size_full().style().clone()),
        interactivity: Default::default(),
        children: Vec::new(),
    })
}

/// A view's first tick carries the host's facts: the theme, the body its
/// pane lays it out in, the reader's offset. A pane resized before the
/// draw tells the view the new body ahead of the frame that shows it, and
/// nothing else: a fact that did not move does not cross again.
#[gpui_kit::test]
fn a_views_first_tick_carries_the_hosts_facts(cx: &mut TestAppContext) {
    const MODULE: &str = "first-frame-facts-test";
    frames_in_order(MODULE, 320, &[full(boxed("card"))]);
    let (seat, drawn, mounted, opened) = open(cx, MODULE);
    let Opened { mut native, handle } = opened;
    native.run_until_parked();
    assert_eq!(*drawn.borrow(), vec![(1, 1)]);
    let ticks = ticked_with(&mounted);
    assert_eq!(ticks.len(), 1);
    let (theme, viewport, offset) = facts(&ticks[0]);
    assert!(theme.is_some(), "the theme: {:?}", ticks[0]);
    assert_eq!(viewport, Some(WINDOW), "the body: {:?}", ticks[0]);
    assert_eq!(offset, Some(crate::runtime::kernel::offset_minutes()));

    seat.update(&mut native, |seat, cx| {
        seat.place(handle, Some((420., WINDOW.1)), cx)
    });
    native.run_until_parked();
    let ticks = ticked_with(&mounted);
    assert_eq!(ticks.len(), 2, "the resize ticked the view once");
    assert_eq!(
        ticks[1],
        [wire::Event::Viewport {
            width: 420.,
            height: WINDOW.1,
        }],
        "the new body, and nothing that did not move"
    );
}

/// A view is never laid out narrower than its own minimum: the pane gives
/// its body `min_w`, and the viewport the guest hears is that width.
#[gpui_kit::test]
fn a_views_viewport_is_never_narrower_than_its_minimum(cx: &mut TestAppContext) {
    const MODULE: &str = "first-frame-min-width-test";
    frames_in_order(MODULE, 640, &[full(boxed("card"))]);
    let (_seat, _drawn, mounted, opened) = open(cx, MODULE);
    opened.native.run_until_parked();
    let ticks = ticked_with(&mounted);
    assert_eq!(facts(&ticks[0]).1, Some((640., WINDOW.1)), "{:?}", ticks[0]);
}

/// A deployment swap: the replacement's first tree is made by
/// `first_frame` knowing the theme, the body and the reader's offset the
/// drawn view had (`Guest::seed`), so the tree the install turn mounts is
/// the one the reader sees, and no corrective tick follows it: the install
/// draws once, at frame_rev 2, and the view has ticked once.
#[gpui_kit::test]
fn a_swaps_first_tree_is_drawn_themed_laid_out_and_in_local_time(cx: &mut TestAppContext) {
    const MODULE: &str = "first-frame-swap-test";
    let _on = crate::perf::on_for_test();
    frames_in_order(MODULE, 320, &[full(boxed("card"))]);
    let (_seat, drawn, mounted, opened) = open(cx, MODULE);
    let native = opened.native;
    native.run_until_parked();
    assert_eq!(*drawn.borrow(), vec![(1, 1)], "the old view is up");
    let (alive, old_ticks, old_theme) = {
        let locked = lock(&mounted);
        let Slot::Ready(old) = &locked.slot else {
            panic!("not seated");
        };
        (old.alive.clone(), old.ticks, old.theme_dark)
    };
    assert!(old_theme.is_some(), "the drawn view heard the theme");
    // the new deployment's load, as a block starts it: the same code, a
    // replacement of the same shape
    let module = Arc::new(
        Module::new(
            crate::runtime::guest::engine(),
            frames_wat(&[full(boxed("card"))]),
        )
        .unwrap(),
    );
    let generation = lock(&mounted).start();
    let load = Load {
        module: MODULE,
        seat: mounted.clone(),
        generation,
        asked_of: connection().lock().unwrap().clone(),
        code: Some((::abi::BlobId::Sha256([7; 32]), false)),
    };
    let first_tick = Rc::new(RefCell::new(None));
    let seen = first_tick.clone();
    load.land(|load, timing| {
        let mut fresh = Guest::instantiate(load.module, &module, load.module).unwrap();
        fresh.min_width = 320;
        let mut ticks = 0;
        let fresh = Guest::replacement(
            fresh,
            &alive,
            &mut ticks,
            &load.seat,
            &module,
            load.module,
            timing,
        )
        .expect("prepared");
        assert_eq!(ticks, old_ticks);
        assert!(fresh.staged, "its first tree is made, its requests held");
        assert_eq!(fresh.ticks, 1, "first_frame ticked it once");
        assert_eq!(fresh.theme_dark, old_theme);
        assert_eq!(fresh.viewport_sent, Some(WINDOW));
        assert!(fresh.pending.is_empty(), "nothing waits for a second tick");
        *seen.borrow_mut() = Some(facts(&fresh.ticked_with[0]));
        Ok(Loaded::Swap {
            fresh: Box::new(fresh),
            alive: alive.clone(),
            ticks,
            code: module.clone(),
            shown: load.module.into(),
        })
    });
    assert_eq!(
        *first_tick.borrow(),
        Some((
            old_theme,
            Some(WINDOW),
            Some(crate::runtime::kernel::offset_minutes())
        )),
        "the replacement made its first tree knowing the theme, its body and the offset"
    );
    // the install woke the seat: its turn mounts the staged tree without a
    // tick (`staged`), and the harness draws the dirtied window
    native.run_until_parked();
    {
        let locked = lock(&mounted);
        let Slot::Ready(fresh) = &locked.slot else {
            panic!("not seated");
        };
        assert!(
            !Arc::ptr_eq(&fresh.alive, &alive),
            "the new code took the seat"
        );
        assert_eq!(fresh.ticks, 1, "the install turn ticked nothing");
        assert!(
            fresh.pending.is_empty(),
            "the turn found nothing to tell it"
        );
    }
    assert_eq!(
        *drawn.borrow(),
        vec![(1, 1), (1, 2)],
        "the staged tree, drawn once"
    );
    // nothing else wakes the seat, now or a frame later
    native.executor().advance_clock(Duration::from_millis(16));
    native.run_until_parked();
    assert_eq!(
        *drawn.borrow(),
        vec![(1, 1), (1, 2)],
        "no corrective tick, no second draw"
    );
    assert_eq!(ticks_of(&mounted), 1);
}

/// The WAT of [`frames_in_order`]: byte 0 is a result's Ok tag, so
/// `snapshot` answers Ok(empty) and `restore` Ok; frame `n` sits in slot
/// `n` above the first page.
fn frames_wat(frames: &[wire::Frame]) -> String {
    assert!(!frames.is_empty());
    const SLOT: usize = 16384;
    let mut data = String::new();
    let mut arms = String::new();
    for (nth, frame) in frames.iter().enumerate() {
        let (bytes, len) = crate::runtime::wat_frame(frame);
        assert!((len as usize) < SLOT, "test frame {nth} is {len} bytes");
        let at = 65536 + SLOT * nth;
        data += &format!("(data (i32.const {at}) \"{bytes}\")\n");
        let tick = wire::abi::pack(at as u32, len);
        if nth + 1 < frames.len() {
            arms += &format!(
                "global.get $n i32.const {} i32.le_u if (result i64) i64.const {tick} else ",
                nth + 1
            );
        } else {
            arms += &format!("i64.const {tick}{}", " end".repeat(nth));
        }
    }
    let pages = 1 + (SLOT * frames.len()).div_ceil(65536);
    let ok = wire::abi::pack(0, 1);
    format!(
        r#"(module
        (memory (export "memory") {pages})
        (global $n (mut i32) (i32.const 0))
        {data}
        (func (export "alloc") (param i32) (result i32) i32.const 64)
        (func (export "init"))
        (func (export "tick") (param i32 i32) (result i64)
            global.get $n i32.const 1 i32.add global.set $n
            {arms})
        (func (export "snapshot") (result i64) i64.const {ok})
        (func (export "restore") (param i32 i32) (result i64) i64.const {ok}))"#
    )
}
