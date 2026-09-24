//! What opens over the desk: Spotlight, Settings, the network switcher,
//! the bar's menus, and closing each.

use super::{AppMessage as Message, Appearance, Ducktape, Overlay, Spot, SpotRow};
use view_wire::Task;

impl Ducktape {
    /// What is open over the desk, and what it holds.
    pub(super) fn on_overlay(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ToggleNetworkMenu => {
                self.toggle(Overlay::Network);
                Task::none()
            }
            Message::TogglePopover(which) => {
                self.toggle(Overlay::Menu(which));
                Task::none()
            }
            Message::OpenSpotlight => {
                self.overlay = Some(Overlay::Spotlight);
                self.spotlight_query.clear();
                self.spotlight_pick = 0;
                Task::none()
            }
            Message::SpotlightTyped(text) => {
                self.spotlight_query = text;
                self.spotlight_pick = 0;
                Task::none()
            }
            Message::SpotlightMove { down, rows } => {
                self.spotlight_pick = match (down, rows) {
                    (_, 0) => 0,
                    (true, rows) => (self.spotlight_pick + 1).min(rows - 1),
                    (false, _) => self.spotlight_pick.saturating_sub(1),
                };
                Task::none()
            }
            Message::SpotlightSubmit => {
                let rows = self.spotlight_rows();
                match rows.into_iter().nth(self.spotlight_pick) {
                    Some(row) => self.update(Message::Spot(row.spot)),
                    None => Task::none(),
                }
            }
            Message::Spot(spot) => {
                self.close(Overlay::Spotlight);
                let message = match spot {
                    Spot::Open(module) => Message::SelectView(module),
                    Spot::Switch(url) => Message::SwitchNetwork(url),
                    Spot::Settings => Message::OpenSettings,
                    Spot::CreateAccount => Message::ShowCreateAccount,
                    Spot::Lock => Message::Lock,
                    Spot::Appearance(mode) => Message::SetAppearance(mode),
                    Spot::OtherNetwork => Message::Disconnect,
                };
                self.update(message)
            }
            Message::CloseOverlay(overlay) => {
                self.close(overlay);
                Task::none()
            }
            Message::OpenSettings => {
                self.overlay = Some(Overlay::Settings);
                Task::none()
            }
            Message::ShowSettingsPage(page) => {
                self.settings_page = page;
                Task::none()
            }
            _ => unreachable!("routed by `update`"),
        }
    }

    /// `overlay` open, or closed if it was the one open.
    fn toggle(&mut self, overlay: Overlay) {
        self.overlay = match self.overlay == Some(overlay) {
            true => None,
            false => Some(overlay),
        };
    }

    /// `overlay` closed, if it is the one open, and what it held let go
    /// either way. Any menu off the bar closes for `Menu(_)`.
    pub(super) fn close(&mut self, overlay: Overlay) {
        match overlay {
            Overlay::Menu(_) => return self.close_menu(),
            Overlay::Spotlight => self.spotlight_query.clear(),
            Overlay::Approve => {
                self.sign_in.approve_found = None;
                self.sign_in.unlock_error.clear();
            }
            Overlay::Settings | Overlay::Network => {}
        }
        if self.overlay == Some(overlay) {
            self.overlay = None;
        }
    }

    /// Whichever menu off the bar is open, closed.
    pub(super) fn close_menu(&mut self) {
        if matches!(self.overlay, Some(Overlay::Menu(_))) {
            self.overlay = None;
        }
    }

    /// What ⌘K offers for the text typed, in the order shown: the
    /// network's programs, the other networks this device reached, then
    /// things to do. A row matches when its title or detail holds the text,
    /// ignoring case.
    pub(crate) fn spotlight_rows(&self) -> Vec<SpotRow> {
        let row = |group, title: String, meta: String, spot| SpotRow {
            group,
            title,
            meta,
            spot,
        };
        let mut rows: Vec<SpotRow> = crate::runtime::rail()
            .into_iter()
            .filter(|program| !program.empty)
            .map(|program| {
                row(
                    "Programs",
                    program.label.clone(),
                    "Open".into(),
                    Spot::Open(program.module),
                )
            })
            .collect();
        rows.extend(
            self.recent_endpoints
                .iter()
                .filter(|entry| entry.url != self.connected_rpc)
                .map(|entry| {
                    row(
                        "Networks",
                        entry.name(),
                        entry.host().to_owned(),
                        Spot::Switch(entry.url.clone()),
                    )
                }),
        );
        rows.push(row(
            "Actions",
            "Ducktape settings".into(),
            "theme, networks".into(),
            Spot::Settings,
        ));
        if matches!(self.account, Some(None)) && !self.signer_key.is_empty() {
            rows.push(row(
                "Actions",
                "Create account".into(),
                self.network.clone(),
                Spot::CreateAccount,
            ));
        }
        if !self.signer_key.is_empty() {
            rows.push(row(
                "Actions",
                "Lock".into(),
                "this device's key".into(),
                Spot::Lock,
            ));
        }
        for (title, mode) in [
            ("Light appearance", Appearance::Light),
            ("Dark appearance", Appearance::Dark),
            ("Match the system's appearance", Appearance::System),
        ] {
            rows.push(row(
                "Actions",
                title.into(),
                String::new(),
                Spot::Appearance(mode),
            ));
        }
        rows.push(row(
            "Actions",
            "Add a network…".into(),
            String::new(),
            Spot::OtherNetwork,
        ));
        let query = self.spotlight_query.trim().to_lowercase();
        rows.retain(|row| {
            query.is_empty()
                || row.title.to_lowercase().contains(&query)
                || row.meta.to_lowercase().contains(&query)
        });
        rows
    }
}
