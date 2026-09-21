//! The status item (macOS): the network and the block, Open, Appearance,
//! Quit. Elsewhere the tray is a no-op that still answers the shell.

use crate::{AppMessage as Message, Appearance, Ducktape};
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui_kit::App;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    icon: usize,
    tooltip: String,
    labels: [String; ROWS],
}

/// network, status, sep, Open, sep, Appearance ▸ (System, Light, Dark), sep, Quit
const ROWS: usize = 11;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SEPARATORS: [usize; 3] = [2, 4, 9];
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SUBMENU: (usize, [usize; 3]) = (5, [6, 7, 8]);
const QUIT: usize = 10;
/// every row the menu lists directly: not the submenu's children, not Quit
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const TOP_LEVEL: [usize; 7] = [0, 1, 2, 3, 4, 5, 9];

impl Snapshot {
    fn of(state: &Ducktape) -> Self {
        let mut labels: [String; ROWS] = std::array::from_fn(|_| String::new());
        labels[0] = match state.network.is_empty() {
            true => "No network".into(),
            false => state.network.clone(),
        };
        // the capture indicator reaches the status item too: a person whose
        // window is hidden still sees what is recording.
        labels[1] = match crate::module_view::capturing() {
            Some(recording) => format!("{} — {recording}", state.status),
            None => state.status.clone(),
        };
        labels[3] = "Open Ducktape".into();
        labels[5] = "Appearance".into();
        for (row, label, mode) in [
            (6, "System", Appearance::System),
            (7, "Light", Appearance::Light),
            (8, "Dark", Appearance::Dark),
        ] {
            labels[row] = match state.appearance == mode {
                true => format!("✓ {label}"),
                false => label.into(),
            };
        }
        labels[QUIT] = "Quit Ducktape".into();
        Self {
            icon: usize::from(state.connected),
            tooltip: format!("{} — {}", labels[0], labels[1]),
            labels,
        }
    }
}

pub fn message(row: usize) -> Option<Message> {
    match row {
        3 => Some(Message::TrayOpen),
        6 => Some(Message::SetAppearance(Appearance::System)),
        7 => Some(Message::SetAppearance(Appearance::Light)),
        8 => Some(Message::SetAppearance(Appearance::Dark)),
        QUIT => Some(Message::TrayQuit),
        _ => None,
    }
}

pub struct Tray {
    snapshot: Option<Snapshot>,
    #[cfg(target_os = "macos")]
    native: Option<native::StatusItem>,
}

pub fn init(_: &mut App) -> (Tray, UnboundedReceiver<usize>) {
    let (send, receive) = unbounded();
    #[cfg(target_os = "macos")]
    let native = match native::StatusItem::new(send) {
        Ok(tray) => Some(tray),
        Err(error) => {
            tracing::warn!(target:"ducktape::app",reason="tray_init_failed",%error,"native status item unavailable");
            None
        }
    };
    #[cfg(not(target_os = "macos"))]
    drop(send);
    (
        Tray {
            snapshot: None,
            #[cfg(target_os = "macos")]
            native,
        },
        receive,
    )
}

impl Tray {
    pub fn sync(&mut self, state: &Ducktape) {
        let next = Snapshot::of(state);
        if self.snapshot.as_ref() == Some(&next) {
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(native) = &mut self.native
            && let Err(error) = native.sync(self.snapshot.as_ref(), &next)
        {
            tracing::warn!(target:"ducktape::app",reason="tray_update_failed",%error,"native status item update refused");
            return;
        }
        self.snapshot = Some(next);
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::{ROWS, SEPARATORS, SUBMENU, Snapshot, TOP_LEVEL};
    use futures::channel::mpsc::UnboundedSender;
    use tray_icon::{
        Icon, TrayIcon, TrayIconBuilder,
        menu::{IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    };

    enum Row {
        Item(MenuItem),
        Submenu(Submenu),
        Separator(PredefinedMenuItem),
    }

    impl Row {
        fn handle(&self) -> &dyn IsMenuItem {
            match self {
                Row::Item(item) => item,
                Row::Submenu(submenu) => submenu,
                Row::Separator(separator) => separator,
            }
        }

        fn set_text(&self, text: &str) {
            match self {
                Row::Item(item) => item.set_text(text),
                Row::Submenu(submenu) => submenu.set_text(text),
                Row::Separator(_) => {}
            }
        }
    }

    pub(super) struct StatusItem {
        tray: TrayIcon,
        icons: [Icon; 2],
        rows: Vec<Row>,
    }

    impl StatusItem {
        pub(super) fn new(send: UnboundedSender<usize>) -> Result<Self, String> {
            let pixels: [&[u8]; 2] = [
                include_bytes!("../assets/tray-offline.rgba"),
                include_bytes!("../assets/tray.rgba"),
            ];
            let mut icons = pixels
                .into_iter()
                .map(|bytes| Icon::from_rgba(bytes.to_vec(), 128, 128));
            let icons = [
                icons.next().unwrap().map_err(|error| error.to_string())?,
                icons.next().unwrap().map_err(|error| error.to_string())?,
            ];
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let Some(row) = event
                    .id
                    .as_ref()
                    .strip_prefix("ducktape-tray-")
                    .and_then(|row| row.parse::<usize>().ok())
                else {
                    return;
                };
                let _ = send.unbounded_send(row);
            }));
            let item = |row: usize, enabled: bool| {
                MenuItem::with_id(format!("ducktape-tray-{row}"), "", enabled, None)
            };
            let mut rows: Vec<Option<Row>> = (0..ROWS).map(|_| None).collect();
            for row in SEPARATORS {
                rows[row] = Some(Row::Separator(PredefinedMenuItem::separator()));
            }
            let (parent, children) = SUBMENU;
            let submenu = Submenu::new("", true);
            for child in children {
                let child_item = item(child, true);
                submenu
                    .append(&child_item)
                    .map_err(|error| error.to_string())?;
                rows[child] = Some(Row::Item(child_item));
            }
            rows[parent] = Some(Row::Submenu(submenu));
            for row in TOP_LEVEL.into_iter().chain([super::QUIT]) {
                if rows[row].is_none() {
                    rows[row] = Some(Row::Item(item(row, row >= 2)));
                }
            }
            let rows: Vec<Row> = rows
                .into_iter()
                .map(|row| row.expect("every row"))
                .collect();
            let menu = Menu::new();
            for row in TOP_LEVEL.into_iter().chain([super::QUIT]) {
                menu.append(rows[row].handle())
                    .map_err(|error| error.to_string())?;
            }
            let tray = TrayIconBuilder::new()
                .with_id("ducktape")
                .with_icon(icons[0].clone())
                .with_icon_as_template(false)
                .with_menu_on_left_click(true)
                .with_menu(Box::new(menu))
                .build()
                .map_err(|error| error.to_string())?;
            Ok(Self { tray, icons, rows })
        }

        pub(super) fn sync(
            &mut self,
            previous: Option<&Snapshot>,
            next: &Snapshot,
        ) -> Result<(), String> {
            if previous.is_none_or(|old| old.icon != next.icon) {
                self.tray
                    .set_icon(Some(self.icons[next.icon].clone()))
                    .map_err(|error| error.to_string())?;
            }
            if previous.is_none_or(|old| old.tooltip != next.tooltip) {
                self.tray
                    .set_tooltip(Some(&next.tooltip))
                    .map_err(|error| error.to_string())?;
            }
            for row in 0..ROWS {
                if previous.is_none_or(|old| old.labels[row] != next.labels[row]) {
                    self.rows[row].set_text(&next.labels[row]);
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_routes_open_appearance_and_quit() {
        assert!(matches!(message(3), Some(Message::TrayOpen)));
        assert!(matches!(message(QUIT), Some(Message::TrayQuit)));
        for row in [0, 1, 2, 4, 5, 9] {
            assert!(message(row).is_none());
        }
        let (mut app, _) = Ducktape::boot();
        app.network = "dognet".into();
        let snapshot = Snapshot::of(&app);
        assert_eq!(snapshot.labels[0], "dognet");
        assert_eq!(snapshot.icon, 0);
    }
}
