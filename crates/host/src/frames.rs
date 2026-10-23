//! One clock per window, counted in the frames its platform delivers: an
//! animation asks for the next frame of its rate's grid, and a window takes
//! frames only while something asks.

use gpui_kit::{App, EntityId, Global, Window, WindowId};
use rt::time::Instant;
use std::collections::HashMap;
use std::time::Duration;

type Due = Box<dyn FnOnce(&mut Window, &mut App)>;

#[derive(Default)]
struct Clocks(HashMap<WindowId, Clock>);

impl Global for Clocks {}

#[derive(Default)]
struct Clock {
    frames: u64,
    waiting: Vec<(u64, Option<EntityId>, Due)>,
}

struct Epoch(Instant);

impl Global for Epoch {}

pub fn stride(refresh: Option<Duration>, fps: u32) -> u64 {
    let Some(refresh) = refresh.filter(|refresh| !refresh.is_zero()) else {
        return 1;
    };
    let period = 1. / f64::from(fps.max(1));
    (period / refresh.as_secs_f64()).round().max(1.) as u64
}

pub fn notify(fps: u32, window: &Window, cx: &mut App) {
    let view = window.current_view();
    wait(
        fps,
        Some(view),
        window,
        cx,
        Box::new(move |_, cx| cx.notify(view)),
    );
}

pub fn then(
    fps: u32,
    window: &Window,
    cx: &mut App,
    due: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    wait(fps, None, window, cx, Box::new(due));
}

pub fn phase(period: Duration, cx: &mut App) -> f32 {
    let now = cx.background_executor().now();
    if !cx.has_global::<Epoch>() {
        cx.set_global(Epoch(now));
    }
    let since = now.duration_since(cx.global::<Epoch>().0).as_nanos();
    let period = period.as_nanos().max(1);
    (since % period) as f32 / period as f32
}

fn wait(fps: u32, key: Option<EntityId>, window: &Window, cx: &mut App, due: Due) {
    let stride = stride(window.refresh_interval(cx), fps);
    let id = window.window_handle().window_id();
    let clocks = &mut cx.default_global::<Clocks>().0;
    let idle = !clocks.contains_key(&id);
    let clock = clocks.entry(id).or_default();
    if key.is_some() && clock.waiting.iter().any(|(_, waiting, _)| *waiting == key) {
        return;
    }
    clock
        .waiting
        .push(((clock.frames / stride + 1) * stride, key, due));
    if idle {
        window.on_next_frame(move |window, cx| frame(id, window, cx));
    }
}

fn frame(id: WindowId, window: &mut Window, cx: &mut App) {
    let clocks = &mut cx.default_global::<Clocks>().0;
    let Some(clock) = clocks.get_mut(&id) else {
        return;
    };
    clock.frames += 1;
    let now = clock.frames;
    let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut clock.waiting)
        .into_iter()
        .partition(|(at, _, _)| *at <= now);
    match waiting.is_empty() {
        true => {
            clocks.remove(&id);
        }
        false => {
            clock.waiting = waiting;
            window.on_next_frame(move |window, cx| frame(id, window, cx));
        }
    }
    for (_, _, due) in due {
        due(window, cx);
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn deliver(native: &mut gpui_kit::VisualTestContext, over: Duration) -> usize {
    let interval = native
        .update(|window, cx| window.refresh_interval(cx))
        .expect("the test display reports its refresh interval");
    let frames = (over.as_nanos() / interval.as_nanos()).max(1);
    (0..frames)
        .map(|_| {
            native.executor().advance_clock(interval);
            let ran = native.update(|window, cx| window.simulate_next_frame(cx));
            native.run_until_parked();
            ran
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        Context, IntoElement, Render, TestAppContext, VisualTestContext, div, px, size,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn a_rate_is_every_nth_frame_the_display_delivers() {
        let hz = |n: u32| Some(Duration::from_secs(1) / n);
        assert_eq!(stride(hz(60), 30), 2);
        assert_eq!(stride(hz(120), 30), 4);
        assert_eq!(stride(hz(60), 60), 1);
        assert_eq!(stride(hz(30), 60), 1);
        assert_eq!(stride(None, 30), 1);
    }

    struct Blank;

    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    type Seen = Rc<RefCell<Vec<u64>>>;

    fn every(times: u32, seen: Seen, counted: Rc<RefCell<u64>>, window: &Window, cx: &mut App) {
        then(30, window, cx, move |window, cx| {
            let now = *counted.borrow();
            seen.borrow_mut().push(now);
            if times > 1 {
                every(times - 1, seen, counted, window, cx);
            }
        });
    }

    #[gpui_kit::test]
    fn animations_at_one_rate_share_frames_and_none_asks_for_none(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(100.), px(100.)), |_, _| Blank);
        let mut native = VisualTestContext::from_window(window.into(), cx);
        let counted = Rc::new(RefCell::new(0));
        let seen: [Seen; 2] = Default::default();
        native.update(|window, cx| {
            assert_eq!(stride(window.refresh_interval(cx), 30), 2);
            for seen in &seen {
                every(3, seen.clone(), counted.clone(), window, cx);
            }
        });
        let mut ran = Vec::new();
        for _ in 0..8 {
            *counted.borrow_mut() += 1;
            ran.push(native.update(|window, cx| window.simulate_next_frame(cx)));
        }
        for seen in &seen {
            assert_eq!(*seen.borrow(), [2, 4, 6]);
        }
        assert_eq!(ran, [1, 1, 1, 1, 1, 1, 0, 0]);
    }
}
