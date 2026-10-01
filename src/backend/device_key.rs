//! This device's key for a network: made the first time the device meets
//! the network, kept by the OS — Keychain on macOS, Credential Manager on
//! Windows, Secret Service on Linux — and never shown or typed. No password,
//! no recovery phrase: a lost device's key is not recovered, the ACCOUNT is
//! (another device, a passkey or a recovery key adds the new device's key).
//!
//! A store that errors is never taken for an empty one. Locked, a prompt
//! dismissed or denied, two entries, no keyring running at all: each is a
//! sentence on the key screen, and no key is made or kept over the one the
//! store may hold. Only "nothing here" lets a key be made, and a key is
//! only ever kept where nothing is.
//!
//! Where the OS keeps nothing (a headless Linux with no Secret Service
//! running), `DUCKTAPE_DEVICE_KEY_STORE=file` keeps the key in a file only
//! this user can read, the way an ssh key without a passphrase is (tests,
//! the qa rig). A file kept by an earlier build is still read when the
//! store answers that it holds nothing.

use std::path::{Path, PathBuf};

use commonware_codec::{DecodeExt as _, Encode as _};
use commonware_cryptography::{Signer as _, ed25519};
use zeroize::Zeroizing;

const SERVICE: &str = "dev.ducktape.app";

fn file_only() -> bool {
    cfg!(test) || std::env::var("DUCKTAPE_DEVICE_KEY_STORE").is_ok_and(|store| store == "file")
}

/// The OS entry for `keyring`'s key; `None` when the file is picked
/// ([`file_only`]). Named by the keyring (a network, or a network and its
/// founding when two share a name), under this home: two homes on one
/// account keep two keys.
fn entry(keyring: &str) -> Result<Option<keyring::Entry>, String> {
    if file_only() {
        return Ok(None);
    }
    let home = ducktape_home::root()?;
    let user = format!("device-key:{}:{keyring}", home.display());
    keyring::Entry::new(SERVICE, &user)
        .map(Some)
        .map_err(refused)
}

/// What the OS store keeps under `entry`: `None` only when it answers that
/// it keeps nothing. Any other answer is a refusal, never an empty store.
fn kept(entry: &keyring::Entry) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    match entry.get_secret() {
        Ok(seed) => Ok(Some(Zeroizing::new(seed))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(refused(error)),
    }
}

/// The key screen's sentence for a store that answered with an error: what
/// to do about it. The store's own words go to the log. On Linux a store
/// that refuses may be no keyring at all (none made yet, no daemon), whose
/// way out is the file store; two keys under one name is not that case,
/// and a file there would only make a third.
fn refused(error: keyring::Error) -> String {
    tracing::info!(target: "ducktape::keys", %error, "OS key store refused");
    let no_keyring = match cfg!(target_os = "linux") {
        true => {
            " If this device has no keyring, start the app with DUCKTAPE_DEVICE_KEY_STORE=file to keep its key in a file."
        }
        false => "",
    };
    match error {
        keyring::Error::NoStorageAccess(_) => format!(
            "The system's key store is locked or has no keyring yet. Unlock it or make one, then try again.{no_keyring}"
        ),
        keyring::Error::Ambiguous(_) => format!(
            "The system's key store holds more than one {SERVICE} key for this network, so none is used. Remove the one this device doesn't use, then try again."
        ),
        _ => format!(
            "The system's key store said no or didn't answer. If it asks, allow this app, then try again.{no_keyring}"
        ),
    }
}

fn file(keyring: &str) -> Result<PathBuf, String> {
    Ok(super::keystore_root(keyring)?.join("device.key"))
}

fn decode(seed: &[u8]) -> Result<ed25519::PrivateKey, String> {
    ed25519::PrivateKey::decode(seed).map_err(|_| "this device's key is damaged".to_string())
}

/// This device's key for `keyring`: opened, or made the first time the
/// device meets the network. `None` when nothing keeps one but `legacy`, a
/// password-locked key from before, is here: its password moves it into
/// the OS ([`save`]). A store that refuses is the error, never "no key".
pub(crate) fn open(keyring: &str, legacy: bool) -> Result<Option<ed25519::PrivateKey>, String> {
    open_in(entry(keyring)?.as_ref(), &file(keyring)?, legacy)
}

fn open_in(
    entry: Option<&keyring::Entry>,
    file: &Path,
    legacy: bool,
) -> Result<Option<ed25519::PrivateKey>, String> {
    match load_in(entry, file)? {
        Some(key) => Ok(Some(key)),
        None if legacy => Ok(None),
        None => mint(entry, file).map(Some),
    }
}

/// The key kept in the OS store, else (when the store answers that it
/// holds nothing) in the file.
fn load_in(
    entry: Option<&keyring::Entry>,
    file: &Path,
) -> Result<Option<ed25519::PrivateKey>, String> {
    if let Some(entry) = entry
        && let Some(seed) = kept(entry)?
    {
        return decode(&seed).map(Some);
    }
    load_file(file)
}

fn load_file(path: &Path) -> Result<Option<ed25519::PrivateKey>, String> {
    match std::fs::read(path) {
        Ok(seed) => decode(&Zeroizing::new(seed)).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("this device's key could not be read: {error}")),
    }
}

/// Keeps `key` as this device's key for `keyring`, in the OS store (the
/// file when it is picked), and only where nothing is kept yet: another
/// key already there stays, and a store that refuses is the error.
pub(crate) fn save(keyring: &str, key: &ed25519::PrivateKey) -> Result<(), String> {
    save_in(entry(keyring)?.as_ref(), &file(keyring)?, key)
}

fn save_in(
    entry: Option<&keyring::Entry>,
    file: &Path,
    key: &ed25519::PrivateKey,
) -> Result<(), String> {
    match load_in(entry, file)? {
        Some(kept) if kept.public_key() == key.public_key() => return Ok(()),
        Some(_) => return Err("this device already keeps another key for this network".into()),
        None => {}
    }
    let seed = Zeroizing::new(key.encode().to_vec());
    match entry {
        Some(entry) => entry.set_secret(&seed).map_err(refused),
        None => save_file(file, &seed),
    }
}

fn save_file(path: &Path, seed: &[u8]) -> Result<(), String> {
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| super::atomic_write(path, seed, true))
        .map_err(|error| format!("this device's key could not be kept: {error}"))
}

/// A new key, kept before it is handed back. Only [`open_in`] makes one,
/// when nothing keeps a key; [`save_in`] keeps it only where nothing is.
fn mint(entry: Option<&keyring::Entry>, file: &Path) -> Result<ed25519::PrivateKey, String> {
    use rand::RngCore as _;
    let mut seed = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(seed.as_mut_slice());
    let key = decode(seed.as_slice())?;
    save_in(entry, file, &key)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// An OS store entry holding `held`, its next call refused with `error`.
    fn store(held: Option<&ed25519::PrivateKey>, error: Option<keyring::Error>) -> keyring::Entry {
        let mock = keyring::mock::MockCredential::default();
        mock.inner.lock().unwrap().get_mut().secret = held.map(|key| key.encode().to_vec());
        if let Some(error) = error {
            mock.set_error(error);
        }
        keyring::Entry::new_with_credential(Box::new(mock))
    }

    fn held(entry: &keyring::Entry) -> Option<Vec<u8>> {
        let mock: &keyring::mock::MockCredential = entry.get_credential().downcast_ref().unwrap();
        mock.inner.lock().unwrap().get_mut().secret.clone()
    }

    fn platform(text: &str) -> Box<dyn std::error::Error + Send + Sync> {
        text.into()
    }

    /// A locked keyring, a dismissed or denied prompt, two entries: the
    /// store's key stays, nothing is made, nothing goes to a file, and the
    /// key screen says what to do: on Linux, where a refusal may be no
    /// keyring at all, that includes the file store (not for two entries,
    /// where a file would only make a third key). The store answers the
    /// next call (the prompt allowed the second time), as in the audit's R3.
    #[test]
    fn a_store_that_refuses_never_mints_and_never_writes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("testkit").join("device.key");
        let account = decode(&[3u8; 32]).unwrap();
        let refusals = [
            keyring::Error::NoStorageAccess(platform("prompt dismissed")),
            keyring::Error::PlatformFailure(platform("errSecAuthFailed")),
            keyring::Error::Ambiguous(Vec::new()),
        ];
        for refusal in refusals {
            let shown = refusal.to_string();
            let two_keys = matches!(refusal, keyring::Error::Ambiguous(_));
            let entry = store(Some(&account), Some(refusal));
            let sentence = open_in(Some(&entry), &file, false).expect_err(&shown);
            assert!(sentence.contains("try again"), "{shown}: {sentence}");
            assert_eq!(
                sentence.contains("DUCKTAPE_DEVICE_KEY_STORE=file"),
                cfg!(target_os = "linux") && !two_keys,
                "{shown}: {sentence}"
            );
            assert_eq!(
                held(&entry).as_deref(),
                Some(account.encode().as_ref()),
                "{shown}: the store's key was replaced"
            );
            assert!(!file.exists(), "{shown}: a key went to the file");
        }
    }

    /// First contact: the store answers that it holds nothing, a key is made
    /// once and kept there, and the next open is that key.
    #[test]
    fn a_store_that_holds_nothing_gets_one_key() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let entry = store(None, None);
        assert!(open_in(Some(&entry), &file, true).unwrap().is_none());
        assert!(
            held(&entry).is_none(),
            "made one beside a password-locked key"
        );
        let made = open_in(Some(&entry), &file, false).unwrap().unwrap();
        assert_eq!(held(&entry).as_deref(), Some(made.encode().as_ref()));
        let again = open_in(Some(&entry), &file, false).unwrap().unwrap();
        assert_eq!(made.public_key(), again.public_key());
        assert!(!file.exists());
    }

    /// A key is kept only where nothing is: the move of a password-locked
    /// key never replaces the key the store or the file already holds.
    #[test]
    fn a_key_is_never_kept_over_another() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let (account, other) = (decode(&[3u8; 32]).unwrap(), decode(&[4u8; 32]).unwrap());
        let entry = store(Some(&account), None);
        assert!(save_in(Some(&entry), &file, &other).is_err());
        assert_eq!(held(&entry).as_deref(), Some(account.encode().as_ref()));
        save_in(Some(&entry), &file, &account).unwrap();
        let refusing = store(
            Some(&account),
            Some(keyring::Error::NoStorageAccess(platform("locked"))),
        );
        assert!(save_in(Some(&refusing), &file, &other).is_err());
        assert_eq!(held(&refusing).as_deref(), Some(account.encode().as_ref()));
        save_file(&file, &account.encode()).unwrap();
        assert!(save_in(None, &file, &other).is_err());
        assert!(save_in(Some(&store(None, None)), &file, &other).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), account.encode().as_ref());
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
