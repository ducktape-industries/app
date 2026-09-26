//! The notification centre and its settings.

use super::{AppMessage as Message, Ducktape};
use view_wire::Task;

impl Ducktape {
    /// The notification centre's rows, and the policy Settings sets.
    pub(super) fn on_notify(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::NotifyOpen(id) => {
                self.close_menu();
                let entry = crate::runtime::notify::center().open(id);
                match entry {
                    Some(entry) if !entry.link.is_empty() => {
                        return self.update(Message::OpenLink(entry.link));
                    }
                    Some(entry) if crate::runtime::listed_view(&entry.module) => {
                        self.open_seat(crate::runtime::intern(&entry.module), None);
                    }
                    _ => {}
                }
                Task::none()
            }
            Message::NotifyMarkAllRead => {
                crate::runtime::notify::center().mark_all_read();
                Task::none()
            }
            Message::NotifyClearRead => {
                crate::runtime::notify::center().clear_read();
                Task::none()
            }
            Message::NotifySettings => {
                self.settings_page = super::SettingsPage::Notifications;
                self.update(Message::OpenSettings)
            }
            Message::NotifyPermission(module, permission) => {
                crate::runtime::notify::set_permission(module, permission);
                Task::none()
            }
            Message::NotifyNotNow(module) => {
                crate::runtime::notify::not_now(module);
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
