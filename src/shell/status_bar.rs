//! `pulse`, the node's status dot: the bar's breath (`layers::StatusDot`),
//! the node menu's and the reconnecting footer's (`layers::ToastView`).

/// The node, as a breath (the canvas's `.pulse`): 5px to 11px and back
/// over 2s, green to its soft tone, while it answers; red, 5px to 9px over
/// 0.8s while it does not. An 8px dot when motion is off or its window is behind.
pub(super) fn pulse(
    ok: bool,
    moving: bool,
    ink: &super::ink::Ink,
    window: &gpui_kit::Window,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    use gpui_kit::*;
    let (color, soft, period, big) = match ok {
        true => (ink.ok, ink.ok_soft, 2000, 11.),
        false => (ink.danger, ink.danger, 800, 9.),
    };
    let dot = div().rounded_full().flex_shrink_0();
    let well = div()
        .size(px(12.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    match moving && !cx.reduce_motion() && window.is_window_active() {
        false => well.child(dot.size(px(8.)).bg(color)).into_any_element(),
        true => {
            ducktape_host::frames::notify(30, window, cx);
            let at = ducktape_host::frames::phase(std::time::Duration::from_millis(period), cx);
            let delta = bounce(ease_in_out)(at);
            well.child(
                dot.size(px(5. + (big - 5.) * delta))
                    .bg(color.blend(soft.opacity(delta))),
            )
            .into_any_element()
        }
    }
}
