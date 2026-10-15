//! The one interface file: what a shell gives the host core. An outside
//! developer reads this file and knows what a new OS must provide; nothing
//! else in this crate names an OS, a window manager or a key store.
//!
//! This is the first cut of the seam (the desktop shell's impl is the app
//! binary's `src/os.rs`): every method here replaces an OS `cfg`, an OS
//! crate or a desktop-only read that the moved code carried. What a
//! shell feeds the host after start (lifecycle, memory warnings, back,
//! insets), this device's key, the soft keyboard, the per-device limits and
//! the view runner follow in the next cuts, on this same trait.

use crate::backend::RpcClient;
use crate::runtime::Answered;
use crate::runtime::notify::{Click, Notice};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// What a shell gives the host. One impl per shell.
pub trait OsServices: Send + Sync + 'static {
    /// Where files go.
    fn dirs(&self) -> &Dirs;
    /// What signs a query while no key is seated (`backend::query_frame`):
    /// the desktop's throwaway reader key. A phone always has a device key,
    /// so its reader is that key.
    fn reader(&self) -> Arc<dyn noded_client::Signer>;
    /// OS banners: post and withdraw. A click comes back through
    /// `runtime::notify::clicked`.
    fn banners(&self) -> &dyn Banners;
    /// Facts that are constants per OS, read once.
    fn facts(&self) -> OsFacts;
    /// `invite.create`: mint with the node key of a node THIS device runs
    /// (the desktop reads it out of the node's workspace). `None` on a host
    /// that runs no node: the kernel refuses `invite_unsupported` with
    /// "Only the machine running this node can mint its invites."
    fn mint_invite(&self, client: &RpcClient, ask: MintInvite) -> Option<Answered>;
}

/// Where the host keeps files. `cache` the OS may purge; `config`, `data`
/// and `state` it keeps (`state` holds the log); `runtime` is the AX door
/// file's directory; `views_override` is a VIEW DEVELOPER'S directory of
/// `<program>_view.wasm` files that stand in for the network's (the desktop
/// reads `DUCKTAPE_VIEWS_DIR`; a phone has none).
#[derive(Clone, Debug)]
pub struct Dirs {
    pub cache: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub runtime: PathBuf,
    pub views_override: Option<PathBuf>,
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
}

/// OS banners. Both calls block on the OS: a view's run through
/// `notify::in_order` on the blocking pool, the host's own on a thread of
/// their own.
pub trait Banners: Send + Sync {
    /// Raises `notice`; a click on it comes back as `click` through
    /// `runtime::notify::clicked`. Whether it went up.
    fn post(&self, notice: &Notice, click: Option<Click>) -> bool;
    /// Takes down the banner standing under `tag`.
    fn withdraw(&self, tag: &str);
    /// Takes down the host's own notice (the one a click opens Settings ›
    /// Updates from), whichever process raised it.
    fn withdraw_host(&self);
}

/// `invite.create`'s ask: how long the invite lives, and the clock.
#[derive(Clone, Copy, Debug)]
pub struct MintInvite {
    pub ttl_days: u64,
    pub now_ms: u64,
}

static OS: OnceLock<Arc<dyn OsServices>> = OnceLock::new();

/// The shell installs its services once, before the host runs anything.
pub fn install(os: Arc<dyn OsServices>) {
    if OS.set(os).is_err() {
        panic!("the shell's OsServices are installed once");
    }
}

/// The installed services. A test build with none installed runs on
/// [`testing::TestOs`]: files under a directory of the process's own, no
/// banners, no node.
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

    /// Files under one directory of this process's own, no banners, a reader
    /// key of its own, no node to mint invites with.
    pub struct TestOs {
        dirs: Dirs,
        reader: Arc<dyn noded_client::Signer>,
    }

    impl TestOs {
        pub fn new() -> Self {
            let root = std::env::temp_dir().join(format!("ducktape-test-{}", std::process::id()));
            Self {
                dirs: Dirs {
                    cache: root.join("cache"),
                    config: root.join("config"),
                    data: root.join("data"),
                    state: root.join("state"),
                    runtime: root.join("runtime"),
                    views_override: None,
                },
                reader: Arc::new(FixedReader),
            }
        }
    }

    impl Default for TestOs {
        fn default() -> Self {
            Self::new()
        }
    }

    /// A reader key that signs with fixed bytes: the tests read a query
    /// frame's shape, not its proof.
    pub struct FixedReader;

    impl noded_client::Signer for FixedReader {
        fn public_key(&self) -> [u8; 32] {
            [7; 32]
        }

        fn sign(&self, _: &[u8], _: &[u8]) -> Result<[u8; 64], noded_client::SignError> {
            Ok([8; 64])
        }
    }

    /// Banners that go nowhere.
    pub struct NoBanners;

    impl Banners for NoBanners {
        fn post(&self, _: &Notice, _: Option<Click>) -> bool {
            false
        }

        fn withdraw(&self, _: &str) {}

        fn withdraw_host(&self) {}
    }

    impl OsServices for TestOs {
        fn dirs(&self) -> &Dirs {
            &self.dirs
        }

        fn reader(&self) -> Arc<dyn noded_client::Signer> {
            self.reader.clone()
        }

        fn banners(&self) -> &dyn Banners {
            &NoBanners
        }

        fn facts(&self) -> OsFacts {
            OsFacts {
                mac_modifiers: false,
                rss_unit: 1024,
            }
        }

        fn mint_invite(&self, _: &RpcClient, _: MintInvite) -> Option<Answered> {
            None
        }
    }
}
