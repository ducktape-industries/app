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

const MOVED: &[u8] = b"moved to the data protection keychain";

const NOT_FOUND: i32 = -25300;
const DUPLICATE: i32 = -25299;
const NOT_ENTITLED: i32 = -34018;
const CANCELLED: i32 = -128;
const AUTH_FAILED: i32 = -25293;
const LOCKED: i32 = -25308;

pub(crate) fn file_only() -> bool {
    cfg!(test) || std::env::var("DUCKTAPE_DEVICE_KEY_STORE").is_ok_and(|store| store == "file")
}

trait Keychain {
    fn copy(&self) -> Result<Zeroizing<Vec<u8>>, i32>;
    fn find(&self) -> Result<(), i32>;
    fn add(&self, seed: &[u8]) -> Result<(), i32>;
}

type Stores = Option<(Option<Box<dyn Keychain>>, keyring::Entry)>;

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

fn split(stores: &Stores) -> (Option<&dyn Keychain>, Option<&keyring::Entry>) {
    stores.as_ref().map_or((None, None), |(keychain, entry)| {
        (keychain.as_deref(), Some(entry))
    })
}

fn kept(entry: &keyring::Entry) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    match entry.get_secret() {
        Ok(seed) => Ok(Some(Zeroizing::new(seed))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(refused(error)),
    }
}

pub(crate) fn refused(error: keyring::Error) -> String {
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

pub(crate) fn open(keyring: &str, legacy: bool) -> Result<Option<ed25519::PrivateKey>, String> {
    #[cfg(test)]
    tests::opened_on(keyring);
    let stores = stores(keyring)?;
    let (keychain, entry) = split(&stores);
    open_in(keychain, entry, &file(keyring)?, legacy)
}

pub(crate) fn open_kept(keyring: &str) -> Result<Option<ed25519::PrivateKey>, String> {
    let stores = stores(keyring)?;
    let (keychain, entry) = split(&stores);
    open_in(keychain, entry, &file(keyring)?, true)
}

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

pub(crate) fn level(keyring: &str) -> Result<KeyLevel, String> {
    match stores(keyring)? {
        Some((keychain, _)) => level_in(keychain.as_deref()),
        None => Ok(KeyLevel::File),
    }
}

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

    const RELEASE_SIGNING_TEAM: &str = "8464KRUNQG";
    const KEYCHAIN_READ_REASON: &str = "unlock this device's key";

    #[link(name = "LocalAuthentication", kind = "framework")]
    unsafe extern "C" {}

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
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

        fn query<const N: usize>(
            &self,
            more: [(CFStringRef, CFType); N],
        ) -> CFDictionary<CFString, CFType> {
            let group = format!("{RELEASE_SIGNING_TEAM}.{}", super::SERVICE);
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
                    (
                        kSecUseAuthenticationContext,
                        touch_id_context_with_read_reason(),
                    ),
                ]
            })?;
            let seed = found.downcast_into::<CFData>().ok_or(errSecParam)?;
            Ok(Zeroizing::new(seed.bytes().to_vec()))
        }

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

    fn touch_id_context_with_read_reason() -> CFType {
        // SAFETY: `LAContext` is linked above; `new` and the setter are its own
        let context: Retained<AnyObject> = unsafe { msg_send_id![class!(LAContext), new] };
        let reason = NSString::from_str(KEYCHAIN_READ_REASON);
        let () = unsafe { msg_send![&*context, setLocalizedReason: &*reason] };
        // SAFETY: an Objective-C object is a CF type; CF retains it here
        unsafe { CFType::wrap_under_get_rule(Retained::as_ptr(&context).cast()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static OPENED: std::sync::Mutex<Vec<(String, std::thread::ThreadId)>> =
        std::sync::Mutex::new(Vec::new());

    pub(super) fn opened_on(keyring: &str) {
        OPENED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((keyring.to_owned(), std::thread::current().id()));
    }

    #[test]
    fn the_desktop_opens_its_key_off_the_thread_that_awaits_it() {
        use ducktape_host::contract::OsServices as _;
        let keyring = "opened-off-the-awaiting-thread";
        let os = crate::os::DesktopOs::new().unwrap();
        let caller = std::thread::current().id();
        let opened = ducktape_host::runtime::handle().block_on(os.device_key(keyring));
        assert!(matches!(opened, Ok(Some(_))), "no key opened");
        let on: Vec<_> = OPENED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(ring, _)| ring == keyring)
            .map(|(_, on)| *on)
            .collect();
        assert!(!on.is_empty(), "the store was never opened");
        assert!(
            !on.contains(&caller),
            "the store was opened on the awaiting thread"
        );
    }

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
