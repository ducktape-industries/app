//! The bar at the window's foot, launcher and desk alike: the node not
//! answering (until it does) and a passing toast. Also `pulse`, the
//! node's status dot, which the menu bar and the node menu show too.

use super::*;

impl DesktopWindow {
    /// The bar at the window's foot (the Reconnecting board): the node not
    /// answering, which stays until it does, and a passing note (a toast:
    /// it fades, or is dismissed). `width: 560px; height: 56px; gap: 14px;
    /// border: 1.5px solid ink`, a soft shadow; a long note wraps.
    pub(super) fn footer(&self, cx: &gpui_kit::App) -> Option<gpui_kit::Div> {
        use super::ink::*;
        use gpui_kit::*;
        let state = &self.model.read(cx).state;
        let lost = state.reconnecting() && !matches!(state.stage, Stage::Connect);
        let toast = state.toast.clone();
        if !lost && toast.is_empty() {
            return None;
        }
        let ink = Ink::of(state.dark());
        let bar = |id: &'static str| {
            div()
                .id(id)
                .w(px(560.))
                .max_w_full()
                .min_h(px(super::ink::tall(56.)))
                .flex()
                .items_center()
                .gap(px(14.))
                .pl(px(18.))
                .pr(px(10.))
                .py(px(10.))
                .bg(ink.bg)
                .border(px(1.5))
                .border_color(ink.ink)
                .shadow_lg()
        };
        let text = |said: String| sans(400, 14.).flex_1().min_w_0().child(said);
        let lost = lost.then(|| {
            let said = format!(
                "{} isn't answering. What you write stays here.",
                state.network
            );
            bar("reconnecting")
                .control(Role::Status, SharedString::from(said.clone()))
                .child(pulse(false, state.motion, &ink))
                .child(text(said))
                .child(self.button(
                    "reconnect",
                    "Retry now",
                    Kind::Small,
                    || Message::Tick,
                    false,
                    &ink,
                ))
        });
        let toast = (!toast.is_empty()).then(|| {
            bar("toast")
                .control(Role::Status, SharedString::from(toast.clone()))
                // `Text` hands its words to the AX value, leaving the
                // node's name empty; a reader announces the name.
                .child(
                    text(toast.clone())
                        .id("toast-message")
                        .control(Role::Label, SharedString::from(toast)),
                )
                .child(self.button(
                    "toast-dismiss",
                    "Dismiss",
                    Kind::Small,
                    || Message::DismissToast,
                    false,
                    &ink,
                ))
        });
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(32.))
                .px_4()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .children(lost)
                .children(toast),
        )
    }
}

/// The node, as a breath (the canvas's `.pulse`): 5px to 11px and back
/// over 2s, green to its soft tone, while it answers; red, 5px to 9px over
/// 0.8s while it does not. An 8px dot when motion is off.
pub(super) fn pulse(ok: bool, moving: bool, ink: &super::ink::Ink) -> gpui_kit::AnyElement {
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
    match moving {
        false => well.child(dot.size(px(8.)).bg(color)).into_any_element(),
        true => well
            .child(
                dot.with_animation(
                    SharedString::from(format!("pulse/{ok}")),
                    Animation::new(std::time::Duration::from_millis(period))
                        .repeat()
                        .with_easing(bounce(ease_in_out))
                        .with_max_fps(30.),
                    move |dot, delta| {
                        dot.size(px(5. + (big - 5.) * delta))
                            .bg(color.blend(soft.opacity(delta)))
                    },
                ),
            )
            .into_any_element(),
    }
}
