//! The status item (macOS): the network and the block, Open, Appearance,
//! Quit. Elsewhere the tray is a no-op that still answers the shell.

use crate::backend::Appearance;
use crate::shell::entities::{Chain, Prefs, Session, SessionState, Shared, Slice};
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui_kit::{App, AppContext as _, Context, Entity, Subscription};

/// What the item shows, diffed before each native update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    /// picks the icon: online or offline
    connected: bool,
    tooltip: String,
    labels: [String; ROWS],
}

// The menu, top to bottom, by row: network, status, sep, Open, sep,
// Appearance ▸ (System, Light, Dark), sep, Quit. A click comes back as
// the row's index.
const NETWORK: usize = 0;
const STATUS: usize = 1;
const OPEN: usize = 3;
const APPEARANCE: usize = 5;
const SYSTEM: usize = 6;
const LIGHT: usize = 7;
const DARK: usize = 8;
const QUIT: usize = 10;
const ROWS: usize = 11;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SEPARATORS: [usize; 3] = [2, 4, 9];
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SUBMENU: (usize, [usize; 3]) = (APPEARANCE, [SYSTEM, LIGHT, DARK]);
/// every row the menu lists directly: not the submenu's children, not Quit
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const TOP_LEVEL: [usize; 7] = [NETWORK, STATUS, 2, OPEN, 4, APPEARANCE, 9];
/// the two rows of status text: listed, not clickable
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const STATUS_ROWS: [usize; 2] = [NETWORK, STATUS];

impl Snapshot {
    /// The session's line, as the connect screen and the footer say it:
    /// "Not connected", "Reaching …", "Connected · block N", "Reconnecting…".
    fn status_line(session: &SessionState, chain: &Chain) -> String {
        match (session.connecting, session.connected, session.reconnecting) {
            (true, _, _) => format!("Reaching {}…", session.reaching),
            (false, false, _) => "Not connected".into(),
            (false, true, true) => "Reconnecting…".into(),
            (false, true, false) => format!("Connected · block {}", chain.height),
        }
    }

    pub(crate) fn of(session: &SessionState, chain: &Chain, appearance: Appearance) -> Self {
        let mut labels: [String; ROWS] = std::array::from_fn(|_| String::new());
        labels[NETWORK] = match session.network.is_empty() {
            true => "No network".into(),
            false => session.network.clone(),
        };
        labels[STATUS] = Self::status_line(session, chain);
        labels[OPEN] = "Open Ducktape".into();
        labels[APPEARANCE] = "Appearance".into();
        for (row, label, mode) in [
            (SYSTEM, "System", Appearance::System),
            (LIGHT, "Light", Appearance::Light),
            (DARK, "Dark", Appearance::Dark),
        ] {
            labels[row] = match appearance == mode {
                true => format!("✓ {label}"),
                false => label.into(),
            };
        }
        labels[QUIT] = "Quit Ducktape".into();
        Self {
            connected: session.connected,
            tooltip: format!("{} — {}", labels[NETWORK], labels[STATUS]),
            labels,
        }
    }
}

/// A row of the menu clicked: what it asks of the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    /// The console brought forward, or opened if there is none.
    Open,
    Appearance(Appearance),
    Quit,
}

pub fn pick(row: usize) -> Option<Pick> {
    match row {
        OPEN => Some(Pick::Open),
        SYSTEM => Some(Pick::Appearance(Appearance::System)),
        LIGHT => Some(Pick::Appearance(Appearance::Light)),
        DARK => Some(Pick::Appearance(Appearance::Dark)),
        QUIT => Some(Pick::Quit),
        _ => None,
    }
}

/// The item, kept in step with the session, the chain and the appearance
/// by observing them.
pub struct Tray {
    snapshot: Option<Snapshot>,
    #[cfg(target_os = "macos")]
    native: Option<native::StatusItem>,
    session: Entity<Session>,
    chain: Entity<Chain>,
    prefs: Entity<Slice<Prefs>>,
    _follows: [Subscription; 3],
}

/// The item over `app`'s session, chain and prefs, drawn once now and
/// again whenever one of them moves; and the rows clicked, by index.
pub fn init(app: &Shared, cx: &mut App) -> (Entity<Tray>, UnboundedReceiver<usize>) {
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
    let tray = cx.new(|cx| {
        let mut tray = Tray {
            snapshot: None,
            #[cfg(target_os = "macos")]
            native,
            session: app.session.clone(),
            chain: app.chain.clone(),
            prefs: app.prefs.clone(),
            _follows: [
                cx.observe(&app.session, |tray: &mut Tray, _, cx| tray.follow(cx)),
                cx.observe(&app.chain, |tray: &mut Tray, _, cx| tray.follow(cx)),
                cx.observe(&app.prefs, |tray: &mut Tray, _, cx| tray.follow(cx)),
            ],
        };
        tray.follow(cx);
        tray
    });
    (tray, receive)
}

impl Tray {
    fn follow(&mut self, cx: &mut Context<Self>) {
        let next = Snapshot::of(
            self.session.read(cx).get(),
            self.chain.read(cx),
            self.prefs.read(cx).get().appearance,
        );
        self.sync(next);
    }

    fn sync(&mut self, next: Snapshot) {
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
    use super::{ROWS, SEPARATORS, STATUS_ROWS, SUBMENU, Snapshot, TOP_LEVEL};
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
            // AppKit makes a menu on the main thread only, and muda panics
            // anywhere else (a test's thread): refuse as the tray itself does
            if unsafe { libc::pthread_main_np() } == 0 {
                return Err("not on the main thread".into());
            }
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
                    rows[row] = Some(Row::Item(item(row, !STATUS_ROWS.contains(&row))));
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
            if previous.is_none_or(|old| old.connected != next.connected) {
                self.tray
                    .set_icon(Some(self.icons[usize::from(next.connected)].clone()))
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
        assert_eq!(pick(OPEN), Some(Pick::Open));
        assert_eq!(pick(QUIT), Some(Pick::Quit));
        for (row, mode) in [
            (SYSTEM, Appearance::System),
            (LIGHT, Appearance::Light),
            (DARK, Appearance::Dark),
        ] {
            assert_eq!(pick(row), Some(Pick::Appearance(mode)));
        }
        for row in [NETWORK, STATUS, 2, 4, APPEARANCE, 9] {
            assert!(pick(row).is_none());
        }
        let mut session = SessionState {
            network: "dognet".into(),
            ..SessionState::default()
        };
        let chain = Chain::default();
        let snapshot = Snapshot::of(&session, &chain, Appearance::System);
        assert_eq!(snapshot.labels[NETWORK], "dognet");
        assert_eq!(snapshot.labels[STATUS], "Not connected");
        assert!(!snapshot.connected);
        session.connected = true;
        let chain = Chain {
            height: 7,
            ..Chain::default()
        };
        let snapshot = Snapshot::of(&session, &chain, Appearance::System);
        assert_eq!(snapshot.labels[STATUS], "Connected · block 7");
        session.reconnecting = true;
        let snapshot = Snapshot::of(&session, &chain, Appearance::System);
        assert_eq!(snapshot.labels[STATUS], "Reconnecting…");
        // the node the attempt out is for, not what the address field holds
        session.connecting = true;
        session.reaching = "http://b".into();
        session.endpoint = "http://bx".into();
        let snapshot = Snapshot::of(&session, &chain, Appearance::System);
        assert_eq!(snapshot.labels[STATUS], "Reaching http://b…");
    }

    /// The item draws itself as the app starts, and again from its own
    /// observers when the session, the chain or the appearance moves: no
    /// one else tells it.
    #[gpui_kit::test]
    fn the_tray_follows_the_session_the_chain_and_the_appearance(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let app = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::shell::entities::tests::entities(cx)
        });
        let (tray, _) = cx.update(|cx| init(&app, cx));
        let labels = |cx: &mut gpui_kit::TestAppContext| {
            tray.read_with(cx, |tray, _| {
                tray.snapshot.clone().expect("drawn at start").labels
            })
        };
        let drawn = labels(cx);
        assert_eq!(drawn[NETWORK], "No network");
        assert_eq!(drawn[STATUS], "Not connected");
        app.session.update(cx, |session, cx| {
            session.seed(
                SessionState {
                    network: "dognet".into(),
                    connected: true,
                    ..SessionState::default()
                },
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(labels(cx)[NETWORK], "dognet", "the session moved");
        app.chain.update(cx, |chain, cx| {
            chain.set(
                Chain {
                    height: 7,
                    ..Chain::default()
                },
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(labels(cx)[STATUS], "Connected · block 7", "the chain moved");
        app.prefs.update(cx, |prefs, cx| {
            let dark = Prefs {
                appearance: Appearance::Dark,
                ..prefs.get().clone()
            };
            prefs.set(dark, cx)
        });
        cx.run_until_parked();
        let drawn = labels(cx);
        assert_eq!(
            (drawn[DARK].as_str(), drawn[SYSTEM].as_str()),
            ("✓ Dark", "System"),
            "the appearance moved"
        );
    }
}
