//! The desk itself: the open view, links, toasts, the windows and the tray.

use super::{AppMessage as Message, Ducktape, SeatRequest};
use crate::backend;
use crate::runtime::Intent;
use view_wire::Task;

impl Ducktape {
    /// Views, links, toasts, windows and the tray.
    pub(super) fn on_desk(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SetAppearance(mode) => {
                self.appearance = mode;
                backend::save_appearance(mode);
                Task::none()
            }
            Message::SetMotion(on) => {
                self.motion = on;
                backend::save_motion(on);
                Task::none()
            }
            Message::SelectView(module) => {
                self.active = Some(module);
                self.badges.remove(module);
                self.seat_request = Some(SeatRequest::Select(module));
                Task::none()
            }
            Message::ViewShown(module) => {
                self.active = Some(module);
                self.badges.remove(module);
                Task::none()
            }
            Message::ViewEvent(module, intent) => match intent {
                Intent::Badge(count) if count > 0 => {
                    self.badges.insert(module, count);
                    Task::none()
                }
                Intent::Badge(_) => {
                    self.badges.remove(module);
                    Task::none()
                }
                Intent::OpenLink(link) => self.update(Message::OpenLink(link)),
                // the dispatch redraws
                Intent::Notified => Task::none(),
            },
            Message::OpenLink(link) => {
                // `duck://<chain>/<program>/<tail>` on this chain, or the short
                // `duck://<view>/<route>`: the seat opens and its view is
                // handed the route. The window coming forward is the answer;
                // a notice only says what there is nothing to see of.
                use crate::runtime::Link;
                match crate::runtime::parse_link(&link) {
                    Link::View { module, route } => self.open_seat(module, route),
                    Link::Chain(parsed) if !crate::runtime::listed_view(&parsed.program) => {
                        self.notice(format!("No view here opens {} links.", parsed.program));
                    }
                    Link::Chain(parsed) => {
                        let module = crate::runtime::intern(&parsed.program);
                        let route = parsed.tail.join("/");
                        if parsed.chain.to_string() != self.chain {
                            // another chain's page is not this chain's to show
                            self.open_seat(module, None);
                            self.notice(format!(
                                "That link is to {}, not this network; opened {module}.",
                                parsed.chain.label
                            ));
                        } else if route.is_empty() || crate::runtime::valid_route(&route) {
                            self.open_seat(module, (!route.is_empty()).then_some(route));
                        } else {
                            self.open_seat(module, None);
                            self.notice(format!("{module} can't open that part of the link."));
                        }
                    }
                    Link::Web(url) => return crate::shell::open_url(url),
                    Link::Unknown => self.notice("This link is not one this app opens.".into()),
                }
                Task::none()
            }
            Message::ShowToast(said) => {
                self.toast = said;
                self.toast_age = 0;
                Task::none()
            }
            Message::DismissToast => {
                self.toast.clear();
                Task::none()
            }
            Message::ToastTick => {
                self.toast_age += 1;
                if self.toast_age > 12 {
                    self.toast.clear();
                }
                Task::none()
            }
            Message::WallTick => {
                self.wall_now += 1;
                Task::none()
            }
            Message::ConsoleOpened(key) => {
                self.console_win = Some(key);
                Task::none()
            }
            Message::WindowWasClosed(key) => {
                if self.console_win == Some(key) {
                    self.console_win = None;
                }
                if self.focused_win == Some(key) {
                    self.focused_win = None;
                }
                Task::none()
            }
            Message::WindowFocused(key) => {
                self.focused_win = Some(key);
                Task::none()
            }
            Message::WindowUnfocused(key) => {
                if self.focused_win == Some(key) {
                    self.focused_win = None;
                }
                Task::none()
            }
            Message::ModifierStateChanged(modifiers) => {
                self.cmd_held = crate::runtime::command_held(modifiers);
                Task::none()
            }
            Message::TrayOpen => self.raise_console(),
            Message::TrayQuit => crate::shell::quit(),
            _ => unreachable!("routed by `update`"),
        }
    }

    pub(super) fn open_seat(&mut self, module: &'static str, route: Option<String>) {
        if let Some(route) = route {
            crate::runtime::route_to(module, route);
        }
        self.active = Some(module);
        self.seat_request = Some(SeatRequest::Open(module));
    }

    fn notice(&mut self, said: String) {
        self.toast = said;
        self.toast_age = 0;
    }

    /// The console window brought forward, or opened if there is none.
    pub(super) fn raise_console(&mut self) -> Task<Message> {
        match self.console_win {
            Some(key) => crate::shell::raise(key),
            None => {
                let (key, opened) = crate::shell::open(crate::shell::WindowKind::Console);
                self.console_win = Some(key);
                opened.map(Message::ConsoleOpened)
            }
        }
    }
}
