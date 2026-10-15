//! The desktop's OS services: the one impl of the host core's interface
//! file (`ducktape_host::contract`). Directories by the platform's own
//! convention (`backend::app_dirs`), banners through the freedesktop
//! notifications service or macOS's notification centre, invites minted
//! with the key of the node this machine runs (`backend::invite`), and
//! the throwaway key that signs a query while no key is seated.

use ducktape_host::contract::{Banners, Dirs, MintInvite, OsFacts, OsServices};
use ducktape_host::runtime::Answered;
use ducktape_host::runtime::notify::{Click, Notice};
use std::path::PathBuf;
use std::sync::Arc;

// Each platform module exposes `post(&Notice, click: Option<Click>) -> raised`,
// `withdraw(tag)` and `withdraw_host()`. All block on OS calls; a view's run
// only through `notify::in_order` under the view's program, on the blocking
// pool, and the host's on the thread that posts it. A click on a banner
// calls back `notify::clicked(click)`.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(target_os = "macos"))]
mod freedesktop;
#[cfg(not(target_os = "macos"))]
use freedesktop as platform;

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
            None => cache.clone(),
        };
        Ok(Self {
            dirs: Dirs {
                config: crate::backend::config_dir()?,
                data: crate::backend::data_dir()?,
                state: crate::backend::app_dirs::state_dir()?,
                runtime,
                // no view ships with the app: every one comes off the
                // connected node. A view developer's DUCKTAPE_VIEWS_DIR
                // supplies files in their place, and app.log says so for
                // each one it supplies
                views_override: std::env::var_os("DUCKTAPE_VIEWS_DIR").map(Into::into),
                cache,
            },
            reader: Arc::new(reader_key()),
        })
    }
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

    fn reader(&self) -> Arc<dyn noded_client::Signer> {
        self.reader.clone()
    }

    fn banners(&self) -> &dyn Banners {
        &DesktopBanners
    }

    fn facts(&self) -> OsFacts {
        OsFacts {
            mac_modifiers: cfg!(target_os = "macos"),
            // `getrusage`'s `ru_maxrss`: bytes on macOS, kibibytes elsewhere
            rss_unit: if cfg!(target_os = "macos") { 1 } else { 1024 },
        }
    }

    /// With the key of the validator node this machine runs, read from
    /// the node's workspace; a node that is not this one refuses inside
    /// `mint`.
    fn mint_invite(&self, client: &crate::backend::RpcClient, ask: MintInvite) -> Option<Answered> {
        let client = client.clone();
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

    fn withdraw(&self, tag: &str) {
        platform::withdraw(tag)
    }

    fn withdraw_host(&self) {
        platform::withdraw_host()
    }
}
