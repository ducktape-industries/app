//! The app's state, and what moves it. Small on purpose: a node to reach,
//! a key to unlock, the programs the node runs, and which one is open.
//! Everything a person does inside a view is the view's.

use std::collections::BTreeMap;

use crate::backend;
use crate::runtime::Intent;
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

/// What is open over the desk: one at a time, and it keeps the desk's
/// keys while it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Overlay {
    /// ⌘K.
    Spotlight,
    /// "Add a device…".
    Approve,
    Settings,
    /// The network switcher.
    Network,
    /// A menu off the bar.
    Menu(Popover),
}

/// A menu hanging off the menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Popover {
    /// The breathing dot: how the node is doing.
    Node,
    /// The account's name: who is signed in, and Lock.
    Account,
    /// The bell: the notification centre.
    Notifications,
}

/// The Settings window's sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    Appearance,
    Notifications,
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

/// Which screen the console window shows: a launcher step, or the desk.
/// Stored, and moved only by messages: what a step alone holds (a typed
/// secret, a task in flight) lives in its variant and goes when it does.
#[derive(Debug)]
pub(crate) enum Stage {
    /// Reaching a node.
    Connect,
    /// This device's key: opening, locked, or behind an old password.
    Unlock(Unlock),
    /// A new recovery key's words, and their check.
    Phrase(Phrase),
    /// The account's recovery key, typed to add this device.
    Recover(Recover),
    /// Name an account for the seated key, or join one.
    Account(Account),
    /// The desk: signed in, or reading without a key.
    Desk,
}

#[cfg(test)]
impl Stage {
    /// The variant, and the sub-step it shows, for tests to compare:
    /// "Unlock/awaiting", "Phrase/quiz", "Account/link".
    pub(crate) fn step(&self) -> String {
        let (name, sub) = match self {
            Stage::Connect => ("Connect", None),
            Stage::Unlock(step) => ("Unlock", step.awaiting.then_some("awaiting")),
            Stage::Phrase(step) => ("Phrase", step.quiz.map(|_| "quiz")),
            Stage::Recover(_) => ("Recover", None),
            Stage::Account(step) => (
                "Account",
                match (step.passkey_task.is_some(), step.link_code.is_empty()) {
                    (true, _) => Some("passkey"),
                    (false, false) => Some("link"),
                    (false, true) => None,
                },
            ),
            Stage::Desk => ("Desk", None),
        };
        match sub {
            Some(sub) => format!("{name}/{sub}"),
            None => name.to_owned(),
        }
    }
}

/// A typed secret or a recovery phrase: wiped when it drops, so leaving
/// a step wipes what it held.
pub(crate) type Secret = zeroize::Zeroizing<String>;

/// The key step.
#[derive(Default)]
pub(crate) struct Unlock {
    /// A password-locked key's password (keys from before they moved into
    /// the OS; see [`backend::device_key`]).
    pub(crate) password: Secret,
    /// The key is seated and the node's first answer about its account is
    /// awaited: none goes on to the account step, one to the desk, and
    /// neither shows the other first (#292).
    pub(crate) awaiting: bool,
}

/// A new recovery key, from the account menu.
#[derive(Default)]
pub(crate) struct Phrase {
    /// Its 24 words, held until the person has typed back the words `quiz`
    /// asks for and the key is on the account.
    pub(crate) words: Secret,
    /// Once "I wrote it down" is pressed: the three word positions
    /// (0-based, ascending) the person types back.
    pub(crate) quiz: Option<[usize; 3]>,
    pub(crate) answers: [Secret; 3],
}

/// "Use a recovery key", off the account step.
#[derive(Default)]
pub(crate) struct Recover {
    /// Its 24 words being typed.
    pub(crate) phrase: Secret,
    /// The account step's name field, back as it was on "← Back".
    pub(crate) name: String,
}

/// The account step, and the ways it joins one.
#[derive(Default)]
pub(crate) struct Account {
    /// The name a new account takes.
    pub(crate) name: String,
    /// "From another device": the code this device shows while it waits
    /// for one on the account to approve it.
    pub(crate) link_code: String,
    pub(crate) link_task: Option<view_wire::task::Handle>,
    /// A passkey ceremony in flight (the browser has it); dropping the
    /// handle cancels it.
    pub(crate) passkey_task: Option<view_wire::task::Handle>,
    /// Set once the person picks "Use a phone instead"; the ceremony reads it.
    pub(crate) passkey_phone: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The QR URL of the touch in flight (its callback is the relay slot).
    pub(crate) passkey_qr: String,
}

impl std::fmt::Debug for Unlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Unlock {{ awaiting: {} }}", self.awaiting)
    }
}

impl std::fmt::Debug for Phrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Phrase {{ quiz: {:?} }}", self.quiz)
    }
}

impl std::fmt::Debug for Recover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Recover")
    }
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Account {{ linking: {}, passkey: {} }}",
            self.link_task.is_some(),
            self.passkey_task.is_some()
        )
    }
}

pub struct Ducktape {
    pub(crate) appearance: Appearance,
    pub(crate) system_dark: bool,
    /// The console window's screen.
    pub(crate) stage: Stage,
    /// The node URL being typed.
    pub(crate) endpoint: String,
    pub(crate) endpoint_error: String,
    pub(crate) recent_endpoints: Vec<backend::RecentEndpoint>,
    /// The node reached: its origin and the network it serves.
    pub(crate) connected_rpc: String,
    pub(crate) network: String,
    /// The chain links name, `<network>#<salt>` ([`ducklink::ChainId`]).
    pub(crate) chain: String,
    /// Each native window's panes: which view is where, their frames and
    /// focus. The console's is `console_win`'s.
    pub(crate) layouts: BTreeMap<WindowKey, super::layout::Layout>,
    /// Where this network's keys live on this device
    /// ([`backend::bind_keyring`]): the name alone is not enough, two
    /// chains can share one.
    pub(crate) keyring: String,
    /// The network shares its name with another chain this device met
    /// first: its keys are its own, and the sign-in screen says so.
    pub(crate) other_chain: bool,
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
    /// What is open over the desk.
    pub(crate) overlay: Option<Overlay>,
    /// What ⌘K holds: the typed text, the picked row.
    pub(crate) spotlight_query: String,
    pub(crate) spotlight_pick: usize,
    /// The Settings page shown, kept while Settings is closed.
    pub(crate) settings_page: SettingsPage,
    /// The drawings in characters turn; off keeps them on their first frame.
    pub(crate) motion: bool,
    pub(crate) error: String,
    /// The seated key's public half, hex; empty while locked.
    pub(crate) signer_key: String,
    /// The account the seated key belongs to, as `(number, name)`: `None`
    /// until the node was asked, `Some(None)` while the key holds none.
    pub(crate) account: Option<Option<(u64, String)>>,
    /// A password-locked key file is here for `network` and this device's
    /// OS-kept key is not: the key screen asks for its password, once, and
    /// moves it into the OS.
    pub(crate) key_exists: bool,
    /// What outlives one sign-in step: the key's own state, the last
    /// failure, and the device-approval dialog. Leaving a network drops it
    /// whole.
    pub(crate) sign_in: SignIn,
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

/// What the sign-in screens share: this device's key (opening, locked),
/// the step's failure and busy mark, and the "Add a device…" dialog.
#[derive(Default)]
pub(crate) struct SignIn {
    pub(crate) unlock_error: String,
    pub(crate) unlock_busy: bool,
    /// This device's key is being opened (or made) for the network reached.
    pub(crate) seating: bool,
    /// Locked on purpose: the key is not reopened until Unlock.
    pub(crate) locked: bool,
    /// "Add a device…": the code typed, and the request it found.
    pub(crate) approve_code: String,
    pub(crate) approve_found: Option<backend::join::Request>,
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
    /// A click on its backdrop, Escape, its close button: `Menu(_)` closes
    /// whichever menu is open.
    CloseOverlay(Overlay),
    TogglePopover(Popover),
    OpenSpotlight,
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
    ShowSettingsPage(SettingsPage),
    SetMotion(bool),
    /// Another node from the switcher: reached first, and only once it
    /// answers does the console leave the network in hand.
    SwitchNetwork(String),
    /// Show a program on the desk (Spotlight, a menu).
    SelectView(&'static str),
    /// Something done to a window's panes.
    Pane(WindowKey, super::layout::PaneMessage),
    /// A window drew its desk this size; `seed` is the program an
    /// untouched desk opens.
    DeskShown {
        window: WindowKey,
        desk: (f32, f32),
        seed: Option<&'static str>,
    },
    ViewEvent(&'static str, Intent),
    OpenLink(String),
    /// A notification centre row picked: read, and its link opened.
    NotifyOpen(u64),
    NotifyMarkAllRead,
    NotifyClearRead,
    /// The centre's footer: Settings, on Notifications.
    NotifySettings,
    /// A view's permission bar, or its row in Settings.
    NotifyPermission(&'static str, crate::runtime::notify::Permission),
    NotifyNotNow(&'static str),
    SetNotifyBanners(bool),
    SetNotifyInFront(bool),
    SetNotifyBurst(u32),
    PasswordTyped(String),
    /// Unlock: the OS-kept key again after a Lock, or a password-locked
    /// key with its password.
    UnlockSubmit,
    Unlocked(String),
    /// This device's key, opened or made on reaching a network: its public
    /// half; `None` when only a password-locked key is here.
    DeviceKey(Result<Option<String>, String>),
    /// "From another device": show a code and wait for an approval.
    LinkStart,
    LinkCancel,
    /// "Use a recovery key".
    RecoverShow,
    RecoverCancel,
    RecoverSubmit,
    /// A join (another device, a recovery key) landed or failed.
    Joined(Result<(), String>),
    /// The approving side, on a device already on the account.
    ApproveOpen,
    ApproveCodeTyped(String),
    ApproveFind,
    ApproveFound(Result<backend::join::Request, String>),
    ApproveConfirm,
    ApproveDone(Result<(), String>),
    /// A new recovery key: its words, their check, then onto the account.
    RecoveryKeyStart,
    RecoveryKeyAdded(Result<(), String>),
    PhraseCancel,
    PhraseWrittenDown,
    PhraseWordTyped(usize, String),
    PhraseCheckSubmit,
    PhraseShowAgain,
    UnlockFailed(String),
    BrowseWithoutKey,
    SignIn,
    RestorePhraseTyped(String),
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
            stage: Stage::Connect,
            endpoint: endpoint.clone(),
            endpoint_error: String::new(),
            recent_endpoints: recent,
            connected_rpc: String::new(),
            network: String::new(),
            chain: String::new(),
            layouts: BTreeMap::new(),
            keyring: String::new(),
            other_chain: false,
            connected: false,
            connecting: false,
            status: "Not connected".into(),
            height: -1,
            status_misses: 0,
            node: None,
            block_seen: 0,
            overlay: None,
            spotlight_query: String::new(),
            spotlight_pick: 0,
            settings_page: SettingsPage::Appearance,
            motion: backend::load_motion(),
            error: String::new(),
            signer_key: String::new(),
            account: None,
            key_exists: false,
            sign_in: SignIn::default(),
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

    /// Before the desk: the console window is the launcher's size.
    pub(crate) fn in_launcher(&self) -> bool {
        !matches!(self.stage, Stage::Desk)
    }

    /// The joining key's fingerprint, once its code was found.
    pub(crate) fn approve_fingerprint(&self) -> Option<String> {
        self.sign_in
            .approve_found
            .as_ref()
            .map(|request| backend::join::fingerprint(&request.key))
    }

    /// The passkey QR URL, while a ceremony runs and the person picked the
    /// phone.
    pub(crate) fn passkey_qr_shown(&self) -> Option<String> {
        let Stage::Account(step) = &self.stage else {
            return None;
        };
        (step.passkey_task.is_some()
            && step
                .passkey_phone
                .load(std::sync::atomic::Ordering::Relaxed)
            && !step.passkey_qr.is_empty())
        .then(|| step.passkey_qr.clone())
    }

    /// Seconds since the height last moved.
    pub(crate) fn block_age(&self) -> i64 {
        self.wall_now - self.block_seen
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
            &self.chain,
            &self.signer_key,
            // the node the views are on — not the address being typed or
            // tried (a switch in flight)
            &self.connected_rpc,
        )
    }
}
