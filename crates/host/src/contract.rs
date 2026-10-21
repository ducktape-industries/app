//! The one interface file: everything a shell gives the host core, and
//! everything a shell feeds it after start. An outside developer reads
//! this file and knows what a new OS must provide; nothing else in this
//! crate names an OS, a window manager or a key store.
//!
//! The seam has two directions. Shell to host: [`OsServices`] (installed
//! once, read anywhere) and [`Inbound`] (OS events after start). Host to
//! shell: what [`start`](crate::start::start) returns in
//! [`Host`](crate::start::Host): the entities to draw, `signals` (the
//! [`Signal`]s the host hands on: `Back`, `InsetsChanged`,
//! `MemoryWarning`), and `consent`, one wake each time the consent queue
//! moves (`runtime::consent`: the shell shows the front ask and answers
//! it); and [`Boot::open`], the shell's opener for what a banner click
//! names, which `start` installs. Both are in what a shell must pass or
//! take, so none can be forgotten.
//!
//! Two impls: the desktop's (`src/os.rs` in the app binary: directories
//! by the platform's rule, the OS keyring, freedesktop / macOS banners,
//! invites minted with the node this machine runs) and the phone's
//! (`crates/phone/src/os/`). A test build with none installed runs on
//! [`testing::TestOs`].
//!
//! The view runner ([`ViewRunner`], its own trait handed to [`Boot`]
//! beside `OsServices`) runs view code: wasmtime on the desktop and
//! Android, JavaScriptCore on iOS. Every call into a view is
//! completion-shaped: it returns at once, and its answer lands on the
//! seat's [`Landing`], before the call returns (wasmtime) or later (JSC).
//! A tick completes when its frame lands, never by a timer.

#![deny(missing_docs)]

use crate::backend::RpcClient;
use crate::motion;
use crate::runtime::Answered;
#[cfg(feature = "ui")]
use crate::runtime::notify::Opened;
use crate::runtime::notify::{Click, Notice};
#[cfg(feature = "ui")]
use gpui_kit::{AppLifecyclePhase, Window, WindowInsets};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// What a shell gives the host. One impl per shell.
pub trait OsServices: Send + Sync + 'static {
    /// Where files go.
    fn dirs(&self) -> &Dirs;
    /// This device's key for `keyring`, made on first ask; it signs and
    /// never hands the key out. Called off the window thread (a store may
    /// prompt). `Ok(None)`: no OS-kept key, but a key from before keys
    /// moved into the OS is here under a password; the shell asks for it
    /// and seats it itself (the desktop's `shell::sign_in`). A phone never
    /// answers `None`.
    fn device_key(
        &self,
        keyring: &str,
    ) -> Result<Option<Arc<dyn noded_client::Signer>>, KeyRefusal>;
    /// Where that key is kept, in the store's own words (the Settings row
    /// "Key storage").
    fn key_storage(&self, keyring: &str) -> Result<KeyStorage, KeyRefusal>;
    /// The OS's store of views' secrets (`store.secret_get`,
    /// `store.secret_set`); `None` where this device has none, which a view
    /// hears as `no_secret_store`.
    fn secrets(&self) -> Option<&dyn Secrets>;
    /// Which key that is, and whether it is linkable to the device, is the
    /// shell's call: the desktop answers a throwaway key made per process.
    /// Not in SPEC §4.2's list: §4.8 leaves the throwaway in the desktop
    /// impl, and the host's query path needs a way to reach it.
    fn reader(&self) -> Arc<dyn noded_client::Signer>;
    /// OS banners: post and withdraw. A click comes back as
    /// [`Signal::BannerClick`].
    fn banners(&self) -> &dyn Banners;
    /// Safe area and keyboard for `window`, as gpui's `WindowInsets`. Zero
    /// on the desktop. Exists because gpui-mobile's `AndroidWindow`
    /// implements none of `PlatformWindow::{insets, show_soft_keyboard,
    /// hide_soft_keyboard}`; when it does, this and `soft_keyboard` go.
    /// A host with no screen (no `ui`) has neither; one built beside a
    /// shell that has them (one workspace build) takes the desktop's.
    #[cfg(feature = "ui")]
    fn insets(&self, _window: &Window) -> WindowInsets {
        WindowInsets::default()
    }
    /// Show the software keyboard for the focused field, or hide it.
    /// Desktop: nothing.
    #[cfg(feature = "ui")]
    fn soft_keyboard(&self, _window: &Window, _want: SoftKeyboard) {}
    /// Facts that are constants per OS, read once.
    fn facts(&self) -> OsFacts;
    /// Values that differ by device and have a measured owner (SPEC §4.5).
    fn limits(&self) -> Limits;
    /// The view section the app ships for the roster's blob `blob` (the
    /// phone's `views.lock`, SPEC §4.7), read on a loader thread in place
    /// of a fetch. `None` on the desktop, which ships no view. The roster
    /// decides what runs: a miss is a fetch, never a refusal.
    fn bundled_view(&self, blob: &::abi::BlobId) -> Option<Vec<u8>>;
    /// `invite.create`: mint with the node key of a node THIS device runs
    /// (the desktop reads it out of the node's workspace). `None` on a host
    /// that runs no node: the kernel refuses `invite_unsupported` with
    /// "Only the machine running this node can mint its invites."
    /// Called on the kernel's thread (`views-kernel`, one tokio
    /// current-thread runtime) inside the request's task, and the answer
    /// is awaited there: neither may block it (blocking work goes to
    /// `tokio::task::spawn_blocking`).
    fn mint_invite(&self, ask: MintInvite) -> Option<Answered>;
    /// The shell's own file dialogs, where gpui's platform has none (the
    /// phones: gpui_ios answers its picker and save dialog "not yet
    /// implemented", gpui-mobile's Android save "cancelled"). `None`, the
    /// default: `file.pick` and `file.save` open gpui's (the desktop).
    fn files(&self) -> Option<&dyn Files> {
        None
    }
    /// The process's memory as the OS counts it, and with `regions` its
    /// breakdown by region; `None` where the shell reads none.
    fn memory(&self, _regions: bool) -> Option<crate::memory::OsMemory> {
        None
    }
}

/// What a pick answers ([`Files::pick`]): the paths of the shell's copies
/// of the files picked, `None` when the person cancelled, or what went
/// wrong. Called once, from any thread.
pub type PickDone = Box<dyn FnOnce(Result<Option<Vec<PathBuf>>, String>) + Send>;

/// What a keep answers ([`Files::keep`]): whether the person kept the
/// file, `false` when they cancelled, or what went wrong. Called once,
/// from any thread.
pub type KeepDone = Box<dyn FnOnce(Result<bool, String>) + Send>;

/// What a keep hands back ([`Files::keep`]): called once the host no longer wants the file shown;
/// a keep the OS shows nothing of yet then never shows, and answers `false`.
pub type Unkeep = Box<dyn FnOnce() + Send>;

/// A shell's own file dialogs ([`OsServices::files`]). Each is called on
/// the window thread or the kernel's and only hands on: the dialog opens on
/// the OS's own thread, and `done` answers when the person is through.
pub trait Files: Send + Sync {
    /// The system's picker of files (on a phone, its photos too). The shell
    /// copies each file picked to a path of its own, in a folder of its own
    /// under its cache; the host opens each and then removes it and its
    /// folder (`runtime::picker`), so a copy outlives no pick.
    fn pick(&self, multiple: bool, done: PickDone);
    /// `path`, a file the host has just written in a folder of its own
    /// under `Dirs::cache`, handed to the OS for the person to keep where
    /// they choose (iOS: a picture in Quick Look, any other file in the
    /// share sheet; Android: a picture in the system's viewer, any other
    /// file through the create-document dialog). The host
    /// removes the folder once `done` answers.
    fn keep(&self, path: &std::path::Path, done: KeepDone) -> Unkeep;
}

/// Where the host keeps files. `cache` the OS may purge; `config` and
/// `data` it keeps; `remotes` holds the keyring directories
/// (`noded_client::keyring::bind(remotes, network, founded)`); `runtime`
/// is a directory that EXISTS and that only this user or app can enter:
/// every `OsServices` impl makes it so before `install` (the desktop:
/// `$XDG_RUNTIME_DIR/ducktape`, else one it creates mode 0700), because
/// the AX door file's token, and anything else written there, is written
/// plainly and relies on it. The host never creates that directory, so an
/// impl that forgets gets an error, not a world-readable default.
/// `views_override` is a VIEW DEVELOPER'S directory of
/// `<program>_view.wasm` files that stand in for the network's (the
/// desktop reads `DUCKTAPE_VIEWS_DIR`; a phone has none).
#[derive(Clone, Debug)]
pub struct Dirs {
    /// What the OS may purge: fetched blobs, compiled view code.
    pub cache: PathBuf,
    /// What the person set: `prefs.json`.
    pub config: PathBuf,
    /// What the app keeps: the notification centre's log, view stores.
    pub data: PathBuf,
    /// The keyring directories, one per network name. The ONE source: the
    /// desktop's key files resolve under it too.
    pub remotes: PathBuf,
    /// The private directory the AX door's file goes in (see above).
    pub runtime: PathBuf,
    /// A view developer's directory of view files, if any.
    pub views_override: Option<PathBuf>,
}

/// Where a device key is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStorage {
    /// The seed never leaves hardware (Secure Enclave, StrongBox).
    pub sealed: bool,
    /// Which store holds it.
    pub level: KeyLevel,
}

/// The store a device key is kept in (the mobile `DeviceKeyStore`'s
/// `storage` levels, `Keychain` shared with the Mac, plus the desktop's
/// two).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyLevel {
    /// iOS: a seed sealed by the Secure Enclave.
    SecureEnclave,
    /// Apple: a this-device-only item in the data protection Keychain (on
    /// macOS read only after Touch ID or the login password).
    Keychain,
    /// Android Keystore backed by StrongBox.
    KeystoreStrongBox,
    /// Android Keystore backed by the TEE.
    KeystoreTee,
    /// Android Keystore in software.
    KeystoreSoftware,
    /// The desktop OS keyring (Secret Service, Credential Manager; on
    /// macOS the file keychain, for a build without the keychain
    /// entitlement).
    OsKeyring,
    /// A file, where no keyring is reachable.
    File,
}

/// Why a device key was not opened. Each carries the sentence the key
/// screen shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyRefusal {
    /// No store to keep one in (no keyring daemon on a headless Linux).
    NoStore(String),
    /// The store refused: locked, a prompt dismissed, two entries.
    Denied(String),
    /// The store answered with something that is not a key.
    Broken(String),
}

impl KeyRefusal {
    /// The sentence the key screen shows.
    pub fn sentence(&self) -> String {
        match self {
            Self::NoStore(s) | Self::Denied(s) | Self::Broken(s) => s.clone(),
        }
    }
}

/// The OS's own store of secrets, which keeps a view's (`runtime::secret`):
/// each call may block on the OS (a prompt, a daemon) and is made off the
/// window thread, one after another per name.
pub trait Secrets: Send + Sync {
    /// The secret kept under `name`, or `None` where none is.
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, KeyRefusal>;
    /// Keeps `value` under `name`, or removes what is kept there with `None`.
    fn set(&self, name: &str, value: Option<&[u8]>) -> Result<(), KeyRefusal>;
}

/// OS banners. Both calls block on the OS: a view's run through
/// `notify::in_order` on the blocking pool, the host's own on a thread of
/// their own.
pub trait Banners: Send + Sync {
    /// Raises `notice`; a click on it comes back as `click` through
    /// [`Signal::BannerClick`]. Whether it went up. A banner stands under
    /// its chain and tag ([`Notice::chain`]): two listed networks'
    /// programs may raise one tag.
    fn post(&self, notice: &Notice, click: Option<Click>) -> bool;
    /// Takes down the banner standing under `tag` on `chain`.
    fn withdraw(&self, chain: &str, tag: &str);
}

/// The software keyboard a focused field wants.
#[cfg(feature = "ui")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoftKeyboard {
    /// Up, for a field configured so.
    Show(Keyboard),
    /// Down.
    Hide,
}

/// The keyboard a field asks for: the keys it opens with, and whether what
/// is typed is a secret, which a phone keeps from suggestions and other
/// keyboards. The shell tells its OS both (iOS: `ducktape_gpui_ios::ios::set_keyboard`).
#[cfg(feature = "ui")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keyboard {
    /// The keys: text, digits, an address.
    pub kind: view_wire::VirtualKeyboard,
    /// The field is masked.
    pub secure: bool,
}

/// Constants per OS.
#[derive(Clone, Copy, Debug)]
pub struct OsFacts {
    /// Whether a view's key claims read the Command key as the primary
    /// modifier (macOS) or Control (everywhere else).
    pub mac_modifiers: bool,
    /// What `getrusage`'s `ru_maxrss` counts in: bytes on macOS, kibibytes
    /// elsewhere.
    pub rss_unit: u64,
    /// Touch is the primary pointer: targets, long presses, no hover.
    pub touch_first: bool,
    /// What this device's key is labelled on the account it joins
    /// (`identity::DeviceKey.label`): "Desktop", "iPhone", ...
    pub device_label: &'static str,
    /// What the person calls this device in a sentence ("this phone's
    /// network"): "phone", "computer".
    pub device_noun: &'static str,
    /// The device has one layout: the desk comes without the layout
    /// question (`Account::show`). A phone.
    pub one_layout: bool,
    /// A view's pane can leave for a window of its own (`host.float`):
    /// the desktop's. A phone shows one full-screen window, and its views
    /// are refused `host.float`, `host.unfloat` and `host.placement` as
    /// unknown requests.
    pub floats: bool,
    /// How this OS moves a stack's screens and a sheet: [`motion::IOS`],
    /// [`motion::ANDROID`]. A pointer window never moves, so the desktop's
    /// is never read.
    pub motion: motion::Idiom,
    /// The OS asks the person once whether the app may show notices at all
    /// (a phone's own prompt, and its Settings switch after), and no
    /// permission bar asks per program: a program they said nothing of
    /// here shows its banners, as the push server shows them for the same
    /// person while the app is closed (`notify::Settings::unasked`). The
    /// desktop asks per program.
    pub os_asks_notices: bool,
    /// A view's node feeds (`program.changes`, `chain.heads`) run only while
    /// it is on screen: a phone's, whose tabs keep panes seated that no one
    /// sees. A desktop pane keeps its feeds while it is presented, focused
    /// or not. A notifier, on no screen, follows either way.
    pub feeds_follow_screen: bool,
}

/// Values that differ by device and have a measured owner (SPEC §4.5).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Concurrent view loads. Desktop: `available_parallelism`. Phone: 1,
    /// because one Cranelift compile peaks at 200-315 MB and the phone's
    /// memory killer is the owner.
    pub loaders: usize,
    /// What a roster read does for a program nobody opened (SPEC §4.7).
    /// Read by `View::load`.
    pub preload: Preload,
    /// How often a seated view's state is kept for a death replay (SPEC
    /// §6.5): a snapshot after a settled tick once the last kept one is
    /// older than this, and every tick's events since, so an instance that
    /// dies is restored and replayed to where it was. `None` where an
    /// instance cannot die without the app (the desktop: wasmtime runs in
    /// this process); the phone's JSC page: 1 s.
    pub snapshot_every: Option<std::time::Duration>,
    /// Whether settled snapshots are written under `Dirs::data` so a cold
    /// start reopens where the person was. Phone true; desktop false.
    /// Not read yet: the phone's persistence (SPEC §7.5) reads it.
    pub snapshot_to_disk: bool,
}

/// What a roster read does for a program nobody opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preload {
    /// Compile every roster program, instantiate none (the desktop).
    Compile,
    /// Seat every roster program at connect, `in_front = false`.
    Seat,
    /// No screen, and no one at the device (`crates/pushd`): no view is
    /// seated or compiled, and no notifier runs for the device; only those
    /// of the recipients the host registers do (`runtime::notify_for`).
    Headless,
}

/// `invite.create`'s ask: the node it is about (the mint reads its status
/// and validator set, so the ask carries the node's client), how long the
/// invite lives (`methods::CreateInvite.ttl_days`, a `u64` on the wire),
/// and the clock.
#[derive(Clone)]
pub struct MintInvite {
    /// The node the invite is for.
    pub client: RpcClient,
    /// How many days the invite lives.
    pub ttl_days: u64,
    /// Now, in milliseconds since the epoch.
    pub now_ms: u64,
}

/// Everything the shell feeds the host after [`start`](crate::start::start):
/// one sender, any thread; one foreground task drains it on the GPUI
/// thread into the entities. An OS event from another thread reaches host
/// state through it (a shell's own GPUI-thread channels, such as the
/// desktop tray's appearance pick, call the entities directly).
#[cfg(feature = "ui")]
#[derive(Clone)]
pub struct Inbound(async_channel::Sender<Signal>);

#[cfg(feature = "ui")]
impl Inbound {
    pub(crate) fn channel() -> (Self, async_channel::Receiver<Signal>) {
        let (sender, receiver) = async_channel::unbounded();
        (Self(sender), receiver)
    }

    /// Never blocks; a signal after the host is gone is dropped.
    pub fn send(&self, signal: Signal) {
        let _ = self.0.try_send(signal);
    }
}

/// One OS event for the host.
#[cfg(feature = "ui")]
#[derive(Clone, Debug)]
pub enum Signal {
    /// gpui's phase. `Background` stops the status poll (and, with the
    /// runner cut, every tick); `Foreground` resumes them.
    Lifecycle(AppLifecyclePhase),
    /// iOS `didReceiveMemoryWarning` / Android `onTrimMemory`.
    MemoryWarning,
    /// Android BACK from the Activity's dispatcher.
    Back,
    /// A sideways swipe under way: iOS's swipe back (from the left edge,
    /// or from anywhere the shell lets one begin), the screen on top
    /// following it; or, on either OS, the tabs' at a tab root.
    Swipe {
        /// How far its finger is, in points to the right (negative to the
        /// left), from where it started.
        x: f32,
        /// How fast it moves, in points per second to the right.
        speed: f32,
    },
    /// That swipe let go where and as fast as its last `Swipe` says: the
    /// shell judges whether it goes back, or which tab it lands on.
    SwipeLetGo,
    /// That swipe ended without going on: the OS cancelled its touch, or
    /// a controller the shell does not draw (iOS: a link's Safari view)
    /// was presented over the screen.
    SwipeCancelled,
    /// The insets changed: re-read `OsServices::insets`, re-place.
    InsetsChanged,
    /// A banner the host raised was clicked: the centre opens what it
    /// names, through [`Boot::open`].
    BannerClick(Click, Option<String>),
}

/// What a shell hands [`start`](crate::start::start).
#[cfg(feature = "ui")]
pub struct Boot {
    /// The shell's services; `start` installs them.
    pub os: Arc<dyn OsServices>,
    /// What runs view code; `start` installs it.
    pub runner: Arc<dyn ViewRunner>,
    /// What opens what a banner click names (a `duck://` link, Settings ›
    /// Updates) in the shell's windows. Called on the GPUI thread.
    pub open: fn(Opened, Option<String>),
}

static OS: OnceLock<Arc<dyn OsServices>> = OnceLock::new();
static RUNNER: OnceLock<Arc<dyn ViewRunner>> = OnceLock::new();

/// The shell's view runner, installed once, before the first load (`start`
/// installs its `Boot`'s). A second, different instance is a bug.
pub fn install_runner(runner: Arc<dyn ViewRunner>) {
    let installed = RUNNER.get_or_init(|| runner.clone());
    assert!(
        Arc::ptr_eq(installed, &runner),
        "the shell's ViewRunner is installed once"
    );
}

/// The installed view runner. A test build with none installed runs on
/// wasmtime, as the desktop does.
pub fn runner() -> &'static Arc<dyn ViewRunner> {
    #[cfg(any(test, feature = "test-support"))]
    {
        RUNNER.get_or_init(|| {
            Arc::new(crate::runner::WasmtimeRunner::new(RunnerConfig {
                signals_based_traps: true,
                cache_dir: Some(os().dirs().cache.join("view-code")),
                compile_apart: None,
            }))
        })
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        RUNNER
            .get()
            .expect("the shell installs its ViewRunner before the first load")
    }
}

/// Runs view code. One per process; the shell picks it ([`Boot::runner`]).
///
/// Threads: `prepare` runs on a loader thread and may block it (wasmtime
/// compiles; JSC runs the meter pass; neither touches a page). `describe`
/// may be called from the kernel's blocking pool and may block it, NEVER
/// from the window thread (a JSC impl posts to main and blocks the pool
/// thread on a oneshot; main never waits on the pool). `instantiate` and
/// every `begin_*` are called only on the seat's thread (the window
/// thread), which awaits each answer as it lands on the seat's
/// [`Landing`], at once or later: a first load, a pane claiming a compiled
/// seat, the developer's override and `Preload::Seat`'s instance (a loader
/// prepares the code and parks it; the window thread seats and inits it),
/// a swap's replacement, a sibling after a restore trap, and a revive.
pub trait ViewRunner: Send + Sync + 'static {
    /// Compile (wasmtime) or validate + meter + cache (JSC) `code`, whose
    /// sha256 is `hash`, for a view whose linear memory may reach
    /// `view_bytes` (the `memory.view_bytes` setting). Nothing runs. The
    /// code stays prepared while the answer's hold is held.
    fn prepare(
        &self,
        hash: [u8; 32],
        code: Code<'_>,
        view_bytes: usize,
    ) -> Result<Prepared, Failure>;
    /// Seat the prepared code `hash` names: lands [`Landed::Seated`] (or a
    /// `Fault`) on `land`, which the instance keeps for every later answer.
    /// Code nothing holds any more lands a `Fault::Runner`. Right after a
    /// page died it seats the view on a new page (see [`ViewInstance`]'s
    /// dead-page rules).
    fn instantiate(&self, hash: [u8; 32], seat: SeatId, land: Landing);
    /// `program.describe`: run the import-free describe module `module`
    /// (sha256 `hash`) on `op`, once `view_meter::accepts` takes it as it
    /// would a view; `None` on any failure.
    fn describe(
        &self,
        hash: [u8; 32],
        module: Code<'_>,
        op: &[u8],
    ) -> Option<view_wire::methods::Description>;
    /// The compiled code this runner holds: by hash, the bytes mapped and
    /// how many of them are resident.
    fn code_memory(&self) -> Vec<crate::memory::HeldCode> {
        Vec::new()
    }
}

/// Code a runner prepares (a view's, a notifier's, a describe module's):
/// its bytes, and where they lie in a file when they are a file's, as a
/// section of a program's blob the cache keeps is. A compile apart
/// ([`RunnerConfig::compile_apart`]) reads them there itself, so they are
/// never copied through this process's heap on their way to it.
#[derive(Clone, Copy, Debug)]
pub struct Code<'a> {
    /// The code.
    pub bytes: &'a [u8],
    /// The file `bytes` are, from this offset on; `None` for code held in
    /// memory only.
    pub file: Option<(&'a Path, u64)>,
}

impl<'a> Code<'a> {
    /// Code held in memory only.
    pub fn of(bytes: &'a [u8]) -> Self {
        Self { bytes, file: None }
    }
}

/// Where prepared code came from, for the load's log and counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeSource {
    /// Prepared before, and still held: nothing compiled.
    Memory,
    /// The runner's disk cache.
    Disk,
    /// Compiled (wasmtime) or validated and metered (JSC) now.
    Cold,
}

impl CodeSource {
    /// How the load's log (`code=`) and its counter (`compile.*`) name it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Disk => "disk",
            Self::Cold => "cold",
        }
    }
}

/// [`ViewRunner::prepare`]'s answer: where the code came from, and the
/// runner's hold on it. Whatever may still seat the code holds the hold (a
/// seat's compiled view, a parked swap, a load about to seat it); a running
/// instance holds its own code. A runner may free code nothing holds.
#[derive(Clone)]
pub struct Prepared {
    /// Where the code came from.
    pub source: CodeSource,
    /// The runner's hold on the code: opaque to the host.
    pub hold: Arc<dyn std::any::Any + Send + Sync>,
}

/// One running view: the five exports (`alloc` and `memory` internal to the
/// impl). `Send` so the seat registry may hold it; every entry is called
/// on the seat's thread and is completion-shaped: it returns at once and
/// the answer lands on the seat's [`Landing`].
///
/// A program's NOTIFIER (`runtime::notifier`) is seated the same way, on
/// the same code path and budget: a module that exports `alloc`,
/// `init` and `tick` only (no state to hand over). Every runner seats a
/// module without `snapshot` and `restore`, and a `begin_snapshot` or
/// `begin_restore` on it lands `Fault::Trap("the view exports no …")`. Its
/// seat's thread is the notifier's own thread, never the window thread;
/// its tick's answer is a [`Landed::Frame`] whose bytes the host reads as
/// a notifier's turn. Nothing else about a runner tells the two apart.
///
/// One call in flight per instance: the host begins a call only once the
/// answer to the one before it landed (a tick in flight is `View::ticking`;
/// a swap's or a kept snapshot's sequence owns the instance meanwhile,
/// `Mounted::owned`), so each answer is the answer to the last call begun.
/// Each call lands exactly one [`Landed`], once.
///
/// Dead pages (SPEC §6.5; the host relies on every rule here):
/// - Every call into a dead page lands `Fault::Runner`, never nothing: the
///   call in flight when the page died, and every call begun on the
///   instance after, until it is dropped. A call that lands nothing leaves
///   its seat waiting for good.
/// - [`ViewRunner::instantiate`] right after a death seats the view on a
///   new page: the host's revive asks at once, and a refusal there (a page
///   still reloading) stops the view. A runner that needs time takes it
///   before it lands `Seated`.
/// - The host revives each seat itself (`View::revive`: a new instance,
///   restored from the kept snapshot, the kept batches replayed): a runner
///   never restores or replays a seat on its own. This replaces SPEC §6.5's
///   runner-side restore.
/// - A dead instance keeps holding its code until it is dropped: a revive
///   seats the same code by hash while the dead instance is still the
///   view's.
///
/// Ending: the host calls [`Self::stop`] on an instance it replaces while
/// it lives (a revive's dead one). Every other instance is dropped, and
/// dropping one stops it, as `stop` does. Drop runs on whichever thread
/// drops it and must not wait on the seat's thread, which may be waiting
/// on a lock the dropping thread holds: the seat's thread (a swap's
/// install), a loader's (an install replacing the view, a `Loaded::Empty`
/// install, both under the seat's lock), or the roster thread's (a
/// program's `retire`, under the registry's and the seat's locks).
/// dispatch2's `MainThreadBound` drops its value through `run_on_main`,
/// which off the main thread is `exec_sync` on the main queue and waits on
/// it: a page handle is posted to the main thread (`exec_async`) to be
/// dropped there, not dropped through it.
pub trait ViewInstance: Send {
    /// The sha256 of the code this is an instance of.
    fn code_hash(&self) -> [u8; 32];
    /// The `init` export: lands `Inited` or a `Fault`.
    fn begin_init(&mut self);
    /// The `tick` export on `events` (an encoded event batch): lands
    /// `Frame` or a `Fault`.
    fn begin_tick(&mut self, events: Vec<u8>);
    /// The `snapshot` export: lands `Snapshot`, `SnapshotRefused` or a
    /// `Fault`.
    fn begin_snapshot(&mut self);
    /// The `restore` export: lands `Restored`, `RestoreRefused` or a
    /// `Fault`.
    fn begin_restore(&mut self, snapshot: Vec<u8>);
    /// The fuel the last landed call used.
    fn fuel_used(&self) -> u64;
    /// The view's linear memory, in bytes.
    fn memory_bytes(&self) -> usize;
    /// How much of the view's linear memory is resident; `None` where the
    /// runner cannot say.
    fn memory_resident(&self) -> Option<u64> {
        None
    }
    /// Ends the instance. Dropping it ends it too.
    fn stop(self: Box<Self>);
    /// The instance's linear memory, for the host's own tests that read
    /// it; only the wasmtime instance lends one.
    #[cfg(test)]
    fn memory_for_test(&self) -> &[u8] {
        unimplemented!("only the wasmtime instance lends its memory to a test")
    }
    /// [`Self::memory_for_test`], for the tests that script it.
    #[cfg(test)]
    fn memory_mut_for_test(&mut self) -> &mut [u8] {
        unimplemented!("only the wasmtime instance lends its memory to a test")
    }
}

/// Which seat an instance is for: (program, the pane's layout instance; 0
/// for a seat no pane held when it was seated).
/// During a swap or a revive two live instances share one id: it names a
/// program's seat for a runner's own logs and bookkeeping, and is no key
/// two instances may not share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeatId(pub &'static str, pub u64);

/// A seat's answer channel: `Send + Clone`, made by the seat when it asks
/// for an instance and kept by the instance; a runner `send`s each answer
/// into it from wherever the answer appears.
#[derive(Clone, Debug)]
pub struct Landing(async_channel::Sender<Landed>);

impl Landing {
    /// A landing and the seat's end of it.
    pub fn channel() -> (Self, async_channel::Receiver<Landed>) {
        let (sender, receiver) = async_channel::unbounded();
        (Self(sender), receiver)
    }

    /// Lands `landed`; one after the seat is gone is dropped.
    pub fn send(&self, landed: Landed) {
        let _ = self.0.try_send(landed);
    }
}

/// One answer from a view.
pub enum Landed {
    /// `instantiate`'s: the running instance.
    Seated(Box<dyn ViewInstance>),
    /// `init` ran.
    Inited,
    /// A tick's answer, its bytes as the module wrote them: a view's
    /// frame, or a notifier's turn (`view_wire::notifier::Turn`); the host
    /// reads which, never the runner.
    Frame(Vec<u8>),
    /// The view's state.
    Snapshot(Vec<u8>),
    /// The view's own word that it cannot hand its state over now.
    SnapshotRefused(String),
    /// The state was taken up.
    Restored,
    /// The view's own word that the state is not its own.
    RestoreRefused(String),
    /// The call failed; the instance is not entered again.
    Fault(Fault),
}

impl std::fmt::Debug for Landed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Seated(instance) => write!(f, "Seated({:02x?})", &instance.code_hash()[..4]),
            Self::Inited => f.write_str("Inited"),
            Self::Frame(bytes) => write!(f, "Frame({} bytes)", bytes.len()),
            Self::Snapshot(bytes) => write!(f, "Snapshot({} bytes)", bytes.len()),
            Self::SnapshotRefused(why) => write!(f, "SnapshotRefused({why})"),
            Self::Restored => f.write_str("Restored"),
            Self::RestoreRefused(why) => write!(f, "RestoreRefused({why})"),
            Self::Fault(fault) => write!(f, "Fault({fault})"),
        }
    }
}

/// Why a call into a view failed. Every runner produces every variant but
/// `Runner`, which is a runner's own failure (a refused script, a dead
/// page before the answer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The view trapped: the trap's words.
    Trap(String),
    /// The view panicked: the words its panic hook
    /// (`ducktape_view.panicked`) handed over before the trap.
    Panicked(String),
    /// A `memory.grow` or `table.grow` past the `memory.view_bytes`
    /// setting.
    Memory,
    /// The runner failed, not the view.
    Runner(String),
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trap(words) | Self::Panicked(words) | Self::Runner(words) => f.write_str(words),
            Self::Memory => f.write_str("the view is past the device's view memory setting"),
        }
    }
}

/// `prepare`'s refusals: `Unsupported` names the operator or feature this
/// runner does not run (the same answer on every runner); `Runner` the
/// runner's own. A view built against another wire is not refused: it runs,
/// and what fails names both wires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Code this runner does not run.
    Unsupported(String),
    /// The runner's own failure.
    Runner(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(words) | Self::Runner(words) => f.write_str(words),
        }
    }
}

/// How a shell configures its runner.
#[derive(Clone, Debug)]
pub struct RunnerConfig {
    /// wasmtime's signal-based traps: the desktop's default; off on Android
    /// (an arm64 guest trap otherwise kills the process).
    pub signals_based_traps: bool,
    /// Where compiled code is kept across launches; `None` keeps none.
    pub cache_dir: Option<PathBuf>,
    /// This binary, run as a child ([`crate::runner::COMPILE_APART`]) for
    /// each compile of code that lies in a file ([`Code::file`]): it reads
    /// the code there and lands the compile in `cache_dir` for this process
    /// to read. A compile's peak (200-315 MB) then leaves with the child,
    /// where the allocator would keep it here on loader threads that go
    /// idle. Needs `cache_dir`. `None` compiles in this process (the
    /// phone), as does code held in memory only.
    pub compile_apart: Option<PathBuf>,
}

/// The shell's services, installed before the host reads anything
/// (`start` installs its `Boot`'s; a shell that needs them earlier, or
/// runs no host, installs the same instance itself). A second, different
/// instance is a bug.
pub fn install(os: Arc<dyn OsServices>) {
    let installed = OS.get_or_init(|| os.clone());
    assert!(
        Arc::ptr_eq(installed, &os),
        "the shell's OsServices are installed once"
    );
}

/// The installed services. A test build with none installed runs on
/// [`testing::TestOs`]: files under a directory of the test thread's own,
/// no banners, no node. A binary never reaches that fallback: every shell
/// installs its services before its first read (`main.rs`).
pub fn os() -> &'static Arc<dyn OsServices> {
    #[cfg(any(test, feature = "test-support"))]
    {
        OS.get_or_init(|| Arc::new(testing::TestOs::new()))
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        OS.get()
            .expect("the shell installs its OsServices before the host runs")
    }
}

/// The services a test runs on when its harness installs none.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ACTIVE: AtomicBool = AtomicBool::new(false);

    /// Whether the test services are the installed ones: what the host's
    /// per-test-thread state (the consent queue) keys on. Never true in a
    /// shell's binary, which installs its own.
    pub fn active() -> bool {
        ACTIVE.load(Ordering::Relaxed)
    }

    /// Files under one directory per test thread (the tests share the
    /// process; one thread's prefs are not another's), no banners, a reader
    /// key of its own, a device key per keyring made in memory, no node to
    /// mint invites with. The directories of test processes that are gone
    /// are removed as a new one starts.
    pub struct TestOs {
        reader: Arc<dyn noded_client::Signer>,
    }

    fn root_of(pid: u32) -> PathBuf {
        std::env::temp_dir().join(format!("ducktape-test-{pid}"))
    }

    /// The roots earlier test processes left: each is named by its pid,
    /// and a pid no process holds is a run that ended.
    fn remove_stale_roots() {
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name
                .to_str()
                .and_then(|name| name.strip_prefix("ducktape-test-"))
                .and_then(|pid| pid.parse::<u32>().ok())
            else {
                continue;
            };
            // SAFETY: `kill` with signal 0 sends nothing; it only answers
            // whether `pid` names a process.
            let alive = pid == std::process::id() || unsafe { libc::kill(pid as i32, 0) } == 0;
            if !alive {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    impl TestOs {
        /// The test services, made active (see [`active`]).
        pub fn new() -> Self {
            remove_stale_roots();
            ACTIVE.store(true, Ordering::Relaxed);
            Self {
                reader: Arc::new(FakeKey([7; 32])),
            }
        }
    }

    impl Default for TestOs {
        fn default() -> Self {
            Self::new()
        }
    }

    /// A key whose public half is its seed and whose signature is fixed
    /// bytes: the tests read a frame's shape, not its proof.
    pub struct FakeKey(pub [u8; 32]);

    impl noded_client::Signer for FakeKey {
        fn public_key(&self) -> [u8; 32] {
            self.0
        }

        fn sign(&self, _: &[u8], _: &[u8]) -> Result<[u8; 64], noded_client::SignError> {
            Ok([8; 64])
        }
    }

    /// Banners that go nowhere: each one asked for is kept, for a test to
    /// read ([`banners_asked`]), and none is raised.
    pub struct NoBanners;

    static ASKED: std::sync::Mutex<Vec<Notice>> = std::sync::Mutex::new(Vec::new());

    #[cfg(test)]
    static POSTED: std::sync::Condvar = std::sync::Condvar::new();

    #[cfg(test)]
    pub(crate) fn wait_for_banners(done: impl Fn(&[Notice]) -> bool) {
        let asked = ASKED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        drop(
            POSTED
                .wait_while(asked, |asked| !done(asked))
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
    }

    /// Every banner the host asked for so far, by every test.
    pub fn banners_asked() -> Vec<Notice> {
        ASKED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    static WITHDRAWN: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

    /// Every banner the host took down so far, by every test: its chain
    /// and tag.
    pub fn banners_withdrawn() -> Vec<(String, String)> {
        WITHDRAWN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    impl Banners for NoBanners {
        fn post(&self, notice: &Notice, _: Option<Click>) -> bool {
            ASKED
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(notice.clone());
            #[cfg(test)]
            POSTED.notify_all();
            false
        }

        fn withdraw(&self, chain: &str, tag: &str) {
            WITHDRAWN
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((chain.to_owned(), tag.to_owned()));
        }
    }

    static SECRETS: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());

    static SECRET_REFUSALS: std::sync::Mutex<Vec<(String, KeyRefusal)>> =
        std::sync::Mutex::new(Vec::new());

    /// Every secret the test key store keeps now, by name, from every test.
    pub fn secrets_kept() -> std::collections::BTreeMap<String, Vec<u8>> {
        SECRETS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// From now on the test key store refuses every secret whose name holds
    /// `part`, with `refusal`.
    pub fn refuse_secrets(part: &str, refusal: KeyRefusal) {
        SECRET_REFUSALS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((part.to_owned(), refusal));
    }

    /// The test services' key store: secrets in memory, shared by every test.
    pub struct TestSecrets;

    fn secret_refusal(name: &str) -> Result<(), KeyRefusal> {
        let refusals = SECRET_REFUSALS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match refusals
            .iter()
            .find(|(part, _)| name.contains(part.as_str()))
        {
            Some((_, refusal)) => Err(refusal.clone()),
            None => Ok(()),
        }
    }

    impl Secrets for TestSecrets {
        fn get(&self, name: &str) -> Result<Option<Vec<u8>>, KeyRefusal> {
            secret_refusal(name)?;
            Ok(secrets_kept().get(name).cloned())
        }

        fn set(&self, name: &str, value: Option<&[u8]>) -> Result<(), KeyRefusal> {
            secret_refusal(name)?;
            let mut kept = SECRETS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match value {
                Some(value) => kept.insert(name.to_owned(), value.to_vec()),
                None => kept.remove(name),
            };
            Ok(())
        }
    }

    static OPENED: std::sync::Mutex<Vec<(String, std::thread::ThreadId)>> =
        std::sync::Mutex::new(Vec::new());

    /// Each keyring a device key was opened for so far, by every test, and
    /// the thread it was opened on: a test tells a key opened off the
    /// thread that asked from one opened on it.
    pub fn keys_opened() -> Vec<(String, std::thread::ThreadId)> {
        OPENED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    thread_local! {
        /// This test thread's `Limits::snapshot_every`: the desktop's
        /// `None` unless a test keeps a death replay
        /// ([`snapshot_every_for_test`]).
        static SNAPSHOT_EVERY: std::cell::Cell<Option<std::time::Duration>> =
            const { std::cell::Cell::new(None) };
    }

    /// `Limits::snapshot_every` on this test thread from now on.
    pub fn snapshot_every_for_test(every: Option<std::time::Duration>) {
        SNAPSHOT_EVERY.set(every);
    }

    thread_local! {
        /// This test thread's `OsFacts::touch_first`: the desktop's `false`
        /// unless a test draws for a finger ([`touch_first_for_test`]).
        static TOUCH_FIRST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// `OsFacts::touch_first` on this test thread from now on.
    pub fn touch_first_for_test(touch: bool) {
        TOUCH_FIRST.set(touch);
    }

    #[cfg(feature = "ui")]
    thread_local! {
        static SOFT_KEYBOARD: std::cell::RefCell<Vec<SoftKeyboard>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    #[cfg(all(test, feature = "ui"))]
    pub(crate) fn soft_keyboard_asked_for_test() -> Vec<&'static str> {
        keyboards_asked_for_test()
            .into_iter()
            .map(|asked| match asked {
                SoftKeyboard::Show(_) => "show",
                SoftKeyboard::Hide => "hide",
            })
            .collect()
    }

    #[cfg(all(test, feature = "ui"))]
    pub(crate) fn keyboards_asked_for_test() -> Vec<SoftKeyboard> {
        SOFT_KEYBOARD.take()
    }

    impl OsServices for TestOs {
        fn dirs(&self) -> &Dirs {
            thread_local! {
                static DIRS: &'static Dirs = {
                    let thread = format!("{:?}", std::thread::current().id());
                    let thread = thread.trim_start_matches("ThreadId(").trim_end_matches(')');
                    let root = root_of(std::process::id()).join(thread);
                    // The rule `Dirs` sets every impl: `runtime` exists and
                    // is private before anything writes there.
                    let runtime = root.join("runtime");
                    {
                        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
                        std::fs::DirBuilder::new()
                            .recursive(true)
                            .mode(0o700)
                            .create(&runtime)
                            .and_then(|()| {
                                std::fs::set_permissions(
                                    &runtime,
                                    std::fs::Permissions::from_mode(0o700),
                                )
                            })
                            .expect("the test runtime directory is made private");
                    }
                    Box::leak(Box::new(Dirs {
                        cache: root.join("cache"),
                        config: root.join("config"),
                        data: root.join("data"),
                        remotes: root.join("remotes"),
                        runtime,
                        views_override: None,
                    }))
                };
            }
            DIRS.with(|dirs| *dirs)
        }

        fn device_key(
            &self,
            keyring: &str,
        ) -> Result<Option<Arc<dyn noded_client::Signer>>, KeyRefusal> {
            OPENED
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((keyring.to_owned(), std::thread::current().id()));
            // a key of the keyring's name: the same device key every time a
            // test meets the same network, none kept anywhere
            let mut seed = [0u8; 32];
            let name = keyring.as_bytes();
            let n = name.len().min(32);
            seed[..n].copy_from_slice(&name[..n]);
            Ok(Some(Arc::new(FakeKey(seed))))
        }

        fn key_storage(&self, _: &str) -> Result<KeyStorage, KeyRefusal> {
            Ok(KeyStorage {
                sealed: false,
                level: KeyLevel::File,
            })
        }

        fn secrets(&self) -> Option<&dyn Secrets> {
            Some(&TestSecrets)
        }

        fn reader(&self) -> Arc<dyn noded_client::Signer> {
            self.reader.clone()
        }

        fn banners(&self) -> &dyn Banners {
            &NoBanners
        }

        #[cfg(feature = "ui")]
        fn insets(&self, _: &Window) -> WindowInsets {
            WindowInsets::default()
        }

        #[cfg(feature = "ui")]
        fn soft_keyboard(&self, _: &Window, want: SoftKeyboard) {
            SOFT_KEYBOARD.with_borrow_mut(|asked| asked.push(want));
        }

        fn facts(&self) -> OsFacts {
            OsFacts {
                mac_modifiers: false,
                rss_unit: 1024,
                touch_first: TOUCH_FIRST.get(),
                device_label: "Test",
                device_noun: "computer",
                one_layout: false,
                floats: true,
                motion: crate::motion::IOS,
                os_asks_notices: false,
                // the phone's, which the screen tests play
                feeds_follow_screen: true,
            }
        }

        fn limits(&self) -> Limits {
            Limits {
                loaders: std::thread::available_parallelism()
                    .map_or(1, std::num::NonZeroUsize::get),
                preload: Preload::Compile,
                snapshot_every: SNAPSHOT_EVERY.get(),
                snapshot_to_disk: false,
            }
        }

        fn bundled_view(&self, _: &::abi::BlobId) -> Option<Vec<u8>> {
            None
        }

        fn mint_invite(&self, _: MintInvite) -> Option<Answered> {
            None
        }
    }

    /// `TestOs` keeps the rule `Dirs` sets every impl: its runtime
    /// directory exists and only this user can enter it.
    #[cfg(test)]
    #[test]
    fn the_test_runtime_directory_exists_and_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&TestOs::new().dirs().runtime)
            .expect("the runtime directory exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
