//! Where a network's keys live on this device: `remotes/<name>` under the
//! ducktape home, bound to the chain first seen under that name; and the
//! password-locked key file from before keys moved into the OS.

use std::path::PathBuf;

use noded_client::keyring::{self, Keyring};

/// Where this device keeps its wallets for a network: the directory
/// [`bind_keyring`] named, under the ducktape home.
pub(crate) fn keystore_root(keyring: &str) -> Result<PathBuf, String> {
    let named = !keyring.is_empty() && !keyring.contains(['/', '\\']) && keyring != "..";
    if !named {
        return Err("the node named no network".into());
    }
    Ok(ducktape_home::root()?.join("remotes").join(keyring))
}

/// Binds `network` (founded at `founded`, the node's `Status::time`) to a
/// key directory under the ducktape home's `remotes`: the shared rule,
/// `noded_client::keyring::bind`.
pub(crate) fn bind_keyring(network: &str, founded: u64) -> Result<Keyring, String> {
    keyring::bind(&ducktape_home::root()?.join("remotes"), network, founded)
}

/// The key file a sign-in opens: `DUCKTAPE_USER_KEY`, else the keyring's
/// active wallet.
pub(crate) fn session_key_path(keyring: &str) -> Result<PathBuf, String> {
    keystore::wallet::active_user_key(&keystore_root(keyring)?)
}

/// Whether a password-locked key file from before keys moved into the OS is
/// here for `keyring`. The OS-kept key is [`device_key`](super::device_key)'s;
/// finding one of these instead, the key screen asks its password once and
/// moves it into the OS store.
pub(crate) fn key_exists(keyring: &str) -> bool {
    let Ok(path) = session_key_path(keyring) else {
        return false;
    };
    !matches!(
        keystore::userkey::key_file_state(&path),
        keystore::userkey::KeyFileState::Absent
    )
}
