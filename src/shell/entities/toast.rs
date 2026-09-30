//! The one-line notice up; empty when none. `show` puts one up and takes
//! it down 3.6 s later (today's twelve 300 ms ticks); a new one restarts
//! the count. It hears the session's and the account's notices
//! (`SessionEvent::Toast`, `AccountEvent::Toast`) itself.
use super::{Account, AccountEvent, Session, SessionEvent};
use gpui_kit::{Context, Entity, Subscription, Task};
use std::time::Duration;

/// How long a notice stays up.
pub(crate) const SHOWN_FOR: Duration = Duration::from_millis(3600);

pub(crate) struct Toast {
    text: String,
    /// Takes the notice down; dropped (cancelled) by the next `show`.
    dismiss: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

impl Toast {
    pub(crate) fn new(
        session: &Entity<Session>,
        account: &Entity<Account>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            text: String::new(),
            dismiss: None,
            _subscriptions: [
                cx.subscribe(session, |this, _, event: &SessionEvent, cx| {
                    if let SessionEvent::Toast(said) = event {
                        this.show(said.clone(), cx);
                    }
                }),
                cx.subscribe(account, |this, _, event: &AccountEvent, cx| {
                    if let AccountEvent::Toast(said) = event {
                        this.show(said.clone(), cx);
                    }
                }),
            ],
        }
    }

    /// The notice up; empty when none.
    pub(crate) fn get(&self) -> &String {
        &self.text
    }

    /// `said` up, for `SHOWN_FOR`.
    pub(crate) fn show(&mut self, said: String, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.text != said {
            self.text = said;
            cx.notify();
        }
        self.dismiss = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHOWN_FOR).await;
            let _ = this.update(cx, |this, cx| this.dismiss(cx));
        }));
    }

    /// The notice down.
    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.dismiss = None;
        if !self.text.is_empty() {
            self.text.clear();
            cx.notify();
        }
    }
}

/// Timed with the reducer's desk arms it took over (docs/perf.md).
fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.desk")
}
