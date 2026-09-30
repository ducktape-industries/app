//! The notification centre and its settings.

use super::{AppMessage as Message, Ducktape};
use crate::ui::task::Task;

impl Ducktape {
    /// The notification centre's rows, and the policy Settings sets.
    pub(super) fn on_notify(&mut self, message: Message) -> Task<Message> {
        match message {
            // (the bell closed as its row was picked: `layers::Chrome`)
            Message::NotifyOpen(id) => {
                let entry = self.center.lock().open(id);
                match entry {
                    // read against the chain in hand by `Desktop::dispatch`
                    Some(entry) if !entry.link.is_empty() => {
                        return Task::done(Message::OpenLink(entry.link));
                    }
                    Some(entry) if self.roster.lists(&entry.module) => {
                        self.open_seat(crate::runtime::intern(&entry.module), None);
                    }
                    _ => {}
                }
                Task::none()
            }
            Message::NotifyMarkAllRead => {
                self.center.lock().mark_all_read();
                Task::none()
            }
            Message::NotifyClearRead => {
                self.center.lock().clear_read();
                Task::none()
            }
            Message::NotifyPermission(module, permission) => {
                crate::runtime::notify::set_permission(&mut self.center.lock(), module, permission);
                Task::none()
            }
            Message::NotifyNotNow(module) => {
                crate::runtime::notify::not_now(&mut self.center.lock(), module);
                Task::none()
            }
            Message::SetNotifyBanners(on) => {
                crate::runtime::notify::save_banners(on);
                Task::none()
            }
            Message::SetNotifyInFront(show) => {
                crate::runtime::notify::save_in_front(show);
                Task::none()
            }
            Message::SetNotifyBurst(burst) => {
                crate::runtime::notify::save_burst(burst);
                Task::none()
            }
            _ => unreachable!("routed by `update`"),
        }
    }
}
