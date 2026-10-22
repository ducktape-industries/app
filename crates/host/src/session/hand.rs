//! The network in hand, of the several a shell may hold: its seats hear what
//! the notifiers ask and hand them the session, and every network's rail is
//! woken as the roster or a seat moves.
use super::{Seats, Session};
use futures::StreamExt as _;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use gpui_kit::{Context, Entity, Task, WeakEntity};

/// The session and seats of the network in hand, and each rail's wake.
pub struct Hand {
    in_hand: Option<(WeakEntity<Session>, WeakEntity<Seats>)>,
    rails: Vec<UnboundedSender<()>>,
    _changes: Task<()>,
    _intents: Task<()>,
}

impl Hand {
    pub fn new(mut changes: UnboundedReceiver<()>, cx: &mut Context<Self>) -> Self {
        let mut heard = crate::runtime::notifier::intents();
        let _intents = cx.spawn(async move |this, cx| {
            while let Some((program, intent)) = heard.next().await {
                let seats = this.update(cx, |this, _| {
                    this.in_hand.as_ref().map(|(_, seats)| seats.clone())
                });
                let Ok(seats) = seats else {
                    break;
                };
                if let Some(seats) = seats {
                    let _ = seats.update(cx, |seats, cx| seats.notifier_said(program, intent, cx));
                }
            }
        });
        let _changes = cx.spawn(async move |this, cx| {
            while changes.next().await.is_some() {
                while changes.try_recv().is_ok() {}
                let woke = this.update(cx, |this, _| {
                    this.rails.retain(|rail| rail.unbounded_send(()).is_ok())
                });
                if woke.is_err() {
                    break;
                }
            }
        });
        Self {
            in_hand: None,
            rails: Vec::new(),
            _changes,
            _intents,
        }
    }

    pub fn wake(&mut self) -> UnboundedReceiver<()> {
        let (send, receive) = mpsc::unbounded();
        self.rails.push(send);
        receive
    }

    pub fn take(
        &mut self,
        session: &Entity<Session>,
        seats: &Entity<Seats>,
        cx: &mut Context<Self>,
    ) {
        let left = self
            .in_hand
            .take()
            .filter(|(was, _)| was.entity_id() != session.entity_id());
        if let Some((was, seated)) = left {
            let _ = seated.update(cx, |seats, _| seats.hand_notifiers(false));
            let _ = was.update(cx, |session, _| session.hand_notifiers(false));
        }
        session.update(cx, |session, _| session.hand_notifiers(true));
        seats.update(cx, |seats, _| seats.hand_notifiers(true));
        self.in_hand = Some((session.downgrade(), seats.downgrade()));
    }
}
