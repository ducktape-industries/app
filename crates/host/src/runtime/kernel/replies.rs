//! The per-view answer queue between the kernel's tasks and the redraw:
//! written off-thread, drained into the view's pending events at its next
//! redraw. A subscription hands over one item and waits for the redraw
//! that takes it before it reads its source again.
use super::*;

/// The kernel's answers to a view's requests, written off-thread and
/// drained into the view's pending events at its next redraw.
pub(in crate::runtime) struct Replies {
    events: Mutex<Vec<wire::Event>>,
    in_flight: AtomicUsize,
    changed: tokio::sync::watch::Sender<()>,
    /// Told on every redraw that takes the queue: a subscription parked in
    /// [`Replies::subscription_item`] wakes here and reads its source again.
    drained: tokio::sync::watch::Sender<()>,
}

impl Default for Replies {
    fn default() -> Self {
        Self {
            events: Mutex::default(),
            in_flight: AtomicUsize::new(0),
            changed: tokio::sync::watch::channel(()).0,
            drained: tokio::sync::watch::channel(()).0,
        }
    }
}

impl Replies {
    /// Fires on every answer, coalesced; each `Seat` (`seat/entity.rs`)
    /// awaits its own receiver and turns. The
    /// answer stays in the queue while no window presents the view.
    pub(in crate::runtime) fn changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.changed.subscribe()
    }

    /// Answers how many were waiting: the backlog at this drain.
    pub(in crate::runtime) fn drain_into(&self, pending: &mut Vec<wire::Event>) -> usize {
        let mut events = self.events.lock().expect("kernel replies");
        let backlog = events.len();
        pending.append(&mut events);
        self.drained.send_replace(());
        backlog
    }

    /// Told on every drain: what a parked subscription waits on.
    pub(super) fn drains(&self) -> tokio::sync::watch::Receiver<()> {
        self.drained.subscribe()
    }

    /// One item from a SUBSCRIPTION, which is the only producer that can
    /// outrun the redraw: it answers for as long as the view holds it,
    /// against a queue only a redraw empties. It PARKS here after the item
    /// until a redraw takes the queue, so its own source — a node socket, a
    /// poll loop — holds what comes next. `false` when the queue's drains
    /// can no longer come: the subscription should end.
    pub(super) async fn subscription_item(
        &self,
        drained: &mut tokio::sync::watch::Receiver<()>,
        claim: &Claim,
        id: u64,
        result: Answer,
    ) -> bool {
        drained.borrow_and_update();
        if !self.answer(claim, id, result, false) {
            return false;
        }
        drained.changed().await.is_ok()
    }

    /// One item from a task holding `claim` on request `id`; `done` ends
    /// it, and gives the claim up with it. `false`, and nothing queued,
    /// once the claim is gone: the task ended the request already, or a
    /// reconnect took it ([`Replies::take`]) and the task is the old
    /// connection's.
    pub(super) fn answer(&self, claim: &Claim, id: u64, result: Answer, done: bool) -> bool {
        let mut events = self.events.lock().expect("kernel replies");
        let mut hold = claim.0.lock().expect("a request's claim");
        if hold.taken {
            return false;
        }
        hold.taken = done;
        events.push(wire::Event::Response { id, result, done });
        self.changed.send_replace(());
        true
    }

    /// The kernel takes request `claim` from its task, to answer it or
    /// open it again (a reconnect): `None` when the task ended it first,
    /// else what the task left standing for it ([`Claim::stand`]). Under
    /// the queue's lock, as every answer of a task is, so the request is
    /// the task's or the kernel's, never both: no id is ended twice, and
    /// nothing the old task sends lands after it is taken.
    pub(super) fn take(&self, claim: &Claim) -> Option<Option<Answer>> {
        let _events = self.events.lock().expect("kernel replies");
        let mut hold = claim.0.lock().expect("a request's claim");
        if hold.taken {
            return None;
        }
        hold.taken = true;
        Some(hold.standing.take())
    }

    pub(super) fn admit(self: &std::sync::Arc<Self>) -> InFlight {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        InFlight(self.clone())
    }

    /// WHETHER THE VIEW IS STILL OWED A FRAME, which is the in-flight count
    /// AND the answers already lying here. The two are one fact to a caller
    /// and reading only the count loses a race it loses often: a request is
    /// spawned inside a redraw, and a node that answers before that redraw
    /// returns has already given the count back — leaving an answer nobody
    /// is coming back for. The widget then stops polling and the view sits
    /// on "Loading…" until an unrelated event wakes it; a test's pump
    /// returns and reads a screen that never got its rows.
    ///
    /// Under the events lock, because that is the lock [`Replies::settled`]
    /// takes to give a count back: with it held, empty and zero together
    /// mean nothing can arrive that no one is waiting for.
    pub(in crate::runtime) fn answer_owed(&self) -> bool {
        let events = self.events.lock().expect("kernel replies");
        !events.is_empty() || self.in_flight.load(Ordering::SeqCst) > 0
    }

    /// One item for a request the kernel is running; `done` ends it for the
    /// view. The in-flight count is [`Replies::settled`]'s to give back —
    /// a subscription's last item and its count are not the same moment.
    pub(super) fn item(&self, id: u64, result: Answer, done: bool) {
        let mut events = self.events.lock().expect("kernel replies");
        events.push(wire::Event::Response { id, result, done });
        self.changed.send_replace(());
    }

    /// One request off the in-flight count, under the lock a waiter holds.
    fn settled(&self) {
        let _events = self.events.lock().expect("kernel replies");
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.changed.send_replace(());
    }
}

/// A task's hold on its request: the right to answer it, which its last
/// answer gives up and a reconnect can take ([`Replies::answer`],
/// [`Replies::take`]).
#[derive(Clone, Default)]
pub(in crate::runtime) struct Claim(std::sync::Arc<Mutex<Hold>>);

#[derive(Default)]
struct Hold {
    taken: bool,
    standing: Option<Answer>,
}

impl Claim {
    /// What the request answers if it is taken from its task from now on:
    /// a submit whose frame is going out is in doubt, not unsent.
    pub(super) fn stand(&self, answer: Answer) {
        self.0.lock().expect("a request's claim").standing = Some(answer);
    }
}

/// One in-flight slot, held by a running request or subscription and given
/// back when its task ends — INCLUDING THE ABORT a cancel or a replaced view fires, which is
/// the only way a socket waiting on the node stops waiting. Without this
/// the count would outlive the socket and the widget would poll forever.
pub(super) struct InFlight(std::sync::Arc<Replies>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.settled();
    }
}

#[cfg(test)]
#[test]
fn reply_notifications_wake_each_presenter_and_keep_the_answer() {
    let replies = Replies::default();
    let mut first = replies.changes();
    let mut second = replies.changes();
    replies.item(7, Ok(vec![1, 2]), true);
    futures::executor::block_on(async {
        first.changed().await.expect("first presenter notified");
        second.changed().await.expect("second presenter notified");
    });
    let mut pending = Vec::new();
    replies.drain_into(&mut pending);
    assert!(
        matches!(pending.as_slice(), [wire::Event::Response { id: 7, result: Ok(bytes), done: true }] if bytes == &[1, 2])
    );
    assert!(!replies.answer_owed());
}

#[cfg(test)]
#[test]
fn a_dropped_request_gives_its_slot_back() {
    let replies = std::sync::Arc::new(Replies::default());
    let admitted: Vec<_> = (0..3).map(|_| replies.admit()).collect();
    assert!(replies.answer_owed());
    drop(admitted);
    assert!(!replies.answer_owed());
}

#[cfg(test)]
#[test]
fn a_subscription_item_waits_for_the_redraw_that_takes_it() {
    use std::future::Future as _;
    let replies = std::sync::Arc::new(Replies::default());
    let mut drained = replies.drains();
    let claim = Claim::default();
    let mut item = Box::pin(replies.subscription_item(&mut drained, &claim, 7, Ok(vec![1])));
    let waker = futures::task::noop_waker();
    let mut poll = std::task::Context::from_waker(&waker);
    assert!(
        item.as_mut().poll(&mut poll).is_pending(),
        "parked until a redraw"
    );
    let mut pending = Vec::new();
    assert_eq!(replies.drain_into(&mut pending), 1);
    assert_eq!(item.as_mut().poll(&mut poll), std::task::Poll::Ready(true));
}
