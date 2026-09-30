//! The reducer: one message in, the state moved, a task out. Each domain
//! has its own sub-reducer; this match only routes.

use super::{AppMessage as Message, Ducktape};
use crate::ui::task::Subscription;
use crate::ui::task::Task;

impl Ducktape {
    /// One message from outside, and what it asks of the native side.
    pub(crate) fn handle(&mut self, message: Message) -> Task<Message> {
        self.update(message)
    }

    /// Each domain's arm is timed as `reducer.<domain>` (docs/perf.md) and
    /// never keyed by the message: `AppMessage`'s `Debug` prints payloads.
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        use crate::perf::{Key, time};
        use Message as M;
        let timed = |domain: &'static str| time(Key::Shell, domain);
        match message {
            M::Spot(spot) => {
                let _timed = timed("reducer.overlay");
                self.on_spot(spot)
            }
            m @ (M::NotifyOpen(_)
            | M::NotifyMarkAllRead
            | M::NotifyClearRead
            | M::NotifyPermission(..)
            | M::NotifyNotNow(_)
            | M::SetNotifyBanners(_)
            | M::SetNotifyInFront(_)
            | M::SetNotifyBurst(_)) => {
                let _timed = timed("reducer.notify");
                self.on_notify(m)
            }
            m @ (M::SetAppearance(_)
            | M::SetMotion(_)
            | M::SelectView(_)
            | M::OpenHelp
            | M::ViewEvent(..)
            | M::OpenLink(_)
            | M::LeftNetwork
            | M::ShowToast(_)
            | M::DismissToast
            | M::ToastTick
            | M::WallTick
            | M::ConsoleOpened(_)
            | M::WindowWasClosed(_)
            | M::WindowFocused
            | M::WindowUnfocused(_)
            | M::TrayOpen
            | M::TrayQuit) => {
                let _timed = timed("reducer.desk");
                self.on_desk(m)
            }
            m @ (M::Pane(..) | M::DeskShown { .. }) => {
                let _timed = timed("reducer.pane");
                self.on_pane(m)
            }
        }
    }

    /// What runs while the app does: the clocks, and nothing else. A
    /// toast's count runs only while one shows. (The node's status poll is
    /// `Session`'s own.)
    pub(crate) fn subscriptions(&self) -> Subscription<Message> {
        let mut recipes = vec![Subscription::run(wall_ticks)];
        if !self.toast.is_empty() {
            recipes.push(Subscription::run(toast_ticks));
        }
        Subscription::batch(recipes)
    }

    /// What a clock's beat ([`Message::is_beat`]) can move on screen: the
    /// toast; and the roster and its seats, which load on threads of their
    /// own and tell no window. (An open panel's ages count on its own
    /// clock: the node's and the bell's menus, `layers::Chrome`; the
    /// node's height is `Session`'s poll's, onto `Chain`.) A beat that
    /// finds it as the windows last drew it draws no frame
    /// (`Desktop::dispatch`, docs/perf.md).
    pub(crate) fn beat_face(&self) -> BeatFace {
        (self.toast.clone(), self.roster.changes())
    }
}

/// See [`Ducktape::beat_face`].
pub(crate) type BeatFace = (String, u64);

impl Message {
    /// The clocks' messages: they come whether or not anything moved.
    pub(crate) fn is_beat(&self) -> bool {
        matches!(self, Message::WallTick | Message::ToastTick)
    }
}

fn wall_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(std::time::Duration::from_secs(1)).map(|()| Message::WallTick)
}

fn toast_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(std::time::Duration::from_millis(300)).map(|()| Message::ToastTick)
}

use futures::StreamExt as _;
