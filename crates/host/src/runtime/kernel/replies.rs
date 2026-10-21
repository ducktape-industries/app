//! The per-view answer queue between the kernel's tasks and the redraw:
//! written off-thread, drained into the view's pending events at its next
//! redraw. A subscription hands over one item and waits for the redraw
//! that takes it before it reads its source again.
use super::*;

pub(in crate::runtime) struct Replies {
    events: Mutex<Vec<wire::Event>>,
    in_flight: AtomicUsize,
    changed: tokio::sync::watch::Sender<()>,
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
    pub(in crate::runtime) fn changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.changed.subscribe()
    }

    pub(in crate::runtime) fn drain_into(&self, pending: &mut Vec<wire::Event>) -> usize {
        let mut events = self.events.lock().expect("kernel replies");
        let backlog = events.len();
        pending.append(&mut events);
        self.drained.send_replace(());
        backlog
    }

    pub(super) fn drains(&self) -> tokio::sync::watch::Receiver<()> {
        self.drained.subscribe()
    }

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

    pub(super) fn items<'a>(
        &self,
        items: impl IntoIterator<Item = (&'a Claim, u64, Answer, bool)>,
    ) -> Vec<bool> {
        let mut events = self.events.lock().expect("kernel replies");
        let told: Vec<bool> = items
            .into_iter()
            .map(|(claim, id, result, done)| {
                let mut hold = claim.0.lock().expect("a request's claim");
                if hold.taken {
                    return false;
                }
                hold.taken = done;
                events.push(wire::Event::Response { id, result, done });
                true
            })
            .collect();
        if told.contains(&true) {
            self.changed.send_replace(());
        }
        told
    }

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

    pub(in crate::runtime) fn answer_owed(&self) -> bool {
        let events = self.events.lock().expect("kernel replies");
        !events.is_empty() || self.in_flight.load(Ordering::SeqCst) > 0
    }

    pub(super) fn item(&self, id: u64, result: Answer, done: bool) {
        let mut events = self.events.lock().expect("kernel replies");
        events.push(wire::Event::Response { id, result, done });
        self.changed.send_replace(());
    }

    #[cfg(feature = "ui")]
    pub(super) fn event(&self, event: wire::Event) {
        let mut events = self.events.lock().expect("kernel replies");
        events.push(event);
        self.changed.send_replace(());
    }

    fn settled(&self) {
        let _events = self.events.lock().expect("kernel replies");
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.changed.send_replace(());
    }
}

#[derive(Clone, Default)]
pub(in crate::runtime) struct Claim(std::sync::Arc<Mutex<Hold>>);

#[derive(Default)]
struct Hold {
    taken: bool,
    standing: Option<Answer>,
}

impl Claim {
    pub(super) fn stand(&self, answer: Answer) {
        self.0.lock().expect("a request's claim").standing = Some(answer);
    }
}

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
