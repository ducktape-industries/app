//! This device's key for a network: made the first time the device meets
//! the network, kept by the OS — Keychain on macOS, Credential Manager on
//! Windows, Secret Service on Linux — and never shown or typed. No password,
//! no recovery phrase: a lost device's key is not recovered, the ACCOUNT is
//! (another device, a passkey or a recovery key adds the new device's key).
//!
//! Where the OS keeps nothing (a headless Linux with no Secret Service
//! running), the key is a file only this user can read, the way an ssh key
//! without a passphrase is. `DUCKTAPE_DEVICE_KEY_STORE=file` picks the file
//! outright (tests, the qa rig).

use std::path::PathBuf;

use commonware_codec::{DecodeExt as _, Encode as _};
use commonware_cryptography::ed25519;
use zeroize::Zeroizing;

const SERVICE: &str = "dev.ducktape.app";

/// Where a device key lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Store {
    Os,
    File,
}

fn file_only() -> bool {
    cfg!(test) || std::env::var("DUCKTAPE_DEVICE_KEY_STORE").is_ok_and(|store| store == "file")
}

/// The OS entry for `keyring`'s key. Named by the keyring (a network, or a
/// network and its founding when two share a name), under this home: two
/// homes on one account keep two keys.
fn entry(keyring: &str) -> Option<keyring::Entry> {
    let home = ducktape_home::root().ok()?;
    let user = format!("device-key:{}:{keyring}", home.display());
    keyring::Entry::new(SERVICE, &user)
        .inspect_err(|error| tracing::debug!(target: "ducktape::keys", %error, "no OS key store"))
        .ok()
}

fn file(keyring: &str) -> Result<PathBuf, String> {
    Ok(super::keystore_root(keyring)?.join("device.key"))
}

fn decode(seed: &[u8]) -> Result<ed25519::PrivateKey, String> {
    ed25519::PrivateKey::decode(seed).map_err(|_| "this device's key is damaged".to_string())
}

/// This device's key for `keyring`, if it has one.
pub(crate) fn load(keyring: &str) -> Result<Option<ed25519::PrivateKey>, String> {
    if !file_only()
        && let Some(entry) = entry(keyring)
    {
        match entry.get_secret() {
            Ok(seed) => return decode(&Zeroizing::new(seed)).map(Some),
            Err(keyring::Error::NoEntry) => {}
            Err(error) => {
                tracing::debug!(target: "ducktape::keys", %error, "OS key store unreadable, trying the file");
            }
        }
    }
    load_file(&file(keyring)?)
}

fn load_file(path: &std::path::Path) -> Result<Option<ed25519::PrivateKey>, String> {
    match std::fs::read(path) {
        Ok(seed) => decode(&Zeroizing::new(seed)).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("this device's key could not be read: {error}")),
    }
}

/// Keeps `key` as this device's key for `keyring`: in the OS store, else
/// the file.
pub(crate) fn save(keyring: &str, key: &ed25519::PrivateKey) -> Result<Store, String> {
    let seed = Zeroizing::new(key.encode().to_vec());
    if !file_only()
        && let Some(entry) = entry(keyring)
    {
        match entry.set_secret(&seed) {
            Ok(()) => return Ok(Store::Os),
            Err(error) => {
                tracing::info!(target: "ducktape::keys", %error, "OS key store refused the key, keeping it in a file");
            }
        }
    }
    save_file(&file(keyring)?, &seed)?;
    Ok(Store::File)
}

fn save_file(path: &std::path::Path, seed: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    write_private(path, seed)
        .map_err(|error| format!("this device's key could not be kept: {error}"))
}

/// A new key for `keyring`, kept before it is handed back.
pub(crate) fn mint(keyring: &str) -> Result<ed25519::PrivateKey, String> {
    use rand::RngCore as _;
    let mut seed = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(seed.as_mut_slice());
    let key = decode(seed.as_slice())?;
    save(keyring, &key)?;
    Ok(key)
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::Signer as _;

    #[test]
    fn a_kept_key_loads_back_the_same_and_only_its_owner_reads_it() {
        let dir =
            std::env::temp_dir().join(format!("ducktape-device-key-{}", rand::random::<u64>()));
        let path = dir.join("testkit").join("device.key");
        assert!(load_file(&path).unwrap().is_none());
        let key = decode(&[7u8; 32]).unwrap();
        save_file(&path, &key.encode()).unwrap();
        let again = load_file(&path).unwrap().expect("kept");
        assert_eq!(key.public_key(), again.public_key());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "readable by others: {mode:o}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_old_recovery_phrase_is_the_same_key() {
        // the phrase a password-locked key was minted with names its seed
        let seed = [9u8; 32];
        let words = keystore::userkey::mnemonic_of_seed(&seed);
        let from_words = decode(&keystore::userkey::seed_of_mnemonic(&words).unwrap()).unwrap();
        assert_eq!(decode(&seed).unwrap().public_key(), from_words.public_key());
        assert_eq!(
            from_words.encode().as_ref(),
            seed.as_slice(),
            "a key encodes as its seed"
        );
    }
}
