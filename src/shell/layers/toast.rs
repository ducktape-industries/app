//! The bar at the window's foot, launcher and desk alike: the node not
//! answering (until it does) and a passing toast. A cached layer of its
//! own over `Toast`, `Session` and `Prefs`, laid over the whole window and
//! transparent to the pointer outside its boxes (no `occlude`, no listener
//! outside them); the root draws it deferred over an open menu, as it
//! painted before it was a layer.

use super::super::WindowKey;
use super::super::entities::{Entities, Observed, Prefs, Screen, Session, Slice, Toast};
use super::super::ink::{Ink, Kind, Press, button, sans, tall};
use super::super::status_bar::pulse;
use gpui_kit::*;

pub(in crate::shell) struct ToastView {
    key: WindowKey,
    toast: Observed<Toast>,
    session: Observed<Session>,
    screen: Observed<Slice<Screen>>,
    prefs: Observed<Slice<Prefs>>,
}

impl ToastView {
    pub(in crate::shell) fn new(app: &Entities, key: WindowKey, cx: &mut Context<Self>) -> Self {
        Self {
            toast: Observed::new(&app.toast, cx),
            session: Observed::new(&app.session, cx),
            screen: Observed::new(&app.screen, cx),
            prefs: Observed::new(&app.prefs, cx),
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
        // `size_full` too: cached, this view is a layout root of its own
        // (`OverlayLayer::render`)
        let layer = div().absolute().inset_0().size_full();
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
                "reconnect",
                "Retry now",
                Kind::Small,
                {
                    let session = self.session.entity().clone();
                    move |cx| session.update(cx, |session, cx| session.poll_now(cx))
                },
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
                "toast-dismiss",
                "Dismiss",
                Kind::Small,
                {
                    let toast = self.toast.entity().clone();
                    move |cx| toast.update(cx, |toast, cx| toast.dismiss(cx))
                },
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
