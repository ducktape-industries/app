//! The desktop's OS services: the one impl of the host core's interface
//! file (`ducktape_host::contract`). Directories by the platform's own
//! convention (`backend::app_dirs`), banners through the freedesktop
//! notifications service or macOS's notification centre, invites minted
//! with the key of the node this machine runs (`backend::invite`), and
//! the throwaway key that signs a query while no key is seated. The
//! updater's own notice ([`post_host`], [`withdraw_host`]) is the
//! desktop's too.

use ducktape_host::contract::{
    Banners, Dirs, Inbound, KeyRefusal, KeyStorage, Limits, MintInvite, OsFacts, OsServices,
    Preload, Signal, SoftKeyboard,
};
use ducktape_host::runtime::Answered;
use ducktape_host::runtime::notify::{Click, Notice, Settings};
use gpui_kit::{Window, WindowInsets};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

// Each platform module exposes `post(&Notice, click: Option<Click>) -> raised`,
// `withdraw(chain, tag)` and `withdraw_host()`. All block on OS calls; a view's run
// only through `notify::in_order` under the view's program, on the blocking
// pool, and the host's on the thread that posts it. A click on a banner
// goes to the host as `Signal::BannerClick(click)` through `Inbound`.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(target_os = "macos"))]
mod freedesktop;
#[cfg(not(target_os = "macos"))]
use freedesktop as platform;

/// What this device's key is labelled on the account it joins.
pub(crate) const DEVICE_LABEL: &str = "Desktop";

static INBOUND: OnceLock<Inbound> = OnceLock::new();

/// Where the OS's events go once the host started (`host::start`'s
/// `Inbound`): the desktop feeds banner clicks.
pub(crate) fn install_inbound(inbound: Inbound) {
    let _ = INBOUND.set(inbound);
    #[cfg(target_os = "macos")]
    macos::start();
}

/// A banner clicked, from the OS's own thread: into the host through the
/// one inbound channel.
fn clicked(click: Click) {
    match INBOUND.get() {
        Some(inbound) => inbound.send(Signal::BannerClick(click)),
        None => {
            tracing::error!(target: "ducktape::app", reason = "no_host", "a banner was clicked before the host started")
        }
    }
}

pub(crate) struct DesktopOs {
    dirs: Dirs,
    reader: Arc<dyn noded_client::Signer>,
}

impl DesktopOs {
    /// This user's directories, by the platform's rule; a home that cannot
    /// be resolved is the error that names the variable to set.
    pub(crate) fn new() -> Result<Self, String> {
        let cache = crate::backend::cache_dir()?;
        // the AX door file: `$XDG_RUNTIME_DIR/ducktape`, else the cache
        // (never the state directory, `~/Library/Logs` on macOS, where
        // support bundles collect)
        let runtime = match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
            Some(dir) => PathBuf::from(dir).join("ducktape"),
            None => cache.join("runtime"),
        };
        // the door file's token is written there plainly: the directory is
        // this user's alone
        private_dir(&runtime).map_err(|error| format!("{}: {error}", runtime.display()))?;
        Ok(Self {
            dirs: Dirs {
                config: crate::backend::config_dir()?,
                data: crate::backend::data_dir()?,
                runtime,
                // no view ships with the app: every one comes off the
                // connected node. A view developer's DUCKTAPE_VIEWS_DIR
                // supplies files in their place, and app.log says so for
                // each one it supplies
                views_override: std::env::var_os("DUCKTAPE_VIEWS_DIR").map(Into::into),
                // the keyring directories, `remotes/<name>` under the
                // ducktape home: the one source `backend::key_dir` reads too
                remotes: ducktape_home::root()?.join("remotes"),
                cache,
            },
            reader: Arc::new(reader_key()),
        })
    }
}

/// A directory only this user may enter, made if missing.
pub(crate) fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    #[cfg(unix)]
    std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    Ok(())
}

/// The updater's own notice: up if the device's banners are on, a click
/// opening Settings › Updates. One stands for the install, whichever of its
/// processes raised it: a newer one takes its place, and [`withdraw_host`]
/// takes it down. Blocks on the OS call, so the caller is a thread of its
/// own. Whether it went up.
pub(crate) fn post_host(notice: &Notice) -> bool {
    Settings::load().banners && platform::post(notice, Some(Click::Updates))
}

/// The updater's notice down, whichever process raised it. Blocks on the
/// OS call, as [`post_host`] does.
pub(crate) fn withdraw_host() {
    platform::withdraw_host()
}

/// A key of this process's own, never kept: what signs a query while no
/// key is seated.
fn reader_key() -> commonware_cryptography::ed25519::PrivateKey {
    use commonware_codec::DecodeExt as _;
    use rand::RngCore as _;
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    commonware_cryptography::ed25519::PrivateKey::decode(seed.as_slice())
        .expect("32 random bytes decode")
}

impl OsServices for DesktopOs {
    fn dirs(&self) -> &Dirs {
        &self.dirs
    }

    /// The OS store's key for `keyring` (`backend::device_key`: on macOS
    /// the data protection keychain, read with Touch ID), or the file it
    /// falls back to; a password-locked key from before keys moved into
    /// the OS is `None`, which `shell::sign_in` opens with its password
    /// once. The store's refusal sentences are one string today, so each
    /// reads as `Denied`.
    fn device_key(
        &self,
        keyring: &str,
    ) -> Result<Option<Arc<dyn noded_client::Signer>>, KeyRefusal> {
        let legacy = crate::backend::key_exists(keyring);
        match crate::backend::device_key::open(keyring, legacy) {
            Ok(key) => Ok(key.map(|key| Arc::new(key) as Arc<dyn noded_client::Signer>)),
            Err(sentence) => Err(KeyRefusal::Denied(sentence)),
        }
    }

    /// Asked without Touch ID (`backend::device_key::level`).
    fn key_storage(&self, keyring: &str) -> Result<KeyStorage, KeyRefusal> {
        Ok(KeyStorage {
            sealed: false,
            level: crate::backend::device_key::level(keyring).map_err(KeyRefusal::Denied)?,
        })
    }

    fn reader(&self) -> Arc<dyn noded_client::Signer> {
        self.reader.clone()
    }

    fn banners(&self) -> &dyn Banners {
        &DesktopBanners
    }

    fn insets(&self, _: &Window) -> WindowInsets {
        WindowInsets::default()
    }

    fn soft_keyboard(&self, _: &Window, _: SoftKeyboard) {}

    fn facts(&self) -> OsFacts {
        OsFacts {
            mac_modifiers: cfg!(target_os = "macos"),
            // `getrusage`'s `ru_maxrss`: bytes on macOS, kibibytes elsewhere
            rss_unit: if cfg!(target_os = "macos") { 1 } else { 1024 },
            touch_first: false,
            device_label: DEVICE_LABEL,
            device_noun: "computer",
            one_layout: false,
            floats: true,
            motion: ducktape_host::motion::IOS,
            os_asks_notices: false,
            feeds_follow_screen: false,
        }
    }

    fn limits(&self) -> Limits {
        Limits {
            // one loader a core: a fetch and a cranelift compile each
            loaders: std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
            preload: Preload::Compile,
            // wasmtime runs in this process: an instance dies with the app
            snapshot_every: None,
            snapshot_to_disk: false,
        }
    }

    /// The desktop ships no view: every one is the network's.
    fn bundled_view(&self, _: &abi::BlobId) -> Option<Vec<u8>> {
        None
    }

    /// With the key of the validator node this machine runs, read from
    /// the node's workspace; a node that is not this one refuses inside
    /// `mint`.
    fn mint_invite(&self, ask: MintInvite) -> Option<Answered> {
        let client = ask.client.clone();
        Some(Box::pin(async move {
            let host_fault = |error: String| {
                view_wire::Error::new(view_wire::methods::refusal::HOST_FAULT, error)
            };
            let workspace = ducktape_home::root().map_err(host_fault)?;
            let key = crate::backend::invite::node_key(&workspace).map_err(host_fault)?;
            let invite =
                crate::backend::invite::mint(&client, key, ask.ttl_days, ask.now_ms).await?;
            Ok(view_wire::methods::encode(&view_wire::methods::Invite {
                invite,
                notes: Vec::new(),
            }))
        }))
    }
}

/// The desktop's banners: the platform module's.
struct DesktopBanners;

impl Banners for DesktopBanners {
    fn post(&self, notice: &Notice, click: Option<Click>) -> bool {
        platform::post(notice, click)
    }

    fn withdraw(&self, chain: &str, tag: &str) {
        platform::withdraw(chain, tag)
    }
}
