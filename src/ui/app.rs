//! The app's state, and what moves it. Small on purpose: a node to reach,
//! a key to unlock, the programs the node runs, and which one is open.
//! Everything a person does inside a view is the view's.

use std::collections::BTreeMap;

use crate::backend;
use crate::runtime::ModuleViewEvent;
use crate::shell::WindowKey;

/// Unanswered status polls in a row before the node counts as lost: one
/// miss is a hiccup, two (four seconds) is a node that went away.
pub(crate) const LOST_AFTER: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
    System,
    Light,
    Dark,
}

/// A menu hanging off the menu bar; one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Popover {
    /// The breathing dot: how the node is doing.
    Node,
    /// The account's name: who is signed in, and Lock.
    Account,
}

/// The Settings window's sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    Appearance,
    Networks,
    About,
}

/// What a Spotlight row does when picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Spot {
    Open(&'static str),
    Switch(String),
    Settings,
    CreateAccount,
    Lock,
    Appearance(Appearance),
    OtherNetwork,
}

/// One Spotlight row, under its group's heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpotRow {
    pub(crate) group: &'static str,
    pub(crate) title: String,
    pub(crate) meta: String,
    pub(crate) spot: Spot,
}

/// Where the person is: reaching a node, or inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Screen {
    Connect,
    Console,
}

pub struct Ducktape {
    pub(crate) appearance: Appearance,
    pub(crate) system_dark: bool,
    pub(crate) screen: Screen,
    /// The node URL being typed.
    pub(crate) endpoint: String,
    pub(crate) endpoint_error: String,
    pub(crate) recent_endpoints: Vec<backend::RecentEndpoint>,
    /// The node reached: its origin and the network it serves.
    pub(crate) connected_rpc: String,
    pub(crate) network: String,
    /// Where this network's keys live on this device
    /// ([`backend::bind_keyring`]): the name alone is not enough, two
    /// chains can share one.
    pub(crate) keyring: String,
    /// The network shares its name with another chain this device met
    /// first: its keys are its own, and the sign-in screen says so.
    pub(crate) other_chain: bool,
    /// The menu bar's network menu is open.
    pub(crate) network_menu: bool,
    pub(crate) connected: bool,
    pub(crate) connecting: bool,
    pub(crate) status: String,
    pub(crate) height: i64,
    /// Status polls gone unanswered in a row; from [`LOST_AFTER`] on the
    /// footer says the node is not answering (the poll keeps running) until
    /// one lands.
    pub(crate) status_misses: u32,
    /// The node's last answer, whole: the node status menu reads it.
    pub(crate) node: Option<backend::NodeStatus>,
    /// `wall_now` when the height last moved: "last block 2 s ago".
    pub(crate) block_seen: i64,
    pub(crate) popover: Option<Popover>,
    /// ⌘K is open, and what it holds: the typed text, the picked row.
    pub(crate) spotlight: bool,
    pub(crate) spotlight_query: String,
    pub(crate) spotlight_pick: usize,
    pub(crate) settings_win: Option<WindowKey>,
    pub(crate) settings_page: SettingsPage,
    /// The drawings in characters turn; off keeps them on their first frame.
    pub(crate) motion: bool,
    pub(crate) error: String,
    /// The seated key's public half, hex; empty while locked.
    pub(crate) signer_key: String,
    /// The account the seated key belongs to, as `(number, name)`: `None`
    /// until the node was asked, `Some(None)` while the key holds none.
    pub(crate) account: Option<Option<(u64, String)>>,
    pub(crate) password: String,
    pub(crate) confirm_password: String,
    pub(crate) unlock_error: String,
    pub(crate) unlock_busy: bool,
    /// Whether this device already holds a key for `network` — Unlock vs
    /// Create on the sign-in screen.
    pub(crate) key_exists: bool,
    /// Reading without a key: the console opens, writes are refused.
    pub(crate) browsing: bool,
    /// On the sign-in screen: restoring a key from its recovery phrase
    /// instead of unlocking or minting one.
    pub(crate) restoring: bool,
    pub(crate) restore_phrase: String,
    pub(crate) restore_password: String,
    pub(crate) restore_confirm_password: String,
    /// The name a new passkey account takes.
    pub(crate) account_name: String,
    /// A passkey ceremony in flight (the browser has it); dropping the
    /// handle cancels it.
    pub(crate) passkey_task: Option<view_wire::task::Handle>,
    /// Armed by a sign-in that did not make an account (a new key past its
    /// phrase check, Unlock, Restore): the node's first answer about the
    /// seated key opens the account step if it holds none. One answer
    /// disarms it, so a later block never pulls the person out of a view.
    pub(crate) account_offer: bool,
    /// The account step is on screen: name an account for the seated key,
    /// or "Not now" to the console.
    pub(crate) account_step: bool,
    /// Set once the person picks "Use a phone instead"; the ceremony reads it.
    pub(crate) passkey_phone: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The QR URL of the touch in flight (its callback is the relay slot).
    pub(crate) passkey_qr: String,
    /// On the sign-in screen with a key already here: minting a new one in
    /// its place, after saying what that costs ("New key").
    pub(crate) replacing: bool,
    /// A freshly minted key's recovery phrase, held until the person has
    /// typed back the words `phrase_quiz` asks for.
    pub(crate) phrase: String,
    /// Once "I wrote it down" is pressed: the three word positions
    /// (0-based, ascending) the person types back before the console opens.
    pub(crate) phrase_quiz: Option<[usize; 3]>,
    pub(crate) quiz_answers: [String; 3],
    /// The program whose view is open.
    pub(crate) active: Option<&'static str>,
    pub(crate) badges: BTreeMap<&'static str, i64>,
    pub(crate) toast: String,
    pub(crate) toast_age: i64,
    pub(crate) console_win: Option<WindowKey>,
    pub(crate) focused_win: Option<WindowKey>,
    pub(crate) cmd_held: bool,
    pub(crate) connect_generation: u64,
    pub(crate) connect_task: Option<view_wire::task::Handle>,
    pub(crate) wall_now: i64,
}

impl std::fmt::Debug for Ducktape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ducktape")
    }
}

#[derive(Debug)]
pub(crate) enum AppMessage {
    SetAppearance(Appearance),
    EndpointTyped(String),
    ConnectSubmit,
    ConnectTo(String),
    Connected {
        generation: u64,
        origin: String,
        status: backend::NodeStatus,
    },
    ConnectFailed {
        generation: u64,
        error: String,
    },
    StatusPushed(backend::NodeStatus),
    StatusMissed,
    AccountResolved {
        /// The node asked: an answer from before a switch is not this one's.
        node: String,
        key: String,
        account: Option<(u64, String)>,
    },
    Disconnect,
    ToggleNetworkMenu,
    CloseNetworkMenu,
    TogglePopover(Popover),
    ClosePopover,
    OpenSpotlight,
    CloseSpotlight,
    SpotlightTyped(String),
    /// Up or down a row among `rows` shown.
    SpotlightMove {
        down: bool,
        rows: usize,
    },
    /// Enter: the picked row's action.
    SpotlightSubmit,
    Spot(Spot),
    OpenSettings,
    SettingsOpened(WindowKey),
    ShowSettingsPage(SettingsPage),
    SetMotion(bool),
    /// Another node from the switcher: reached first, and only once it
    /// answers does the console leave the network in hand.
    SwitchNetwork(String),
    SelectView(&'static str),
    SplitView(&'static str),
    ClosePane(usize),
    FocusPane(usize),
    PopOut(usize),
    PopIn(WindowKey),
    ViewEvent(&'static str, ModuleViewEvent),
    OpenLink(String),
    PasswordTyped(String),
    ConfirmPasswordTyped(String),
    UnlockSubmit,
    CreateWalletSubmit,
    Unlocked(String),
    WalletCreated {
        pubkey: String,
        phrase: String,
    },
    PhraseWrittenDown,
    PhraseWordTyped(usize, String),
    PhraseCheckSubmit,
    PhraseShowAgain,
    ShowNewKey,
    NewKeyCancel,
    UnlockFailed(String),
    BrowseWithoutKey,
    SignIn,
    ShowRestore,
    RestoreCancel,
    RestorePhraseTyped(String),
    RestorePasswordTyped(String),
    RestoreConfirmPasswordTyped(String),
    RestoreSubmit,
    Restored(String),
    AccountNameTyped(String),
    PasskeyCreateSubmit,
    PasskeySignInSubmit,
    PasskeyCancel,
    PasskeyUsePhone,
    PasskeyQr(String),
    PasskeyDone(String),
    PasskeyFailed(String),
    ShowCreateAccount,
    CreateAccountSubmit,
    CreateAccountLater,
    AccountCreated(Result<(u64, String), String>),
    ForgetEndpoint(String),
    Lock,
    ShowToast(String),
    DismissToast,
    ToastTick,
    Tick,
    WallTick,
    ConsoleOpened(WindowKey),
    WindowWasClosed(WindowKey),
    WindowFocused(WindowKey),
    WindowUnfocused(WindowKey),
    ModifierStateChanged(gpui_kit::Modifiers),
    TrayOpen,
    TrayQuit,
}

impl Ducktape {
    /// The state at launch, and the first thing to do: reach the node last
    /// used, if there was one.
    pub(crate) fn boot() -> (Self, view_wire::Task<AppMessage>) {
        let recent = backend::recent_endpoints();
        let endpoint = recent
            .first()
            .map(|entry| entry.url.clone())
            .unwrap_or_else(|| backend::DEFAULT_ENDPOINT.to_owned());
        let state = Ducktape {
            appearance: backend::load_appearance(),
            system_dark: false,
            screen: Screen::Connect,
            endpoint: endpoint.clone(),
            endpoint_error: String::new(),
            recent_endpoints: recent,
            connected_rpc: String::new(),
            network: String::new(),
            keyring: String::new(),
            other_chain: false,
            network_menu: false,
            connected: false,
            connecting: false,
            status: "Not connected".into(),
            height: -1,
            status_misses: 0,
            node: None,
            block_seen: 0,
            popover: None,
            spotlight: false,
            spotlight_query: String::new(),
            spotlight_pick: 0,
            settings_win: None,
            settings_page: SettingsPage::Appearance,
            motion: backend::load_motion(),
            error: String::new(),
            signer_key: String::new(),
            account: None,
            password: String::new(),
            confirm_password: String::new(),
            unlock_error: String::new(),
            unlock_busy: false,
            key_exists: false,
            browsing: false,
            restoring: false,
            restore_phrase: String::new(),
            restore_password: String::new(),
            restore_confirm_password: String::new(),
            account_name: String::new(),
            passkey_task: None,
            account_offer: false,
            account_step: false,
            passkey_phone: Default::default(),
            passkey_qr: String::new(),
            replacing: false,
            phrase: String::new(),
            phrase_quiz: None,
            quiz_answers: Default::default(),
            active: None,
            badges: BTreeMap::new(),
            toast: String::new(),
            toast_age: 0,
            console_win: None,
            focused_win: None,
            cmd_held: false,
            connect_generation: 0,
            connect_task: None,
            wall_now: 0,
        };
        let first = match std::env::var("DUCKTAPE_RPC")
            .ok()
            .or_else(|| (!state.recent_endpoints.is_empty()).then(|| endpoint.clone()))
        {
            Some(endpoint) => view_wire::Task::done(AppMessage::ConnectTo(endpoint)),
            None => view_wire::Task::none(),
        };
        (state, first)
    }

    /// Before the desk: reaching a node, this device's key and its
    /// phrase, or the account step. The console window is the launcher's
    /// size meanwhile.
    pub(crate) fn in_launcher(&self) -> bool {
        match self.screen {
            Screen::Connect => true,
            Screen::Console => {
                !self.phrase.is_empty()
                    || (self.signer_key.is_empty() && !self.browsing)
                    || (self.account_step && !self.signer_key.is_empty())
            }
        }
    }

    /// Connected, but the last [`LOST_AFTER`] status polls went unanswered.
    pub(crate) fn reconnecting(&self) -> bool {
        self.connected && self.status_misses >= LOST_AFTER
    }

    pub(crate) fn dark(&self) -> bool {
        match self.appearance {
            Appearance::Light => false,
            Appearance::Dark => true,
            Appearance::System => self.system_dark,
        }
    }

    /// What every view is handed as its props.
    pub(crate) fn view_props(&self) -> Vec<u8> {
        crate::runtime::props(
            self.dark(),
            self.connected,
            &self.network,
            &self.signer_key,
            // the node the views are on — not the address being typed or
            // tried (a switch in flight)
            &self.connected_rpc,
        )
    }
}

#[path = "update.rs"]
mod update;
