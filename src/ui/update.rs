//! The reducer: one message in, the state moved, a task out. Only the
//! wall clock's beat is left, which moves nothing.

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
        match message {
            Message::WallTick => {
                let _timed = time(Key::Shell, "reducer.desk");
                Task::none()
            }
        }
    }

    /// What runs while the app does: the wall clock, and nothing else.
    /// (The node's status poll is `Session`'s own; the toast's count is
    /// `Toast`'s.)
    pub(crate) fn subscriptions(&self) -> Subscription<Message> {
        Subscription::run(wall_ticks)
    }

    /// What a clock's beat ([`Message::is_beat`]) can move on screen: the
    /// roster and its seats, which load on threads of their own and tell
    /// no window. (An open panel's ages count on its own clock: the node's
    /// and the bell's menus, `layers::Chrome`; the node's height is
    /// `Session`'s poll's, onto `Chain`; the toast is `Toast`'s.) A beat
    /// that finds it as the windows last drew it draws no frame
    /// (`Desktop::dispatch`, docs/perf.md).
    pub(crate) fn beat_face(&self) -> BeatFace {
        self.roster.changes()
    }
}

/// See [`Ducktape::beat_face`].
pub(crate) type BeatFace = u64;

impl Message {
    /// The clocks' messages: they come whether or not anything moved.
    pub(crate) fn is_beat(&self) -> bool {
        matches!(self, Message::WallTick)
    }
}

fn wall_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(std::time::Duration::from_secs(1)).map(|()| Message::WallTick)
}

use futures::StreamExt as _;
