//! This device's key for a network: made the first time the device meets
//! the network, kept by the OS — Keychain on macOS, Credential Manager on
//! Windows, Secret Service on Linux — and never shown or typed. No password,
//! no recovery phrase: a lost device's key is not recovered, the ACCOUNT is
//! (another device, a passkey or a recovery key adds the new device's key).
//!
//! On macOS the key lives in the data protection keychain, this device
//! only, never synchronized, and reading it asks for Touch ID (the login
//! password on a Mac without it): the key itself is locked, not a check in
//! front of it. Keeping one asks nothing. A key the file keychain kept for
//! an earlier build moves there once; only once it is kept there is the
//! file keychain's item overwritten with [`MOVED`], which is never a key.
//! A build that can't reach the keychain (one from before, a rollback, a
//! local build) then refuses instead of making this Mac a second device.
//!
//! A store that errors is never taken for an empty one. Locked, a prompt
//! dismissed or denied, Touch ID cancelled, two entries, no keyring running
//! at all: each is a sentence on the key screen, and no key is made or kept
//! over the one the store may hold. Only "nothing here" lets a key be made,
//! and a key is only ever kept where nothing is.
//!
//! Where the OS keeps nothing (a headless Linux with no Secret Service
//! running), `DUCKTAPE_DEVICE_KEY_STORE=file` keeps the key in a file only
//! this user can read, the way an ssh key without a passphrase is (tests,
//! the qa rig). A file kept by an earlier build is still read when the
//! store answers that it holds nothing.

use std::path::{Path, PathBuf};

use commonware_codec::{DecodeExt as _, Encode as _};
use commonware_cryptography::{Signer as _, ed25519};
use ducktape_host::contract::KeyLevel;
use zeroize::Zeroizing;

const SERVICE: &str = "dev.ducktape.app";

/// What the file keychain holds once its key moved into the data protection
/// keychain: not 32 bytes, so never a key. A build from before reads it as
/// a damaged key, this one as a key it can't open; neither makes one.
const MOVED: &[u8] = b"moved to the data protection keychain";

// The Security framework's answers (`OSStatus`) the order below reads.
/// `errSecItemNotFound`: nothing here.
const NOT_FOUND: i32 = -25300;
/// `errSecDuplicateItem`: something is here already.
const DUPLICATE: i32 = -25299;
/// `errSecMissingEntitlement`: this build may not use the data protection
/// keychain.
const NOT_ENTITLED: i32 = -34018;
/// `errSecUserCanceled`: Touch ID or the password sheet was cancelled.
const CANCELLED: i32 = -128;
/// `errSecAuthFailed`: Touch ID or the password did not match.
const AUTH_FAILED: i32 = -25293;
/// `errSecInteractionNotAllowed`: the item is here but would have to ask,
/// and may not: this Mac is locked, or the query said never to ask.
const LOCKED: i32 = -25308;

fn file_only() -> bool {
    cfg!(test) || std::env::var("DUCKTAPE_DEVICE_KEY_STORE").is_ok_and(|store| store == "file")
}

/// The data protection keychain's item for one key (macOS; a fake in the
/// tests), answering in the Security framework's own codes.
trait Keychain {
    /// The seed kept here. Asks for Touch ID.
    fn copy(&self) -> Result<Zeroizing<Vec<u8>>, i32>;
    /// Whether an item is here, never asking: `NOT_FOUND` when not,
    /// `LOCKED` when it is but reading it would ask.
    fn find(&self) -> Result<(), i32>;
    /// Keeps `seed` here only where nothing is (`DUPLICATE` otherwise),
    /// without asking.
    fn add(&self, seed: &[u8]) -> Result<(), i32>;
}

/// The data protection keychain (macOS) and the `keyring` crate's entry
/// (the file keychain there); `None` when the file is picked.
type Stores = Option<(Option<Box<dyn Keychain>>, keyring::Entry)>;

/// The OS stores for `keyring`'s key; none when the file is picked
/// ([`file_only`]). Both are named by the keyring (a network, or a network
/// and its founding when two share a name), under this home: two homes on
/// one account keep two keys.
fn stores(keyring: &str) -> Result<Stores, String> {
    if file_only() {
        return Ok(None);
    }
    let home = ducktape_home::root()?;
    let account = format!("device-key:{}:{keyring}", home.display());
    let entry = keyring::Entry::new(SERVICE, &account).map_err(refused)?;
    #[cfg(target_os = "macos")]
    let keychain: Option<Box<dyn Keychain>> = Some(Box::new(keychain::Item::new(SERVICE, account)));
    #[cfg(not(target_os = "macos"))]
    let keychain = None;
    Ok(Some((keychain, entry)))
}

/// [`Stores`] as the order below takes them.
fn split(stores: &Stores) -> (Option<&dyn Keychain>, Option<&keyring::Entry>) {
    stores.as_ref().map_or((None, None), |(keychain, entry)| {
        (keychain.as_deref(), Some(entry))
    })
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

/// The key screen's sentence for the data protection keychain's refusal;
/// its code goes to the log.
fn keychain_refused(status: i32) -> String {
    tracing::info!(target: "ducktape::keys", status, "data protection keychain refused");
    match status {
        CANCELLED => "Unlocking this device's key was cancelled. Try again, and confirm with Touch ID or your password.",
        AUTH_FAILED => "Touch ID or the password didn't match, so this device's key stayed locked. Try again.",
        LOCKED => "This Mac is locked. Unlock it, then try again.",
        _ => "The system's key store said no or didn't answer. Try again.",
    }
    .into()
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
    let stores = stores(keyring)?;
    let (keychain, entry) = split(&stores);
    open_in(keychain, entry, &file(keyring)?, legacy)
}

/// The order: the data protection keychain's key (Touch ID); else the one
/// the file keychain or the file keeps, moved into it once; else, unless a
/// password-locked key waits, a new one kept there.
fn open_in(
    keychain: Option<&dyn Keychain>,
    entry: Option<&keyring::Entry>,
    file: &Path,
    legacy: bool,
) -> Result<Option<ed25519::PrivateKey>, String> {
    let keychain = match keychain {
        Some(keychain) => match keychain.copy() {
            Ok(seed) => return decode(&seed).map(Some),
            Err(NOT_FOUND) => Some(keychain),
            // a build without the keychain entitlement (ad-hoc, local,
            // unbundled): its keys stay in the file keychain
            Err(NOT_ENTITLED) => None,
            Err(status) => return Err(keychain_refused(status)),
        },
        None => None,
    };
    match load_in(entry, file)? {
        Some(key) => match keychain {
            Some(keychain) => move_in(keychain, entry, key).map(Some),
            None => Ok(Some(key)),
        },
        None if legacy => Ok(None),
        None => mint(keychain, entry, file).map(Some),
    }
}

/// Moves `key`, kept the old way, into the data protection keychain, then
/// overwrites the file keychain's item with [`MOVED`]: only once the key is
/// kept there, so a move that fails leaves the old item as it was. The key
/// handed back is the one the keychain keeps: another instance's, when it
/// moved the same key first.
fn move_in(
    keychain: &dyn Keychain,
    entry: Option<&keyring::Entry>,
    key: ed25519::PrivateKey,
) -> Result<ed25519::PrivateKey, String> {
    match keychain.add(&Zeroizing::new(key.encode().to_vec())) {
        Ok(()) => {}
        Err(NOT_ENTITLED) => return Ok(key),
        Err(DUPLICATE) => return decode(&keychain.copy().map_err(keychain_refused)?),
        Err(status) => {
            tracing::info!(target: "ducktape::keys", status, "data protection keychain refused the move");
            let unlock = if status == LOCKED {
                " Unlock this Mac first."
            } else {
                ""
            };
            return Err(format!(
                "This device's key couldn't move behind Touch ID, so it stays safe where it was; the move is tried again next time.{unlock}"
            ));
        }
    }
    if let Some(entry) = entry {
        entry.set_secret(MOVED).map_err(|error| {
            tracing::info!(target: "ducktape::keys", %error, "the file keychain's copy of a moved key was not marked");
            "This device's key moved behind Touch ID, but its old copy in the login keychain couldn't be marked as moved, so the key is safe in both places. Try again to open it.".to_string()
        })?;
    }
    Ok(key)
}

/// Keeps `key` in the data protection keychain, only where nothing is:
/// `false` when this build may not use it. A key already there is fine
/// when it is this one, the way the file keychain's is; telling which
/// reads it, so asks for Touch ID, which only a race comes to.
fn add(keychain: &dyn Keychain, key: &ed25519::PrivateKey) -> Result<bool, String> {
    match keychain.add(&Zeroizing::new(key.encode().to_vec())) {
        Ok(()) => Ok(true),
        Err(NOT_ENTITLED) => Ok(false),
        Err(DUPLICATE) => match decode(&keychain.copy().map_err(keychain_refused)?)? {
            kept if kept.public_key() == key.public_key() => Ok(true),
            _ => Err("this device already keeps another key for this network".into()),
        },
        Err(status) => Err(keychain_refused(status)),
    }
}

/// The key kept in the OS store, else (when the store answers that it
/// holds nothing) in the file. [`MOVED`] there is a key behind Touch ID
/// this build can't open: a refusal, never "nothing here".
fn load_in(
    entry: Option<&keyring::Entry>,
    file: &Path,
) -> Result<Option<ed25519::PrivateKey>, String> {
    if let Some(entry) = entry
        && let Some(seed) = kept(entry)?
    {
        if *seed == MOVED {
            return Err("This device's key is kept behind Touch ID by the installed Ducktape; this build can't open it.".into());
        }
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
    let stores = stores(keyring)?;
    let (keychain, entry) = split(&stores);
    save_in(keychain, entry, &file(keyring)?, key)
}

fn save_in(
    keychain: Option<&dyn Keychain>,
    entry: Option<&keyring::Entry>,
    file: &Path,
    key: &ed25519::PrivateKey,
) -> Result<(), String> {
    match load_in(entry, file)? {
        Some(kept) if kept.public_key() == key.public_key() => return Ok(()),
        Some(_) => return Err("this device already keeps another key for this network".into()),
        None => {}
    }
    if let Some(keychain) = keychain
        && add(keychain, key)?
    {
        return Ok(());
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
        .and_then(|()| noded_client::fs::atomic_write(path, seed, true))
        .map_err(|error| format!("this device's key could not be kept: {error}"))
}

/// A new key, kept before it is handed back. Only [`open_in`] makes one,
/// when nothing keeps a key; [`save_in`] keeps it only where nothing is.
fn mint(
    keychain: Option<&dyn Keychain>,
    entry: Option<&keyring::Entry>,
    file: &Path,
) -> Result<ed25519::PrivateKey, String> {
    use rand::RngCore as _;
    let mut seed = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(seed.as_mut_slice());
    let key = decode(seed.as_slice())?;
    save_in(keychain, entry, file, &key)?;
    Ok(key)
}

/// Where this device's key for `keyring` is kept, asked without Touch ID
/// (`OsServices::key_storage`).
pub(crate) fn level(keyring: &str) -> Result<KeyLevel, String> {
    match stores(keyring)? {
        Some((keychain, _)) => level_in(keychain.as_deref()),
        None => Ok(KeyLevel::File),
    }
}

/// `Keychain` only when the data protection keychain holds the item: a key
/// not moved yet is still the file keychain's.
fn level_in(keychain: Option<&dyn Keychain>) -> Result<KeyLevel, String> {
    let Some(keychain) = keychain else {
        return Ok(KeyLevel::OsKeyring);
    };
    match keychain.find() {
        Ok(()) | Err(LOCKED) => Ok(KeyLevel::Keychain),
        Err(NOT_FOUND | NOT_ENTITLED) => Ok(KeyLevel::OsKeyring),
        Err(status) => Err(keychain_refused(status)),
    }
}

/// macOS's data protection keychain, through the Security framework's item
/// calls. A Mac app reaches it only when its signature carries an
/// application identifier and this access group, which only an embedded
/// provisioning profile grants (the release's); any other build is
/// answered `errSecMissingEntitlement`.
#[cfg(target_os = "macos")]
mod keychain {
    use core_foundation::base::{CFType, TCFType as _, kCFAllocatorDefault};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::data::CFData;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::{CFString, CFStringRef};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_foundation::NSString;
    use security_framework_sys::access_control::{
        SecAccessControlCreateWithFlags, kSecAccessControlUserPresence,
        kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
    };
    use security_framework_sys::base::errSecParam;
    use security_framework_sys::item::{
        kSecAttrAccessControl, kSecAttrAccessGroup, kSecAttrAccount, kSecAttrService,
        kSecAttrSynchronizable, kSecClass, kSecClassGenericPassword, kSecReturnAttributes,
        kSecReturnData, kSecUseAuthenticationContext, kSecUseAuthenticationUI,
        kSecUseDataProtectionKeychain, kSecValueData,
    };
    use security_framework_sys::keychain_item::{SecItemAdd, SecItemCopyMatching};
    use zeroize::Zeroizing;

    /// The team that signs the release: `<team>.dev.ducktape.app` is the
    /// access group its `keychain-access-groups` entitlement names.
    const TEAM: &str = "8464KRUNQG";
    /// What the Touch ID sheet says the read is for, after "Ducktape is
    /// trying to".
    const REASON: &str = "unlock this device's key";

    // `LAContext`, whose `localizedReason` the Touch ID sheet shows
    #[link(name = "LocalAuthentication", kind = "framework")]
    unsafe extern "C" {}

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        /// `kSecUseAuthenticationUI`'s "fail rather than ask" (not in
        /// `security-framework-sys`).
        static kSecUseAuthenticationUIFail: CFStringRef;
    }

    pub(super) struct Item {
        service: String,
        account: String,
    }

    impl Item {
        pub(super) fn new(service: &str, account: String) -> Self {
            Self {
                service: service.to_owned(),
                account,
            }
        }

        /// This item's query, plus `more`: a password under this service
        /// and account, in the app's access group of the data protection
        /// keychain, never synchronized.
        fn query<const N: usize>(
            &self,
            more: [(CFStringRef, CFType); N],
        ) -> CFDictionary<CFString, CFType> {
            let group = format!("{TEAM}.{}", super::SERVICE);
            // SAFETY: each key and the class are the Security framework's
            // constant strings
            let pairs: Vec<_> = unsafe {
                [
                    (
                        kSecClass,
                        CFString::wrap_under_get_rule(kSecClassGenericPassword).into_CFType(),
                    ),
                    (kSecAttrService, CFString::new(&self.service).into_CFType()),
                    (kSecAttrAccount, CFString::new(&self.account).into_CFType()),
                    (kSecAttrAccessGroup, CFString::new(&group).into_CFType()),
                    (
                        kSecUseDataProtectionKeychain,
                        CFBoolean::true_value().into_CFType(),
                    ),
                    (
                        kSecAttrSynchronizable,
                        CFBoolean::false_value().into_CFType(),
                    ),
                ]
            }
            .into_iter()
            .chain(more)
            .map(|(key, value)| (unsafe { CFString::wrap_under_get_rule(key) }, value))
            .collect();
            CFDictionary::from_CFType_pairs(&pairs)
        }

        /// `SecItemCopyMatching`'s one answer, or its code.
        fn matching<const N: usize>(
            &self,
            more: [(CFStringRef, CFType); N],
        ) -> Result<CFType, i32> {
            let query = self.query(more);
            let mut found = std::ptr::null();
            // SAFETY: a well-formed query; `found` is ours to release
            status(unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &mut found) })?;
            if found.is_null() {
                return Err(errSecParam);
            }
            Ok(unsafe { CFType::wrap_under_create_rule(found) })
        }
    }

    impl super::Keychain for Item {
        fn copy(&self) -> Result<Zeroizing<Vec<u8>>, i32> {
            // SAFETY: the Security framework's constant keys
            let found = self.matching(unsafe {
                [
                    (kSecReturnData, CFBoolean::true_value().into_CFType()),
                    (kSecUseAuthenticationContext, reason()),
                ]
            })?;
            let seed = found.downcast_into::<CFData>().ok_or(errSecParam)?;
            Ok(Zeroizing::new(seed.bytes().to_vec()))
        }

        /// Attributes only, and told never to ask: an item that would is
        /// `errSecInteractionNotAllowed`, which is still "here".
        fn find(&self) -> Result<(), i32> {
            // SAFETY: the Security framework's constant keys and value
            self.matching(unsafe {
                [
                    (kSecReturnAttributes, CFBoolean::true_value().into_CFType()),
                    (
                        kSecUseAuthenticationUI,
                        CFString::wrap_under_get_rule(kSecUseAuthenticationUIFail).into_CFType(),
                    ),
                ]
            })
            .map(drop)
        }

        fn add(&self, seed: &[u8]) -> Result<(), i32> {
            // SAFETY: a constant protection class and flags; a null answer
            // is the failure
            let access = unsafe {
                SecAccessControlCreateWithFlags(
                    kCFAllocatorDefault,
                    kSecAttrAccessibleWhenUnlockedThisDeviceOnly.cast(),
                    kSecAccessControlUserPresence,
                    std::ptr::null_mut(),
                )
            };
            if access.is_null() {
                return Err(errSecParam);
            }
            // SAFETY: created above, ours to release; the constant keys
            let query = unsafe {
                self.query([
                    (
                        kSecAttrAccessControl,
                        CFType::wrap_under_create_rule(access.cast()),
                    ),
                    (kSecValueData, CFData::from_buffer(seed).into_CFType()),
                ])
            };
            // SAFETY: a well-formed item; no result asked for
            status(unsafe { SecItemAdd(query.as_concrete_TypeRef(), std::ptr::null_mut()) })
        }
    }

    fn status(code: i32) -> Result<(), i32> {
        match code {
            0 => Ok(()),
            code => Err(code),
        }
    }

    /// An authentication context whose Touch ID sheet gives [`REASON`].
    fn reason() -> CFType {
        // SAFETY: `LAContext` is linked above; `new` and the setter are its own
        let context: Retained<AnyObject> = unsafe { msg_send_id![class!(LAContext), new] };
        let reason = NSString::from_str(REASON);
        let () = unsafe { msg_send![&*context, setLocalizedReason: &*reason] };
        // SAFETY: an Objective-C object is a CF type; CF retains it here
        unsafe { CFType::wrap_under_get_rule(Retained::as_ptr(&context).cast()) }
    }
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
            let sentence = open_in(None, Some(&entry), &file, false).expect_err(&shown);
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
        assert!(open_in(None, Some(&entry), &file, true).unwrap().is_none());
        assert!(
            held(&entry).is_none(),
            "made one beside a password-locked key"
        );
        let made = open_in(None, Some(&entry), &file, false).unwrap().unwrap();
        assert_eq!(held(&entry).as_deref(), Some(made.encode().as_ref()));
        let again = open_in(None, Some(&entry), &file, false).unwrap().unwrap();
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
        assert!(save_in(None, Some(&entry), &file, &other).is_err());
        assert_eq!(held(&entry).as_deref(), Some(account.encode().as_ref()));
        save_in(None, Some(&entry), &file, &account).unwrap();
        let refusing = store(
            Some(&account),
            Some(keyring::Error::NoStorageAccess(platform("locked"))),
        );
        assert!(save_in(None, Some(&refusing), &file, &other).is_err());
        assert_eq!(held(&refusing).as_deref(), Some(account.encode().as_ref()));
        save_file(&file, &account.encode()).unwrap();
        assert!(save_in(None, None, &file, &other).is_err());
        assert!(save_in(None, Some(&store(None, None)), &file, &other).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), account.encode().as_ref());
        // the data protection keychain the same way: the file keychain's key
        // is asked first, and the keychain's own is kept again but never
        // replaced
        let nothing = dir.path().join("nothing.key");
        let keychain = Fake::default();
        assert!(save_in(Some(&keychain), Some(&entry), &nothing, &other).is_err());
        assert!(keychain.held().is_none(), "kept beside the file keychain's");
        let (keychain, empty) = (Fake::holding(&account), store(None, None));
        save_in(Some(&keychain), Some(&empty), &nothing, &account).unwrap();
        assert!(save_in(Some(&keychain), Some(&empty), &nothing, &other).is_err());
        assert_eq!(keychain.held().as_deref(), Some(account.encode().as_ref()));
        assert!(held(&empty).is_none());
    }

    /// A data protection keychain holding `held`: the next read answers
    /// `read` when set, every add answers `add` when set; else it keeps a
    /// seed only where none is, as the real one does. `asked` counts reads
    /// and adds.
    #[derive(Default)]
    struct Fake {
        held: std::cell::RefCell<Option<Vec<u8>>>,
        read: std::cell::Cell<Option<i32>>,
        add: Option<i32>,
        asked: std::cell::Cell<u32>,
    }

    impl Fake {
        fn holding(key: &ed25519::PrivateKey) -> Self {
            let fake = Self::default();
            fake.held.replace(Some(key.encode().to_vec()));
            fake
        }

        fn held(&self) -> Option<Vec<u8>> {
            self.held.borrow().clone()
        }
    }

    impl Keychain for Fake {
        fn copy(&self) -> Result<Zeroizing<Vec<u8>>, i32> {
            self.asked.set(self.asked.get() + 1);
            match self.read.take() {
                Some(status) => Err(status),
                None => self.held().map(Zeroizing::new).ok_or(NOT_FOUND),
            }
        }

        fn find(&self) -> Result<(), i32> {
            self.copy().map(drop)
        }

        fn add(&self, seed: &[u8]) -> Result<(), i32> {
            self.asked.set(self.asked.get() + 1);
            match (self.add, self.held()) {
                (Some(status), _) => Err(status),
                (None, Some(_)) => Err(DUPLICATE),
                (None, None) => {
                    self.held.replace(Some(seed.to_vec()));
                    Ok(())
                }
            }
        }
    }

    /// The data protection keychain's key is the key: read first, over what
    /// the file keychain holds, and nothing is written anywhere.
    #[test]
    fn a_found_key_is_read_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let (account, other) = (decode(&[3u8; 32]).unwrap(), decode(&[4u8; 32]).unwrap());
        let keychain = Fake::holding(&account);
        let entry = store(Some(&other), None);
        let opened = open_in(Some(&keychain), Some(&entry), &file, false)
            .unwrap()
            .unwrap();
        assert_eq!(opened.public_key(), account.public_key());
        assert_eq!(keychain.held().as_deref(), Some(account.encode().as_ref()));
        assert_eq!(held(&entry).as_deref(), Some(other.encode().as_ref()));
        assert!(!file.exists());
    }

    /// The file keychain's key from an earlier build moves into the data
    /// protection keychain, the same key, and only then is the file
    /// keychain's item overwritten with `MOVED`.
    #[test]
    fn a_file_keychain_key_moves_once_and_stays_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let account = decode(&[3u8; 32]).unwrap();
        let keychain = Fake::default();
        let entry = store(Some(&account), None);
        let moved = open_in(Some(&keychain), Some(&entry), &file, false)
            .unwrap()
            .unwrap();
        assert_eq!(moved.public_key(), account.public_key());
        assert_eq!(keychain.held().as_deref(), Some(account.encode().as_ref()));
        assert_eq!(held(&entry).as_deref(), Some(MOVED));
        let again = open_in(Some(&keychain), Some(&entry), &file, false)
            .unwrap()
            .unwrap();
        assert_eq!(again.public_key(), account.public_key());
    }

    /// A move the data protection keychain refuses (here a locked Mac) is
    /// the key screen's sentence: the file keychain's key stays where it
    /// was, and the sentence says so; nothing is made.
    #[test]
    fn a_failed_move_leaves_the_file_keychain_key_and_mints_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let account = decode(&[3u8; 32]).unwrap();
        let keychain = Fake {
            add: Some(LOCKED),
            ..Fake::default()
        };
        let entry = store(Some(&account), None);
        let sentence = open_in(Some(&keychain), Some(&entry), &file, false).unwrap_err();
        assert!(sentence.contains("safe where it was"), "{sentence}");
        assert!(sentence.contains("Unlock this Mac"), "{sentence}");
        assert_eq!(held(&entry).as_deref(), Some(account.encode().as_ref()));
        assert!(keychain.held().is_none());
        assert!(!file.exists());
    }

    /// A build the data protection keychain refuses (no entitlement: an
    /// ad-hoc, local or unbundled build, at the read or only at the add)
    /// opens, makes and keeps its key in the file keychain, as before.
    #[test]
    fn a_build_without_the_entitlement_uses_the_file_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let (account, other) = (decode(&[3u8; 32]).unwrap(), decode(&[4u8; 32]).unwrap());
        let unentitled = || {
            [
                Fake {
                    read: Some(NOT_ENTITLED).into(),
                    add: Some(NOT_ENTITLED),
                    ..Fake::default()
                },
                Fake {
                    add: Some(NOT_ENTITLED),
                    ..Fake::default()
                },
            ]
        };
        for keychain in unentitled() {
            let entry = store(Some(&account), None);
            let opened = open_in(Some(&keychain), Some(&entry), &file, false)
                .unwrap()
                .unwrap();
            assert_eq!(opened.public_key(), account.public_key());
            assert_eq!(held(&entry).as_deref(), Some(account.encode().as_ref()));
            assert!(keychain.held().is_none());
            assert!(keychain.asked.get() > 0, "the keychain was never asked");
        }
        for keychain in unentitled() {
            let entry = store(None, None);
            let made = open_in(Some(&keychain), Some(&entry), &file, false)
                .unwrap()
                .unwrap();
            assert_eq!(held(&entry).as_deref(), Some(made.encode().as_ref()));
            assert!(keychain.held().is_none());
            let entry = store(None, None);
            let asked = keychain.asked.get();
            save_in(Some(&keychain), Some(&entry), &file, &other).unwrap();
            assert_eq!(held(&entry).as_deref(), Some(other.encode().as_ref()));
            assert!(
                asked > 0 && keychain.asked.get() > asked,
                "the keychain was never asked"
            );
        }
        assert!(!file.exists());
    }

    /// Touch ID cancelled or not matched, or this Mac locked: the key
    /// screen's sentence, and nothing is made or written over the key the
    /// keychain holds.
    #[test]
    fn touch_id_refused_never_mints_and_never_writes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let account = decode(&[3u8; 32]).unwrap();
        let refusals = [
            (CANCELLED, "cancelled"),
            (AUTH_FAILED, "didn't match"),
            (LOCKED, "This Mac is locked"),
        ];
        for (status, says) in refusals {
            let keychain = Fake {
                read: Some(status).into(),
                ..Fake::holding(&account)
            };
            let entry = store(None, None);
            let sentence = open_in(Some(&keychain), Some(&entry), &file, false).unwrap_err();
            assert!(sentence.contains(says), "{status}: {sentence}");
            assert_eq!(keychain.held().as_deref(), Some(account.encode().as_ref()));
            assert!(
                held(&entry).is_none(),
                "{status}: a key went to the file keychain"
            );
            assert!(!file.exists(), "{status}: a key went to the file");
        }
    }

    /// Nothing anywhere: a key is made once, kept in the data protection
    /// keychain alone, and the next open is that key; a password-locked key
    /// waiting makes none, and a key handed over later is not kept over it.
    #[test]
    fn nothing_anywhere_mints_once_into_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let keychain = Fake::default();
        let entry = store(None, None);
        assert!(
            open_in(Some(&keychain), Some(&entry), &file, true)
                .unwrap()
                .is_none()
        );
        assert!(
            keychain.held().is_none(),
            "made one beside a password-locked key"
        );
        let made = open_in(Some(&keychain), Some(&entry), &file, false)
            .unwrap()
            .unwrap();
        assert_eq!(keychain.held().as_deref(), Some(made.encode().as_ref()));
        let again = open_in(Some(&keychain), Some(&entry), &file, false)
            .unwrap()
            .unwrap();
        assert_eq!(made.public_key(), again.public_key());
        let other = decode(&[4u8; 32]).unwrap();
        assert!(save_in(Some(&keychain), Some(&entry), &file, &other).is_err());
        assert_eq!(keychain.held().as_deref(), Some(made.encode().as_ref()));
        assert!(held(&entry).is_none());
        assert!(!file.exists());
    }

    /// Two instances moving the same key at once: the one whose add finds
    /// the other's item reads that key instead of refusing, and leaves the
    /// file keychain to the one that moved it.
    #[test]
    fn a_move_that_races_reads_the_moved_key() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let account = decode(&[3u8; 32]).unwrap();
        let keychain = Fake {
            read: Some(NOT_FOUND).into(),
            ..Fake::holding(&account)
        };
        let entry = store(Some(&account), None);
        let opened = open_in(Some(&keychain), Some(&entry), &file, false)
            .unwrap()
            .unwrap();
        assert_eq!(opened.public_key(), account.public_key());
        assert_eq!(held(&entry).as_deref(), Some(account.encode().as_ref()));
        assert!(!file.exists());
    }

    /// After a move the file keychain holds `MOVED`, never a key: a build
    /// that can't reach the data protection keychain refuses to open or
    /// keep one there and makes nothing, and a build from before (its
    /// `decode`) reads a damaged key.
    #[test]
    fn a_moved_key_is_never_replaced_by_a_build_that_cannot_open_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let other = decode(&[4u8; 32]).unwrap();
        assert!(decode(MOVED).is_err());
        let unentitled = Fake {
            read: Some(NOT_ENTITLED).into(),
            add: Some(NOT_ENTITLED),
            ..Fake::default()
        };
        let entry = store(None, None);
        entry.set_secret(MOVED).unwrap();
        let sentence = open_in(Some(&unentitled), Some(&entry), &file, false).unwrap_err();
        assert!(sentence.contains("behind Touch ID"), "{sentence}");
        assert!(save_in(Some(&unentitled), Some(&entry), &file, &other).is_err());
        assert_eq!(held(&entry).as_deref(), Some(MOVED));
        assert!(unentitled.held().is_none());
        assert!(!file.exists());
    }

    /// Key storage names the data protection keychain only once the key is
    /// there (an item that would ask to be read is there too), else the
    /// file keychain, where an unmoved key still is.
    #[test]
    fn key_storage_names_the_keychain_only_once_the_key_is_there() {
        let account = decode(&[3u8; 32]).unwrap();
        let level = |keychain: Fake| level_in(Some(&keychain));
        assert_eq!(level(Fake::holding(&account)), Ok(KeyLevel::Keychain));
        let locked = Fake {
            read: Some(LOCKED).into(),
            ..Fake::holding(&account)
        };
        assert_eq!(level(locked), Ok(KeyLevel::Keychain));
        assert_eq!(level(Fake::default()), Ok(KeyLevel::OsKeyring));
        let unentitled = Fake {
            read: Some(NOT_ENTITLED).into(),
            ..Fake::default()
        };
        assert_eq!(level(unentitled), Ok(KeyLevel::OsKeyring));
        assert_eq!(level_in(None), Ok(KeyLevel::OsKeyring));
    }

    /// The real keychains, under a test service: a test binary carries no
    /// keychain entitlement, so the data protection keychain refuses it and
    /// its key goes to the file keychain, as an ad-hoc build's does. It
    /// writes this Mac's login keychain (and deletes what it wrote), so it
    /// runs by hand: `cargo test -p ducktape-app --bin ducktape-app
    /// device_key -- --ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "writes this Mac's login keychain"]
    fn an_unentitled_build_keeps_its_key_in_the_file_keychain() {
        let service = "ducktape-device-key-probe";
        let account = format!("probe:{}", rand::random::<u64>());
        let item = keychain::Item::new(service, account.clone());
        assert_eq!(item.copy().map(drop), Err(NOT_ENTITLED));
        assert_eq!(item.find(), Err(NOT_ENTITLED));
        let entry = keyring::Entry::new(service, &account).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("device.key");
        let made = open_in(Some(&item), Some(&entry), &file, false);
        let again = open_in(Some(&item), Some(&entry), &file, false);
        let in_file_keychain = kept(&entry);
        entry.delete_credential().unwrap();
        let made = made.unwrap().unwrap();
        assert_eq!(again.unwrap().unwrap().public_key(), made.public_key());
        assert_eq!(
            in_file_keychain.unwrap().as_deref().map(Vec::as_slice),
            Some(made.encode().as_ref())
        );
        assert!(!file.exists());
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
