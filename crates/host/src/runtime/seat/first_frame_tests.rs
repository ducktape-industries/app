//! A view's first frame, in the seat harness: what the view knew when it
//! drew it. Every test logs the view's `ticks` and `frame_rev` at each
//! render of its tree (`render::Drawn`), so "which frame was drawn, and
//! when" is read off the log, not reasoned, and reads the events each
//! tick carried (`View::ticked_with`).
use super::*;
use gpui_kit::{
    Entity, IntoElement, ParentElement as _, Render, Styled as _, Subscription, TestAppContext,
    VisualTestContext, div, px, size,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

fn instance() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

type DrawnLog = Rc<RefCell<Vec<(u64, u64)>>>;

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
        if let Slot::Ready(view) = &lock(&self.mounted).slot
            && seat.tree().is_some()
            && seat.standin().is_none()
        {
            self.drawn.borrow_mut().push((view.ticks, view.frame_rev));
        }
        let body = match (seat.standin(), seat.tree()) {
            (Some(standin), _) => standin.element(seat.program(), seat.instance(), seat.ax_mark()),
            (None, Some(tree)) => gpui_kit::AnyView::from(tree)
                .cached(gpui_kit::StyleRefinement::default().size_full())
                .into_any_element(),
            (None, None) => div().size_full().into_any_element(),
        };
        div().size_full().child(body)
    }
}

fn open(
    cx: &mut TestAppContext,
    program: &'static str,
) -> (Entity<Seat>, DrawnLog, Arc<Mutex<Mounted>>, Opened) {
    open_in(cx, program, Some(WINDOW))
}

fn open_in(
    cx: &mut TestAppContext,
    program: &'static str,
    body: Option<(f32, f32)>,
) -> (Entity<Seat>, DrawnLog, Arc<Mutex<Mounted>>, Opened) {
    cx.update(gpui_kit::init);
    let drawn: DrawnLog = Default::default();
    let log = drawn.clone();
    let window = cx.open_window(size(px(WINDOW.0), px(WINDOW.1)), |window, cx| {
        let seat = cx.new(|cx| Seat::new(program, instance(), cx));
        let key = seat.read(cx);
        let mounted = registry().lock().unwrap()[&(key.program(), key.instance())].clone();
        seat.update(cx, |seat, cx| {
            seat.place(window.window_handle(), body, true, cx)
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

struct Opened {
    native: VisualTestContext,
    handle: gpui_kit::AnyWindowHandle,
}

fn ticks_of(mounted: &Mutex<Mounted>) -> u64 {
    match &lock(mounted).slot {
        Slot::Ready(view) => view.ticks,
        _ => panic!("not seated"),
    }
}

fn ticked_with(mounted: &Mutex<Mounted>) -> Vec<Vec<wire::Event>> {
    match &lock(mounted).slot {
        Slot::Ready(view) => view.ticked_with.clone(),
        _ => panic!("not seated"),
    }
}

type Facts = (Option<bool>, Option<(f32, f32)>, Option<i32>, Option<bool>);

fn facts(events: &[wire::Event]) -> Facts {
    let mut facts = (None, None, None, None);
    for event in events {
        match event {
            wire::Event::Theme { dark } => facts.0 = Some(*dark),
            wire::Event::Viewport { width, height } => facts.1 = Some((*width, *height)),
            wire::Event::Offset { minutes } => facts.2 = Some(*minutes),
            wire::Event::Pointer { touch } => facts.3 = Some(*touch),
            _ => {}
        }
    }
    facts
}

fn frames_in_order(program: &'static str, min_width: u32, frames: &[wire::Frame]) {
    let code = crate::runtime::view::prepared_for_test(frames_wat(frames));
    crate::runtime::seat_code_for_test(program, min_width, code);
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

#[gpui_kit::test]
fn a_views_first_tick_carries_the_hosts_facts(cx: &mut TestAppContext) {
    const PROGRAM: &str = "first-frame-facts-test";
    frames_in_order(PROGRAM, 320, &[full(boxed("card"))]);
    let (seat, drawn, mounted, opened) = open(cx, PROGRAM);
    let Opened { mut native, handle } = opened;
    native.run_until_parked();
    assert_eq!(*drawn.borrow(), vec![(1, 1)]);
    let ticks = ticked_with(&mounted);
    assert_eq!(ticks.len(), 1);
    let (theme, viewport, offset, touch) = facts(&ticks[0]);
    assert!(theme.is_some(), "the theme: {:?}", ticks[0]);
    assert_eq!(viewport, Some(WINDOW), "the body: {:?}", ticks[0]);
    assert_eq!(offset, Some(crate::runtime::kernel::offset_minutes()));
    assert_eq!(touch, Some(false), "the pointer: {:?}", ticks[0]);

    seat.update(&mut native, |seat, cx| {
        seat.place(handle, Some((420., WINDOW.1)), true, cx)
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

#[gpui_kit::test]
fn under_touch_a_view_is_laid_out_at_its_body_under_its_minimum(cx: &mut TestAppContext) {
    const PROGRAM: &str = "first-frame-touch-minimum-test";
    crate::contract::testing::touch_first_for_test(true);
    frames_in_order(PROGRAM, 640, &[full(boxed("card"))]);
    let (_seat, _drawn, mounted, opened) = open(cx, PROGRAM);
    opened.native.run_until_parked();
    crate::contract::testing::touch_first_for_test(false);
    let (_, viewport, _, touch) = facts(&ticked_with(&mounted)[0]);
    assert_eq!(touch, Some(true));
    assert_eq!(viewport, Some(WINDOW), "the body, not the 640 minimum");
}

#[gpui_kit::test]
fn a_view_is_not_ticked_before_its_pane_has_a_body(cx: &mut TestAppContext) {
    const PROGRAM: &str = "first-frame-no-body-test";
    frames_in_order(PROGRAM, 320, &[full(boxed("card"))]);
    let (seat, drawn, mounted, opened) = open_in(cx, PROGRAM, None);
    let Opened { mut native, handle } = opened;
    native.run_until_parked();
    assert_eq!(
        ticked_with(&mounted),
        Vec::<Vec<wire::Event>>::new(),
        "no tick before the pane has a body"
    );
    assert!(drawn.borrow().is_empty());

    seat.update(&mut native, |seat, cx| {
        seat.place(handle, Some(WINDOW), true, cx)
    });
    native.run_until_parked();
    let ticks = ticked_with(&mounted);
    assert_eq!(ticks.len(), 1);
    assert_eq!(facts(&ticks[0]).1, Some(WINDOW), "{:?}", ticks[0]);
    assert_eq!(*drawn.borrow(), vec![(1, 1)]);
}

#[gpui_kit::test]
fn a_views_viewport_is_never_narrower_than_its_minimum(cx: &mut TestAppContext) {
    const PROGRAM: &str = "first-frame-min-width-test";
    frames_in_order(PROGRAM, 640, &[full(boxed("card"))]);
    let (_seat, _drawn, mounted, opened) = open(cx, PROGRAM);
    opened.native.run_until_parked();
    let ticks = ticked_with(&mounted);
    assert_eq!(facts(&ticks[0]).1, Some((640., WINDOW.1)), "{:?}", ticks[0]);
}

#[gpui_kit::test]
fn a_swaps_first_tree_is_drawn_themed_laid_out_and_in_local_time(cx: &mut TestAppContext) {
    const PROGRAM: &str = "first-frame-swap-test";
    let _on = crate::perf::on_for_test();
    frames_in_order(PROGRAM, 320, &[full(boxed("card"))]);
    let (_seat, drawn, mounted, opened) = open(cx, PROGRAM);
    let native = opened.native;
    native.run_until_parked();
    assert_eq!(*drawn.borrow(), vec![(1, 1)], "the old view is up");
    let (alive, old_theme) = {
        let locked = lock(&mounted);
        let Slot::Ready(old) = &locked.slot else {
            panic!("not seated");
        };
        (old.alive.clone(), old.theme_dark)
    };
    assert!(old_theme.is_some(), "the drawn view heard the theme");
    let program = crate::runtime::view::prepared_for_test(frames_wat(&[full(boxed("card"))]));
    let generation = lock(&mounted).start();
    let load = Load {
        program: PROGRAM,
        seat: mounted.clone(),
        generation,
        asked_of: connection().lock().unwrap().clone(),
        code: Some((::abi::BlobId::Sha256([7; 32]), false)),
    };
    load.land(|load, _| {
        let mut code = ViewCode::for_test("", "", 320);
        code.hash = program;
        Ok(Loaded::Swap {
            code,
            shown: load.program.into(),
        })
    });
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
        assert_eq!(
            fresh.ticks, 1,
            "first_frame ticked it once; the install turn nothing"
        );
        assert_eq!(fresh.theme_dark, old_theme);
        assert_eq!(fresh.touch_sent, Some(false));
        assert_eq!(fresh.viewport_sent, Some(WINDOW));
        assert!(
            fresh.pending.is_empty(),
            "the turn found nothing to tell it"
        );
        assert_eq!(
            facts(&fresh.ticked_with[0]),
            (
                old_theme,
                Some(WINDOW),
                Some(crate::runtime::kernel::offset_minutes()),
                Some(false)
            ),
            "the replacement made its first tree knowing the theme, its body, the offset and the pointer"
        );
    }
    assert_eq!(
        *drawn.borrow(),
        vec![(1, 1), (1, 2)],
        "the staged tree, drawn once"
    );
    native.executor().advance_clock(Duration::from_millis(16));
    native.run_until_parked();
    assert_eq!(
        *drawn.borrow(),
        vec![(1, 1), (1, 2)],
        "no corrective tick, no second draw"
    );
    assert_eq!(ticks_of(&mounted), 1);
}

pub(super) fn frames_wat(frames: &[wire::Frame]) -> String {
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
    let state = wire::abi::pack(32, 2);
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
        (func (export "snapshot") (result i64)
            (i32.store8 (i32.const 33) (global.get $n))
            i64.const {state})
        (func (export "restore") (param i32 i32) (result i64)
            (global.set $n (i32.load8_u (local.get 0)))
            i64.const {ok}))"#
    )
}

#[gpui_kit::test]
fn back_goes_to_the_roots_on_back_and_otherwise_leaves(cx: &mut TestAppContext) {
    const PROGRAM: &str = "first-frame-back-test";
    let paged = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("pages".into())),
        style: crate::render::test_style(div().size_full().style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
            on_back: Some(3),
            ..Default::default()
        })),
        children: Vec::new(),
    });
    frames_in_order(PROGRAM, 320, &[full(paged), full(boxed("root"))]);
    let (seat, _drawn, mounted, opened) = open(cx, PROGRAM);
    let Opened { mut native, .. } = opened;
    native.run_until_parked();
    let back = seat.update(&mut native, |seat, cx| seat.back(cx));
    assert_eq!(back, Back::Heard);
    native.run_until_parked();
    let ticks = ticked_with(&mounted);
    assert_eq!(ticks.len(), 2, "the Back ticked the view");
    assert_eq!(ticks[1], [wire::Event::Back { handler: 3 }]);
    let back = seat.update(&mut native, |seat, cx| seat.back(cx));
    assert_eq!(back, Back::Leave);
    native.run_until_parked();
    assert_eq!(
        ticked_with(&mounted).len(),
        2,
        "a Back it cannot take is not sent"
    );
}
