//! The bar at the window's foot, launcher and desk alike: the node not
//! answering (until it does) and a passing toast. A cached layer of its
//! own over `Toast`, `Session` and `Prefs`, laid over the whole window and
//! transparent to the pointer outside its boxes (no `occlude`, no listener
//! outside them); the root draws it deferred over an open menu, as it
//! painted before it was a layer.

use super::super::entities::{Observed, Prefs, Screen, Session, Slice, Toast};
use super::super::ink::{Ink, Kind, Press, button, sans, tall};
use super::super::status_bar::pulse;
use super::super::{Desktop, WindowKey};
use crate::AppMessage as Message;
use gpui_kit::*;

pub(in crate::shell) struct ToastView {
    model: Entity<Desktop>,
    key: WindowKey,
    toast: Observed<Toast>,
    session: Observed<Slice<Session>>,
    screen: Observed<Slice<Screen>>,
    prefs: Observed<Slice<Prefs>>,
}

impl ToastView {
    pub(in crate::shell) fn new(
        model: Entity<Desktop>,
        key: WindowKey,
        cx: &mut Context<Self>,
    ) -> Self {
        let entities = &model.read(cx).entities;
        let (toast, session, screen, prefs) = (
            entities.toast.clone(),
            entities.session.clone(),
            entities.screen.clone(),
            entities.prefs.clone(),
        );
        Self {
            toast: Observed::new(&toast, cx),
            session: Observed::new(&session, cx),
            screen: Observed::new(&screen, cx),
            prefs: Observed::new(&prefs, cx),
            model,
            key,
        }
    }
}

impl Render for ToastView {
    /// The bar at the window's foot (the Reconnecting board): the node not
    /// answering, which stays until it does, and a passing note (a toast:
    /// it fades, or is dismissed). `width: 560px; height: 56px; gap: 14px;
    /// border: 1.5px solid ink`, a soft shadow; a long note wraps.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.toast", 1);
        let session = self.session.read(cx).get();
        let prefs = self.prefs.read(cx).get();
        let lost = session.reconnecting && *self.screen.read(cx).get() != Screen::Connect;
        let toast = self.toast.read(cx).get().clone();
        let layer = div().absolute().inset_0();
        if !lost && toast.is_empty() {
            return layer;
        }
        let ink = Ink::of(prefs.dark());
        let bar = |id: &'static str| {
            div()
                .id(id)
                .w(px(560.))
                .max_w_full()
                .min_h(px(tall(56.)))
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
                session.network
            );
            crate::a11y::live(
                bar("reconnecting").role(Role::Status),
                gpui_kit::accesskit::Live::Polite,
                said.clone(),
            )
            .child(pulse(false, prefs.motion, &ink))
            .child(text(said))
            .child(button(
                &self.model,
                "reconnect",
                "Retry now",
                Kind::Small,
                || Message::Tick,
                Press::Ready,
                &ink,
            ))
        });
        let toast = (!toast.is_empty()).then(|| {
            crate::a11y::live(
                bar("toast").role(Role::Status),
                gpui_kit::accesskit::Live::Polite,
                toast.clone(),
            )
            .child(text(toast))
            .child(button(
                &self.model,
                "toast-dismiss",
                "Dismiss",
                Kind::Small,
                || Message::DismissToast,
                Press::Ready,
                &ink,
            ))
        });
        layer.child(
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
