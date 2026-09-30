//! The node's breath, over the bar: a view of its own so its pulse draws
//! itself and the window's root, never the bar. Uncached (a frame per
//! pulse is its whole job), 12×12 absolute at `DotSlot`, where the bar's
//! node button laid its empty well out (`Chrome` commits it after each
//! frame). Green while the node answers, red while it does not; an 8px
//! still disc with motion off.

use super::super::entities::{DotSlot, Observed, Prefs, Session, Slice, WindowEntities};
use super::super::ink::Ink;
use super::super::status_bar::pulse;
use super::super::{Desktop, WindowKey};
use gpui_kit::{
    Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window, div,
};

pub(in crate::shell) struct StatusDot {
    key: WindowKey,
    slot: Observed<DotSlot>,
    session: Observed<Slice<Session>>,
    prefs: Observed<Slice<Prefs>>,
}

impl StatusDot {
    pub(in crate::shell) fn new(
        model: &Entity<Desktop>,
        key: WindowKey,
        own: &WindowEntities,
        cx: &mut Context<Self>,
    ) -> Self {
        let entities = &model.read(cx).entities;
        let (session, prefs) = (entities.session.clone(), entities.prefs.clone());
        Self {
            key,
            slot: Observed::new(&own.dot, cx),
            session: Observed::new(&session, cx),
            prefs: Observed::new(&prefs, cx),
        }
    }
}

impl Render for StatusDot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.dot", 1);
        // nothing until the bar has laid its well out (the first frame)
        let Some(slot) = *self.slot.read(cx).get() else {
            return div();
        };
        let session = self.session.read(cx).get();
        let prefs = self.prefs.read(cx).get();
        // the breath's colour follows the bar's word: switching or in sync
        // breathe green, not answering breathes red
        let ok = session.connecting || !session.reconnecting;
        div()
            .absolute()
            .left(slot.origin.x)
            .top(slot.origin.y)
            .size(slot.size.width)
            .child(pulse(ok, prefs.motion, &Ink::of(prefs.dark())))
    }
}
