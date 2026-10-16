//! Where a network's keys live on this device: `remotes/<name>` under the
//! ducktape home (`Dirs::remotes`, bound to the chain first seen under
//! that name by `Session`); and the password-locked key file from before
//! keys moved into the OS.

use std::path::PathBuf;

/// Where this device keeps its wallets for a network: the directory
/// `Session` bound it to, under the ducktape home.
pub(crate) fn keystore_root(keyring: &str) -> Result<PathBuf, String> {
    let named = !keyring.is_empty() && !keyring.contains(['/', '\\']) && keyring != "..";
    if !named {
        return Err("the node named no network".into());
    }
    Ok(ducktape_host::contract::os().dirs().remotes.join(keyring))
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
