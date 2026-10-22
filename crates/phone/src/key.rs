//! This device's key, held by the OS key store the Expo build used: the
//! Secure Enclave and the Keychain through `DeviceKeyStore.swift` over the C
//! ABI ([`ios`]), Android Keystore through `DeviceKeyStore.kt` over JNI
//! ([`android`]). Rust asks the store for a public key or a signature; the
//! key never leaves the store's side.
//!
//! Ported from the Expo core (mobile `crates/mobile-core/src/key.rs` and
//! `key/{ios,android}.rs`, mobile dev 6f23151): the same aliases, so the
//! update from the Expo build to this one finds the same key (wasm-mobile
//! SPEC §4.8).

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "android")]
use android as store;
#[cfg(target_os = "ios")]
use ios as store;

use ducktape_host::contract::KeyLevel;
#[cfg(any(target_os = "ios", target_os = "android"))]
use ducktape_host::contract::KeyStorage;

/// What went wrong asking the key store, as a sentence.
pub type KeyResult<T> = Result<T, String>;

/// The store's alias of this device's key for `keyring` (the directory
/// `noded_client::keyring::bind` names): the Expo build's
/// `ducktape:device-key:<keyring>` (mobile `key.rs:41-42`), letter for
/// letter, or the update loses the key.
pub fn alias(keyring: &str) -> String {
    format!("ducktape:device-key:{keyring}")
}

/// This device's key for one keyring: the store's alias and its public
/// half, read once.
#[cfg(any(target_os = "ios", target_os = "android"))]
pub struct DeviceKey {
    alias: String,
    public: [u8; 32],
}

#[cfg(any(target_os = "ios", target_os = "android"))]
impl DeviceKey {
    /// The key for `keyring`, made in the store on first ask.
    pub fn open(keyring: &str) -> KeyResult<Self> {
        let alias = alias(keyring);
        Ok(Self {
            public: store::public_key(&alias)?,
            alias,
        })
    }
}

#[cfg(any(target_os = "ios", target_os = "android"))]
impl noded_client::Signer for DeviceKey {
    fn public_key(&self) -> [u8; 32] {
        self.public
    }

    fn sign<'a>(
        &'a self,
        namespace: &'a [u8],
        message: &'a [u8],
    ) -> rt::Fut<'a, Result<[u8; 64], noded_client::SignError>> {
        let alias = self.alias.clone();
        let message = commonware_utils::union_unique(namespace, message);
        Box::pin(async move {
            rt::task::blocking(move || store::sign(&alias, &message))
                .await
                .map_err(|_| noded_client::SignError("the key store's work did not finish".into()))?
                .map_err(noded_client::SignError)
        })
    }
}

/// Where the key for `keyring` is kept, in the store's words.
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn storage(keyring: &str) -> KeyResult<KeyStorage> {
    let (sealed, level) = store::storage(&alias(keyring))?;
    Ok(KeyStorage {
        sealed,
        level: level_of(&level)?,
    })
}

/// What a Java exception from the key store says to a person: its message,
/// or, where it carries none (AndroidKeyStore's `AEADBadTagException`), its
/// class.
pub fn java_refusal(class: &str, message: &str) -> String {
    let unsaid = message.is_empty() || message == "<NULL>";
    match unsaid {
        true => {
            let short = class.rsplit(['.', '/']).next().unwrap_or(class);
            format!("The key store refused ({short}).")
        }
        false => message.to_owned(),
    }
}

/// A store's level word as the host names it. The Swift store answers
/// `secure-enclave` or `keychain`; the Kotlin one `strongbox`, `tee`,
/// `secure` (secure hardware, which one not said), `software` or `unknown`.
/// The last two kinds of answer have no level of their own in the contract
/// and are said as they are rather than guessed.
pub fn level_of(word: &str) -> KeyResult<KeyLevel> {
    match word {
        "secure-enclave" => Ok(KeyLevel::SecureEnclave),
        "keychain" => Ok(KeyLevel::Keychain),
        "strongbox" => Ok(KeyLevel::KeystoreStrongBox),
        "tee" => Ok(KeyLevel::KeystoreTee),
        "software" => Ok(KeyLevel::KeystoreSoftware),
        other => Err(format!(
            "The key store names where this key is kept as `{other}`, which this app does not read yet."
        )),
    }
}

/// A view's secrets (`contract::Secrets`) in the store that keeps this
/// device's key: the Keychain on iOS, a file sealed by a Keystore key on
/// Android.
#[cfg(any(target_os = "ios", target_os = "android"))]
pub struct PhoneSecrets;

#[cfg(any(target_os = "ios", target_os = "android"))]
impl ducktape_host::contract::Secrets for PhoneSecrets {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, ducktape_host::contract::KeyRefusal> {
        store::secret_get(name).map_err(ducktape_host::contract::KeyRefusal::Denied)
    }

    fn set(
        &self,
        name: &str,
        value: Option<&[u8]>,
    ) -> Result<(), ducktape_host::contract::KeyRefusal> {
        let done = match value {
            Some(value) => store::secret_set(name, value),
            None => store::secret_remove(name),
        };
        done.map_err(ducktape_host::contract::KeyRefusal::Denied)
    }
}

/// The key a query is signed with while no key is seated
/// (`OsServices::reader`): a throwaway made per process, as the desktop's,
/// so a read is never linkable to this device.
pub struct Reader(ring::signature::Ed25519KeyPair);

impl Reader {
    /// A fresh key; `None` when the OS gives no randomness.
    pub fn new() -> Option<Self> {
        let random = ring::rand::SystemRandom::new();
        let document = ring::signature::Ed25519KeyPair::generate_pkcs8(&random).ok()?;
        ring::signature::Ed25519KeyPair::from_pkcs8(document.as_ref())
            .ok()
            .map(Self)
    }
}

impl noded_client::Signer for Reader {
    fn public_key(&self) -> [u8; 32] {
        use ring::signature::KeyPair as _;
        let mut public = [0; 32];
        public.copy_from_slice(self.0.public_key().as_ref());
        public
    }

    fn sign<'a>(
        &'a self,
        namespace: &'a [u8],
        message: &'a [u8],
    ) -> rt::Fut<'a, Result<[u8; 64], noded_client::SignError>> {
        let signature = self
            .0
            .sign(&commonware_utils::union_unique(namespace, message));
        Box::pin(std::future::ready(signature.as_ref().try_into().map_err(
            |_| {
                noded_client::SignError("Ed25519 answered a signature that is not 64 bytes.".into())
            },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noded_client::Signer as _;

    #[test]
    fn the_alias_and_its_keyring_are_the_expo_builds() {
        let documents = tempfile::tempdir().unwrap();
        let remotes = crate::os::dirs(documents.path(), documents.path()).remotes;
        assert_eq!(remotes, documents.path().join("remotes"));
        let first =
            noded_client::keyring::bind(&noded_client::fs::Native, &remotes, "testkit", 1).unwrap();
        let again =
            noded_client::keyring::bind(&noded_client::fs::Native, &remotes, "testkit", 1).unwrap();
        let other =
            noded_client::keyring::bind(&noded_client::fs::Native, &remotes, "testkit", 2).unwrap();
        assert_eq!(alias(&first.dir), "ducktape:device-key:testkit");
        assert_eq!(alias(&again.dir), alias(&first.dir));
        assert_eq!(alias(&other.dir), "ducktape:device-key:testkit+2");
    }

    #[test]
    fn every_store_word_reads_as_a_level_or_a_sentence() {
        for (word, level) in [
            ("secure-enclave", KeyLevel::SecureEnclave),
            ("keychain", KeyLevel::Keychain),
            ("strongbox", KeyLevel::KeystoreStrongBox),
            ("tee", KeyLevel::KeystoreTee),
            ("software", KeyLevel::KeystoreSoftware),
        ] {
            assert_eq!(level_of(word), Ok(level));
        }
        assert!(level_of("secure").unwrap_err().contains("`secure`"));
    }

    #[test]
    fn a_java_refusal_with_no_message_names_its_class() {
        assert_eq!(
            java_refusal("javax.crypto.AEADBadTagException", "<NULL>"),
            "The key store refused (AEADBadTagException)."
        );
        assert_eq!(
            java_refusal("javax/crypto/AEADBadTagException", ""),
            "The key store refused (AEADBadTagException)."
        );
        assert_eq!(
            java_refusal(
                "java.lang.IllegalStateException",
                "could not open the secret (AEADBadTagException)"
            ),
            "could not open the secret (AEADBadTagException)"
        );
    }

    #[test]
    fn the_reader_signs_the_namespaced_message() {
        let reader = Reader::new().unwrap();
        let signature = futures::executor::block_on(reader.sign(b"ducktape", b"query")).unwrap();
        let public =
            ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, reader.public_key());
        public
            .verify(
                &commonware_utils::union_unique(b"ducktape", b"query"),
                &signature,
            )
            .unwrap();
        assert!(public.verify(b"query", &signature).is_err());
    }
}
