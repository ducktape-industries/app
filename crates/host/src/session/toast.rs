//! The one-line notice up; empty when none. `show` puts one up and takes
//! it down 3.6 s later (today's twelve 300 ms ticks); a new one restarts
//! the count. It hears the session's and the account's notices
//! (`SessionEvent::Toast`, `AccountEvent::Toast`) itself.
use super::{Account, AccountEvent, Session, SessionEvent};
use gpui_kit::{Context, Entity, Subscription, Task, WeakEntity};
use std::time::Duration;

/// How long a notice stays up.
pub const SHOWN_FOR: Duration = Duration::from_millis(3600);

#[derive(Default)]
pub struct Toast {
    text: String,
    dismiss: Option<Task<()>>,
    followed: Vec<(WeakEntity<Session>, [Subscription; 2])>,
}

impl Toast {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            dismiss: None,
            followed: Vec::new(),
        }
    }

    pub fn follow(
        &mut self,
        session: &Entity<Session>,
        account: &Entity<Account>,
        cx: &mut Context<Self>,
    ) {
        self.followed
            .retain(|(session, _)| session.upgrade().is_some());
        let heard = [
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
        ];
        self.followed.push((session.downgrade(), heard));
    }

    /// The notice up; empty when none.
    pub fn get(&self) -> &String {
        &self.text
    }

    /// `said` up, for `SHOWN_FOR`.
    pub fn show(&mut self, said: String, cx: &mut Context<Self>) {
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
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.dismiss = None;
        if !self.text.is_empty() {
            self.text.clear();
            cx.notify();
        }
    }
}

fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.desk")
}
