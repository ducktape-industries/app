//! The one interface file: everything a shell gives the host core, and
//! everything a shell feeds it after start. An outside developer reads
//! this file and knows what a new OS must provide; nothing else in this
//! crate names an OS, a window manager or a key store.
//!
//! Two impls: the desktop's (`src/os.rs` in the app binary: directories
//! by the platform's rule, the OS keyring, freedesktop / macOS banners,
//! invites minted with the node this machine runs) and the phone's
//! (`crates/phone/src/os/`). A test build with none installed runs on
//! [`testing::TestOs`].
//!
//! The view runner (`ViewRunner`, its own trait handed to [`Boot`] beside
//! `OsServices`) is the next cut on this file.

use crate::backend::RpcClient;
use crate::runtime::Answered;
use crate::runtime::notify::{Click, Notice};
#[cfg(feature = "ui")]
use gpui_kit::{AppLifecyclePhase, TextInputConfiguration, Window, WindowInsets};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// What a shell gives the host. One impl per shell.
pub trait OsServices: Send + Sync + 'static {
    /// Where files go.
    fn dirs(&self) -> &Dirs;
    /// This device's key for `keyring`, made on first ask; it signs and
    /// never hands the key out. Called off the window thread (a store may
    /// prompt).
    fn device_key(&self, keyring: &str) -> Result<Arc<dyn noded_client::Signer>, KeyRefusal>;
    /// Where that key is kept, in the store's own words (the Settings row
    /// "Key storage").
    fn key_storage(&self, keyring: &str) -> Result<KeyStorage, KeyRefusal>;
    /// What signs a query while no key is seated (`backend::query_frame`).
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
    /// `invite.create`: mint with the node key of a node THIS device runs
    /// (the desktop reads it out of the node's workspace). `None` on a host
    /// that runs no node: the kernel refuses `invite_unsupported` with
    /// "Only the machine running this node can mint its invites."
    fn mint_invite(&self, ask: MintInvite) -> Option<Answered>;
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
    pub cache: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub remotes: PathBuf,
    pub runtime: PathBuf,
    pub views_override: Option<PathBuf>,
}

/// Where a device key is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStorage {
    /// The seed never leaves hardware (Secure Enclave, StrongBox).
    pub sealed: bool,
    pub level: KeyLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyLevel {
    SecureEnclave,
    Keychain,
    KeystoreStrongBox,
    KeystoreTee,
    KeystoreSoftware,
    OsKeyring,
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
    /// No OS-kept key, but a key from before keys moved into the OS is
    /// here under a password: the shell asks for it and seats it itself
    /// (the desktop's `shell::sign_in`). A phone never answers this.
    Legacy,
}

impl KeyRefusal {
    /// The sentence the key screen shows.
    pub fn sentence(&self) -> String {
        match self {
            Self::NoStore(s) | Self::Denied(s) | Self::Broken(s) => s.clone(),
            Self::Legacy => "Type this key's password first.".into(),
        }
    }
}

/// OS banners. Both calls block on the OS: a view's run through
/// `notify::in_order` on the blocking pool, the host's own on a thread of
/// their own.
pub trait Banners: Send + Sync {
    /// Raises `notice`; a click on it comes back as `click` through
    /// [`Signal::BannerClick`]. Whether it went up.
    fn post(&self, notice: &Notice, click: Option<Click>) -> bool;
    /// Takes down the banner standing under `tag`.
    fn withdraw(&self, tag: &str);
}

/// The software keyboard a focused field wants. (`InputMode`, the one
/// field gpui lacks, joins `Show` with the fork pin that adds it.)
#[cfg(feature = "ui")]
#[derive(Clone, Debug)]
pub enum SoftKeyboard {
    Show(TextInputConfiguration),
    Hide,
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
}

/// Values that differ by device and have a measured owner (SPEC §4.5).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Concurrent view loads. Desktop: `available_parallelism`. Phone: 1,
    /// because one Cranelift compile peaks at 200-315 MB and the phone's
    /// memory killer is the owner.
    pub loaders: usize,
    /// What a roster read does for a program nobody opened (SPEC §4.7).
    /// Read by the seating policy (the runner cut).
    pub preload: Preload,
    /// Whether settled snapshots are written under `Dirs::data` so a cold
    /// start reopens where the person was. Phone true; desktop false.
    /// Read by the runner cut.
    pub snapshot_to_disk: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preload {
    /// Compile every roster program, instantiate none (the desktop).
    Compile,
    /// Seat every roster program at connect, `in_front = false`.
    Seat,
}

/// `invite.create`'s ask: the node it is about (the mint reads its status
/// and validator set, so the ask carries the node's client), how long the
/// invite lives (`methods::CreateInvite.ttl_days`, a `u64` on the wire),
/// and the clock.
#[derive(Clone)]
pub struct MintInvite {
    pub client: RpcClient,
    pub ttl_days: u64,
    pub now_ms: u64,
}

/// Everything the shell feeds the host after [`start`](crate::start::start):
/// one sender, any thread; one foreground task drains it on the GPUI
/// thread into the entities. The only way an OS event reaches host state.
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

#[cfg(feature = "ui")]
#[derive(Clone, Debug)]
pub enum Signal {
    /// gpui's phase. `Background` stops the status poll (and, with the
    /// runner cut, every tick); `Foreground` resumes them.
    Lifecycle(AppLifecyclePhase),
    /// iOS `didReceiveMemoryWarning` / Android `onTrimMemory`.
    MemoryWarning,
    /// Android BACK from the Activity's dispatcher; iOS from the capsule
    /// or the edge pan.
    Back,
    /// The insets changed: re-read `OsServices::insets`, re-place.
    InsetsChanged,
    BannerClick(Click),
}

/// What a shell hands [`start`](crate::start::start). The view runner joins
/// it with the runner cut.
#[cfg(feature = "ui")]
pub struct Boot {
    pub os: Arc<dyn OsServices>,
}

static OS: OnceLock<Arc<dyn OsServices>> = OnceLock::new();

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

    /// Every banner the host asked for so far, by every test.
    pub fn banners_asked() -> Vec<Notice> {
        ASKED
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
            false
        }

        fn withdraw(&self, _: &str) {}
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

        fn device_key(&self, keyring: &str) -> Result<Arc<dyn noded_client::Signer>, KeyRefusal> {
            // a key of the keyring's name: the same device key every time a
            // test meets the same network, none kept anywhere
            let mut seed = [0u8; 32];
            let name = keyring.as_bytes();
            let n = name.len().min(32);
            seed[..n].copy_from_slice(&name[..n]);
            Ok(Arc::new(FakeKey(seed)))
        }

        fn key_storage(&self, _: &str) -> Result<KeyStorage, KeyRefusal> {
            Ok(KeyStorage {
                sealed: false,
                level: KeyLevel::File,
            })
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
        fn soft_keyboard(&self, _: &Window, _: SoftKeyboard) {}

        fn facts(&self) -> OsFacts {
            OsFacts {
                mac_modifiers: false,
                rss_unit: 1024,
                touch_first: false,
                device_label: "Test",
            }
        }

        fn limits(&self) -> Limits {
            Limits {
                loaders: std::thread::available_parallelism()
                    .map_or(1, std::num::NonZeroUsize::get),
                preload: Preload::Compile,
                snapshot_to_disk: false,
            }
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
