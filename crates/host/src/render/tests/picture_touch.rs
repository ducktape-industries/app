//! A finger on a picture inside a message row, as chat draws one on a
//! phone: a tap and a long press go to the row, and nothing panics (J-075).
use super::*;

const SIDE: u32 = 60;

fn row_with_picture() -> wire::Node {
    let pixels = [255, 0, 0, 255].repeat((SIDE * SIDE) as usize);
    let picture = wire::Node::Image {
        id: Some(named_id("picture")),
        hash: 75,
        data: Some(wire::ImageData::Rgba {
            width: SIDE,
            height: SIDE,
            pixels,
        }),
        label: Some("image.png".into()),
        image_style: wire::ImageStyle {
            grayscale: false,
            object_fit: wire::ImageObjectFit::ScaleDown,
        },
        loading: false,
        fallback: true,
        state_children: Vec::new(),
        style: crate::render::test_style(div().size(px(SIDE as f32)).style().clone()),
        interactivity: Default::default(),
    };
    let mut row = container_with_style("row", div().size_full().style().clone(), [picture]);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut row {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.on_click = Some(71);
        interactivity.on_aux_click = Some(72);
        interactivity.focusable = true;
        interactivity.role = Some(gpui_kit::Role::Group);
    }
    row
}

fn touch(native: &mut gpui_kit::VisualTestContext, phase: gpui_kit::TouchPhase) {
    let event = gpui_kit::TouchEvent {
        id: gpui_kit::TouchId(7),
        phase,
        position: point(px(20.), px(20.)),
        predicted_position: None,
        force: None,
    };
    native.update(|window, cx| {
        let _ = window.dispatch_event(event.to_platform_input(), cx);
        crate::render::draw_frame(window, cx);
    });
    native.run_until_parked();
}

#[gpui_kit::test]
fn a_tap_and_a_hold_on_a_picture_in_a_row_go_to_the_row(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(200.), px(120.)), |_, _| {
        ViewTree::new(row_with_picture())
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    touch(&mut native, gpui_kit::TouchPhase::Started);
    touch(&mut native, gpui_kit::TouchPhase::Ended);
    native
        .executor()
        .advance_clock(std::time::Duration::from_millis(200));
    native.run_until_parked();
    touch(&mut native, gpui_kit::TouchPhase::Started);
    let held = gpui_kit::GestureTuning::default().long_press_duration;
    native
        .executor()
        .advance_clock(held + std::time::Duration::from_millis(1));
    native.run_until_parked();
    touch(&mut native, gpui_kit::TouchPhase::Ended);
    let heard: Vec<String> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::Click { handler, .. } => Some(format!("click {handler}")),
            wire::Event::AuxClick { handler, .. } => Some(format!("aux {handler}")),
            _ => None,
        })
        .collect();
    assert_eq!(heard, ["click 71", "aux 72"]);
}
