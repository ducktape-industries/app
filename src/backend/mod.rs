//! What crosses the process boundary and only the desktop runs, over the
//! host core's shared half (`ducktape_host::backend`, re-exported whole):
//! this device's per-network key in the OS store ([`device_key`]) and the
//! key directory it falls back to ([`key_dir`]); a passkey ([`passkey`],
//! through [`auth_page`] and [`loopback`]) and a recovery key
//! ([`recovery`]) onto an account; invites minted with the key of the node
//! this machine runs ([`invite`]); the desktop's own directories
//! ([`app_dirs`]: the platform rule behind `Dirs`).

pub(crate) mod app_dirs;
pub(crate) mod auth_page;
pub(crate) mod device_key;
pub(crate) mod invite;
mod key_dir;
mod loopback;
pub(crate) mod passkey;
pub(crate) mod recovery;

pub(crate) use app_dirs::{app_log_path, cache_dir, config_dir, data_dir};
pub(crate) use ducktape_host::backend::*;
pub(crate) use key_dir::{bind_keyring, key_exists, keystore_root, session_key_path};
