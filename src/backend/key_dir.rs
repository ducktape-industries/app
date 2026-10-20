//! Where a network's keys live on this device: `remotes/<name>` under the
//! ducktape home (`Dirs::remotes`, bound to the chain first seen under
//! that name by `Session`); and the password-locked key file from before
//! keys moved into the OS.

use std::path::PathBuf;

pub(crate) fn keystore_root(keyring: &str) -> Result<PathBuf, String> {
    let named = !keyring.is_empty() && !keyring.contains(['/', '\\']) && keyring != "..";
    if !named {
        return Err("the node named no network".into());
    }
    Ok(ducktape_host::contract::os().dirs().remotes.join(keyring))
}

fn session_key_path(keyring: &str) -> Result<PathBuf, String> {
    keystore::wallet::active_user_key(&keystore_root(keyring)?)
}

pub(crate) fn key_exists(keyring: &str) -> bool {
    let Ok(path) = session_key_path(keyring) else {
        return false;
    };
    !matches!(
        keystore::userkey::key_file_state(&path),
        keystore::userkey::KeyFileState::Absent
    )
}

pub(crate) fn open_password_locked(
    keyring: &str,
    password: &str,
) -> Result<commonware_cryptography::ed25519::PrivateKey, String> {
    let key = keystore::userkey::open_user_key_at(&session_key_path(keyring)?, password)?;
    if let Err(error) = super::device_key::save(keyring, &key) {
        tracing::info!(target: "ducktape::keys", %error, "password-locked key not moved into the OS");
    }
    Ok(key)
}
