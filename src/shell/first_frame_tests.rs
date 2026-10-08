//! A pane's size is in the host's hands before its view's first tick, and
//! the view hears it: the console as the app opens it, a view whose load
//! lands after the desk has drawn, and the body its first tick carried
//! against the body its root was laid out in.
use super::entities::Screen;
use super::layers::tests::{Seed, open_console};
use super::layers::{BORDER, TITLE};
use gpui_kit::{Styled as _, TestAppContext, div};
use view_wire as wire;

fn counted(module: &str, stage: &str) -> Option<u64> {
    crate::perf::snapshot(false)["views"][module][stage].as_u64()
}

#[gpui_kit::test]
fn a_views_first_tick_carries_the_body_its_pane_lays_it_out_in(cx: &mut TestAppContext) {
    const MODULE: &str = "first-frame-size-test";
    let _on = crate::perf::on_for_test();
    let mut seed = Seed::boot();
    seed.roster = Default::default();
    seed.center = Default::default();
    seed.screen = Screen::Desk;
    seed.active = Some(MODULE);
    // the console opened and drawn once, its first frame's callbacks run:
    // the desk measured, the active program seeded into a pane, its seat
    // made and placed, nothing loaded for it (no node, no override)
    let (app, _key, view, mut native) = open_console(seed, cx);
    let layout = native.update(|_, cx| view.read(cx).layout(cx));
    let desk = layout
        .desk
        .expect("the desk is measured before it is seeded");
    assert_eq!(layout.panes.len(), 1);
    let pane = layout.panes[0].clone();
    assert_eq!(pane.module, MODULE);
    let before = pane
        .frame
        .expect("the pane has a frame before its view loads");
    assert_eq!(
        (before.w, before.h),
        (
            (desk.0 * 0.6).round().max(crate::ui::layout::MIN_WIDTH),
            (desk.1 * 0.7).round().max(crate::ui::layout::MIN_HEIGHT)
        ),
        "a new window's share of the desk"
    );
    assert_eq!(counted(MODULE, "ticks"), None, "the view has not ticked");
    let seat = app
        .seats
        .read_with(&native, |seats, _| seats.seat(pane.instance))
        .expect("the pane's seat is placed");
    assert!(
        seat.read_with(&native, |seat, _| seat.standin().is_some_and(|s| s.loading)),
        "the pane shows the load"
    );

    // the load lands: a view 900 wide (wider than its frame), its root an
    // identified box filling whatever it is given
    let card = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("card".into())),
        style: crate::render::test_style(div().size_full().style().clone()),
        interactivity: Default::default(),
        children: Vec::new(),
    });
    crate::runtime::seat_drawing_for_test(MODULE, 900, card);
    native.run_until_parked();
    assert_eq!(counted(MODULE, "ticks"), Some(1), "the first tick ran");
    let after = native.update(|_, cx| view.read(cx).layout(cx)).panes[0]
        .frame
        .expect("still framed");
    assert_eq!(
        after.w,
        900. + 2. * BORDER,
        "`Seated` widened the frame to the view's minimum"
    );
    assert_eq!(after.h, before.h);
    // the body the frame lays the view out in, never narrower than the
    // view's minimum: what the first tick carried
    let body = (
        (before.w - 2. * BORDER).max(900.),
        before.h - 2. * BORDER - TITLE,
    );
    let ticks = crate::runtime::ticks_for_test(MODULE);
    let viewport = ticks[0].iter().find_map(|event| match event {
        wire::Event::Viewport { width, height } => Some((*width, *height)),
        _ => None,
    });
    assert_eq!(
        viewport,
        Some(body),
        "the first tick carried the body: {:?}",
        ticks[0]
    );

    // and the body is what the view's root was given to draw in
    native.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    native.run_until_parked();
    let bounds = native.update(|window, _| {
        use gpui_kit::test::TestWindowExt as _;
        window.find("card").bounds()
    });
    assert_eq!(
        (f32::from(bounds.size.width), f32::from(bounds.size.height)),
        body,
        "the root is laid out in the body the tick named"
    );
}
