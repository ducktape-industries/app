//! The app's state, and what moves it. Small on purpose: a node to reach,
//! a key to unlock, the programs the node runs, and which one is open.
//! Everything a person does inside a view is the view's.

use std::collections::BTreeMap;

use crate::backend;
use crate::runtime::ModuleViewEvent;
use crate::shell::WindowKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
    System,
    Light,
    Dark,
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
    pub(crate) connected: bool,
    pub(crate) connecting: bool,
    pub(crate) status: String,
    pub(crate) height: i64,
    pub(crate) error: String,
    /// The seated key's public half, hex; empty while locked.
    pub(crate) signer_key: String,
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
    /// Set once the person picks "Use a phone instead"; the ceremony reads it.
    pub(crate) passkey_phone: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The QR URL of the touch in flight (its callback is the relay slot).
    pub(crate) passkey_qr: String,
    /// A freshly minted key's recovery phrase, shown once until written down.
    pub(crate) phrase: String,
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
    Disconnect,
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
            connected: false,
            connecting: false,
            status: "Not connected".into(),
            height: -1,
            error: String::new(),
            signer_key: String::new(),
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
            passkey_phone: Default::default(),
            passkey_qr: String::new(),
            phrase: String::new(),
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
            &self.endpoint,
        )
    }
}

#[path = "update.rs"]
mod update;
