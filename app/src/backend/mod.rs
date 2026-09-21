use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// this device's signing key, opened in-process by `keystore` and signing
// frames through `noded::Frame::sign` — see `rpc::Signer`.
use commonware_cryptography::{Signer as _, ed25519};
use futures::{FutureExt as _, StreamExt as _};
pub(crate) use noded::{Client as RpcClient, Status as NodeStatus};
use tokio::sync::OwnedSemaphorePermit;
use zeroize::Zeroizing;


pub(crate) mod workspace_config;


/// How many one-second polls the provisioning screen waits before it says the
/// node is not running and names the command that starts it.
const PROVISION_PATIENCE: u32 = 8;

#[derive(Clone, Debug, Hash, PartialEq)]
pub struct WorkspaceData {
    pub generation: i64,
    pub rpc: String,
    pub status: String,
    pub height: i64,
}

#[derive(Clone, Debug, Hash, PartialEq)]
pub struct AppError {
    pub message: String,
    pub committed: bool,
}

impl From<String> for AppError {
    fn from(message: String) -> Self {
        Self {
            message: user_error(message),
            committed: false,
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq)]
pub struct HydrationError {
    pub generation: i64,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveUpdate {
    /// `ready` (topics subscribed — run the catch-up resync), `retry`
    /// (stream down, reconnecting), `chat` (one ordered bounded delta batch),
    /// `pages` (one folded delta), `resync` (this module's replay lagged —
    /// reload its slices).
    pub kind: crate::LiveKind,
    pub status: String,
    pub height: i64,
    /// the module needing a scoped resync (`kind == LiveKind::Resync`).
    pub module: String,
    /// trail 100ms so a burst of ops coalesces into one reload.
    pub debounce: bool,
    /// Subscription backpressure, not UI state. The next socket publication
    /// cannot be read until the app message carrying this token has
    /// finished its update and all of its clones have been dropped.
    pub(crate) permit: LivePermit,
}

#[derive(Clone, Default)]
pub(crate) struct LivePermit(Option<Arc<OwnedSemaphorePermit>>);

impl LivePermit {
    pub(crate) fn held(permit: OwnedSemaphorePermit) -> Self {
        Self(Some(Arc::new(permit)))
    }

    #[cfg(test)]
    pub(crate) fn is_held(&self) -> bool {
        self.0.is_some()
    }
}

impl std::fmt::Debug for LivePermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("LivePermit")
            .field(&self.0.is_some())
            .finish()
    }
}

impl PartialEq for LivePermit {
    fn eq(&self, _other: &Self) -> bool {
        // The permit changes scheduling only; it is not part of a publication's
        // domain value and must not perturb reducer/test equality.
        true
    }
}

impl Default for LiveUpdate {
    fn default() -> Self {
        Self {
            kind: crate::LiveKind::Retry,
            status: String::new(),
            height: 0,
            module: String::new(),
            debounce: false,
            permit: LivePermit::default(),
        }
    }
}

mod app_dirs;
mod load;
mod model;
mod node;
mod noded;
mod notify;
mod rpc;
mod shell;
mod style;
mod view_artifact;
pub mod view_source;

pub use app_dirs::app_log_path;
pub(crate) use app_dirs::cache_dir;
pub(crate) use app_dirs::state_dir;
pub(crate) use load::*;
pub use model::*;
pub use node::*;
pub use notify::*;
pub use rpc::*;
pub use shell::*;
pub(crate) use style::*;

#[cfg(test)]
mod tests;
