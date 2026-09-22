use super::*;

/// What one answer holds against the reply budget.
fn result_bytes(result: &Answer) -> usize {
    match result {
        Ok(bytes) => bytes.len(),
        Err(refusal) => refusal.reason.len() + refusal.sentence.len(),
    }
}

/// What the queue holds against it.
fn queued_bytes(events: &[wire::Event]) -> usize {
    events
        .iter()
        .map(|event| match event {
            wire::Event::Response { result, .. } => result_bytes(result),
            _ => 0,
        })
        .sum()
}

/// The kernel's answers to a view's requests, written off-thread and
/// drained into the guest's pending events at its next redraw.
pub(in crate::runtime) struct Replies {
    events: Mutex<Vec<wire::Event>>,
    in_flight: AtomicUsize,
    /// Told on every answer delivered: a test waits here for the node
    /// calls in flight, never on a clock.
    landed: std::sync::Condvar,
    changed: tokio::sync::watch::Sender<()>,
    /// Told on every redraw that takes the queue: a subscription parked on
    /// [`Replies::backlogged`] wakes here and reads its socket again.
    drained: tokio::sync::watch::Sender<()>,
    fault: Mutex<Option<String>>,
}

impl Default for Replies {
    fn default() -> Self {
        Self {
            events: Mutex::default(),
            in_flight: AtomicUsize::new(0),
            landed: std::sync::Condvar::new(),
            changed: tokio::sync::watch::channel(()).0,
            drained: tokio::sync::watch::channel(()).0,
            fault: Mutex::default(),
        }
    }
}

impl Replies {
    /// Coalesced notifications wake each native presenter independently. The
    /// answer remains in the queue, including when no window is presenting it.
    pub(in crate::runtime) fn changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.changed.subscribe()
    }

    pub(in crate::runtime) fn drain_into(
        &self,
        pending: &mut Vec<wire::Event>,
    ) -> Result<(), String> {
        let mut events = self.events.lock().expect("kernel replies");
        if let Some(fault) = self.fault() {
            return Err(fault);
        }
        pending.append(&mut events);
        self.drained.send_replace(());
        Ok(())
    }

    /// Told on every drain: what a parked subscription waits on.
    pub(super) fn drains(&self) -> tokio::sync::watch::Receiver<()> {
        self.drained.subscribe()
    }

    /// Whether ONE subscription's share of the queue is spoken for, in
    /// either budget — a frame past this waits for a redraw rather than
    /// growing the queue toward the fault in [`Replies::item`].
    fn backlogged(&self) -> bool {
        let events = self.events.lock().expect("kernel replies");
        events.len() >= MAX_STREAM_BACKLOG_EVENTS
            || queued_bytes(&events) >= MAX_STREAM_BACKLOG_BYTES
    }

    /// One item from a SUBSCRIPTION, which is the only producer that can
    /// outrun the redraw: it answers for as long as the view holds it,
    /// against a queue only a redraw empties. It PARKS here while its share
    /// is spoken for, so its own source — a node socket, a companion
    /// session — holds the backlog instead of the queue growing into the
    /// fault. `false` when the view is gone and the subscription should end.
    pub(super) async fn subscription_item(
        &self,
        drained: &mut tokio::sync::watch::Receiver<()>,
        id: u64,
        result: Answer,
    ) -> bool {
        while self.backlogged() {
            if drained.changed().await.is_err() {
                return false;
            }
        }
        self.item(id, result, false);
        self.fault().is_none()
    }

    pub(in crate::runtime) fn fault(&self) -> Option<String> {
        self.fault.lock().expect("kernel reply fault").clone()
    }

    pub(super) fn admit(self: &std::sync::Arc<Self>) -> Option<InFlight> {
        if self.fault().is_some() {
            return None;
        }
        self.in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                (count < MAX_IN_FLIGHT).then_some(count + 1)
            })
            .ok()?;
        Some(InFlight(self.clone()))
    }

    /// Whether a query or a submit is still on its way.
    #[cfg(test)]
    pub(in crate::runtime) fn any_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) > 0
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
    /// guest. The in-flight count is [`Replies::settled`]'s to give back —
    /// a subscription's last item and its count are not the same moment.
    pub(super) fn item(&self, id: u64, result: Answer, done: bool) {
        let mut events = self.events.lock().expect("kernel replies");
        if self.fault().is_some() {
            return;
        }
        let queued = queued_bytes(&events);
        let exceeds_budget = events.len() >= MAX_REPLY_EVENTS
            || result_bytes(&result) > MAX_REPLY_BYTES.saturating_sub(queued);
        if exceeds_budget {
            *self.fault.lock().expect("kernel reply fault") =
                Some("view reply backlog limit exceeded; view stopped".into());
            self.landed.notify_all();
            self.changed.send_replace(());
            return;
        }
        events.push(wire::Event::Response { id, result, done });
        self.landed.notify_all();
        self.changed.send_replace(());
    }

    /// One request off the in-flight count, under the lock a waiter holds.
    fn settled(&self) {
        let _events = self.events.lock().expect("kernel replies");
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.landed.notify_all();
        self.changed.send_replace(());
    }
}

/// The in-flight count one subscription took, given back when its task
/// ends — INCLUDING THE ABORT a cancel or a replaced view fires, which is
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
    replies.drain_into(&mut pending).expect("reply budget");
    assert!(
        matches!(pending.as_slice(), [wire::Event::Response { id: 7, result: Ok(bytes), done: true }] if bytes == &[1, 2])
    );
    assert!(!replies.answer_owed());
}

#[cfg(test)]
#[test]
fn request_admission_is_bounded_and_drop_returns_capacity() {
    let replies = std::sync::Arc::new(Replies::default());
    let mut admitted: Vec<_> = (0..MAX_IN_FLIGHT)
        .map(|_| replies.admit().expect("within budget"))
        .collect();
    assert!(replies.admit().is_none());
    admitted.pop();
    let replacement = replies.admit().expect("dropped request returns capacity");
    drop(replacement);
    drop(admitted);
    assert!(!replies.any_in_flight());
}

#[cfg(test)]
#[test]
fn reply_overflow_stops_the_view_instead_of_losing_an_answer_silently() {
    let replies = std::sync::Arc::new(Replies::default());
    for id in 0..MAX_REPLY_EVENTS {
        replies.item(id as u64, Ok(Vec::new()), false);
    }
    assert!(replies.fault().is_none());
    replies.item(MAX_REPLY_EVENTS as u64, Ok(Vec::new()), true);
    assert!(replies.fault().is_some());
    assert!(replies.admit().is_none());
    let mut pending = Vec::new();
    assert!(replies.drain_into(&mut pending).is_err());
    assert!(pending.is_empty());
    assert_eq!(replies.events.lock().unwrap().len(), MAX_REPLY_EVENTS);
}

#[cfg(test)]
#[test]
fn queued_reply_bytes_are_bounded_across_individually_valid_items() {
    let replies = Replies::default();
    replies.item(1, Ok(vec![0; MAX_REPLY_BYTES]), false);
    assert!(replies.fault().is_none());
    replies.item(2, Ok(vec![1]), true);
    assert!(replies.fault().is_some());
    assert_eq!(replies.events.lock().unwrap().len(), 1);
}
