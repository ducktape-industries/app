//! The desk itself: the open view, links, toasts, the windows and the tray.

use super::layout::PaneMessage;
use super::{AppMessage as Message, Ducktape};
use crate::backend;
use crate::runtime::Intent;
use crate::shell::Spot;
use crate::ui::task::Task;

impl Ducktape {
    /// Views, links, toasts, windows and the tray.
    pub(super) fn on_desk(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SetAppearance(mode) => {
                self.appearance = mode;
                backend::save_appearance(mode);
                crate::shell::sync_appearance()
            }
            Message::SetMotion(on) => {
                self.motion = on;
                backend::save_motion(on);
                Task::none()
            }
            // (a menu that asked for it closed as it did: its row, `WindowRoot`;
            // Help greeting a new account or not is `Account.welcome`)
            Message::OpenHelp => {
                self.open_help();
                Task::none()
            }
            Message::SelectView(module) => {
                // a pick (⌘K's, a menu's) opens as the bar's click does, not
                // in place of what the focused window shows
                self.open_seat(module, None);
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
                // read against the chain in hand by `Desktop::dispatch`
                Intent::OpenLink(link) => Task::done(Message::OpenLink(link)),
                // the dispatch redraws
                Intent::Notified => Task::none(),
                // a window placed before its view came is widened to it
                Intent::Seated => {
                    for layout in self.layouts.values_mut() {
                        if layout.panes.iter().any(|pane| pane.module == module) {
                            layout.settle();
                        }
                    }
                    Task::none()
                }
            },
            Message::OpenLink(_) => unreachable!("read against the chain by `Desktop::dispatch`"),
            // the network in hand was left: every window's panes go, each
            // desk keeping its measure; the badges and the active program
            // with them (what is open over each desk closes as it sees the
            // network go: `Overlays::new`)
            Message::LeftNetwork => {
                self.active = None;
                for layout in self.layouts.values_mut() {
                    layout.clear();
                }
                self.badges.clear();
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
            // moves nothing any more: an open menu's ages count on its own
            // clock (`layers::Chrome`); s12 deletes the beats
            Message::WallTick => Task::none(),
            Message::ConsoleOpened(key) => {
                self.console_win = Some(key);
                Task::none()
            }
            Message::WindowWasClosed(key) => {
                self.layouts.remove(&key);
                if self.console_win == Some(key) {
                    self.console_win = None;
                }
                Task::none()
            }
            // changes no state: its frame is the one a window switch is
            // timed to (`WindowRoot::start_switch`)
            Message::WindowFocused => Task::none(),
            Message::WindowUnfocused(key) => {
                // the keys left the window: a hold on one of its panes ends
                if let Some(layout) = self.layouts.get_mut(&key) {
                    layout.held = None;
                }
                Task::none()
            }
            Message::TrayOpen => self.raise_console(),
            Message::TrayQuit => crate::shell::quit(),
            _ => unreachable!("routed by `update`"),
        }
    }

    /// A link, read against the chain in hand:
    /// `duck://<chain>/<program>/<tail>` on this chain, or the short
    /// `duck://<view>/<route>`: the seat opens and its view is handed the
    /// route. The window coming forward is the answer; a notice only says
    /// what there is nothing to see of.
    pub(crate) fn open_link(&mut self, link: &str, chain: &str) -> Task<Message> {
        let _timed = crate::perf::time(crate::perf::Key::Shell, "reducer.desk");
        use crate::runtime::Link;
        match self.roster.parse_link(link) {
            Link::View { module, route } => self.open_seat(module, route),
            Link::Chain(parsed) if !self.roster.lists(&parsed.program) => {
                self.notice(format!("No view here opens {} links.", parsed.program));
            }
            Link::Chain(parsed) => {
                let module = crate::runtime::intern(&parsed.program);
                let route = parsed.tail.join("/");
                if parsed.chain.to_string() != chain {
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

    /// A Spotlight row picked, Spotlight already closed (`Overlays::submit`):
    /// what it does that the reducer still owns. Settings is the overlays'
    /// own and never comes here; the session's and the account's rows
    /// (switch, lock, create account, another network) are their entities'
    /// (`layers::overlays::run`).
    pub(super) fn on_spot(&mut self, spot: Spot) -> Task<Message> {
        let message = match spot {
            Spot::Open(module) => Message::SelectView(module),
            Spot::Appearance(mode) => Message::SetAppearance(mode),
            Spot::Help => Message::OpenHelp,
            Spot::FillWindow | Spot::HoldWindow => {
                let Some((key, index)) = self.framed_pane() else {
                    return Task::none();
                };
                Message::Pane(
                    key,
                    match spot {
                        Spot::FillWindow => PaneMessage::Fill(index),
                        _ => PaneMessage::Hold(index),
                    },
                )
            }
            Spot::Settings
            | Spot::Switch(_)
            | Spot::CreateAccount
            | Spot::Lock
            | Spot::OtherNetwork => {
                unreachable!("run by `layers::overlays::run` on the entity it moves")
            }
        };
        self.update(message)
    }

    pub(super) fn open_seat(&mut self, module: &'static str, route: Option<String>) {
        if let Some(route) = route {
            crate::runtime::route_to(module, route);
        }
        self.active = Some(module);
        // a link, a notice or a pick opens beside the view in front, not in
        // place of it
        if let Some(desk) = self.desk_layout() {
            desk.open(module);
            desk.settle();
        }
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
