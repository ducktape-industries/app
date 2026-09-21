//! This device: its signing key, opened from the keystore and held here
//! while the person is signed in; the frames it signs; and the small
//! preferences file. Every write a view submits is signed here — a view
//! never sees the key, a password or an endpoint.

use std::path::PathBuf;

use commonware_cryptography::{Signer as _, ed25519};
use view_wire::Refusal;
use zeroize::Zeroizing;

use super::noded::{Frame, Layer};
use super::{RpcClient, hex_encode, refused};

/// The host namespace that holds each signer's next sequence.
const SIGNERS: &str = "$signers";

struct Signer {
    key: ed25519::PrivateKey,
}

static SIGNER: tokio::sync::Mutex<Option<Signer>> = tokio::sync::Mutex::const_new(None);

pub(crate) const LOCKED: &str = "the local user key is locked; enter its password";

pub(crate) fn locked_seat() -> Refusal {
    Refusal::new("session_locked", LOCKED)
}

/// Opens the key file with `password` and seats it; answers the public key.
pub(crate) async fn seat_signer(
    path: PathBuf,
    password: Zeroizing<String>,
) -> Result<String, String> {
    if password.is_empty() {
        return Err(LOCKED.into());
    }
    let key =
        tokio::task::spawn_blocking(move || keystore::userkey::open_user_key_at(&path, &password))
            .await
            .map_err(|_| "opening this device's key did not finish".to_string())??;
    let pubkey = hex_encode(key.public_key().as_ref());
    *SIGNER.lock().await = Some(Signer { key });
    Ok(pubkey)
}

pub(crate) async fn lock_signer() -> bool {
    SIGNER.lock().await.take().is_some()
}

/// A write: signed with the seated key at the sequence the node says is
/// that signer's next.
pub(crate) async fn seated_frame(
    client: &RpcClient,
    network: &str,
    target: &str,
    payload: Vec<u8>,
) -> Result<Vec<u8>, Refusal> {
    let session = SIGNER.lock().await;
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    let pubkey = signer.key.public_key().as_ref().to_vec();
    let seq = client
        .get(Layer::Preconfirmed, SIGNERS, &pubkey)
        .await
        .map_err(refused)?
        .map(|bytes| abi::decode::<u64>(&bytes))
        .transpose()
        .map_err(|refusal| Refusal::new("malformed_reply", refusal.sentence))?
        .unwrap_or(0);
    Ok(Frame::sign(&signer.key, network.as_bytes(), seq, target, payload).encode())
}

/// A read: a query still travels as a signed frame (the program hears who
/// asks), but the node checks no sequence on it. Signed with the seated
/// key, or with this process's reader key while nobody is signed in.
pub(crate) async fn query_frame(network: &str, target: &str, payload: Vec<u8>) -> Vec<u8> {
    let session = SIGNER.lock().await;
    let key = match session.as_ref() {
        Some(signer) => &signer.key,
        None => reader_key(),
    };
    Frame::sign(key, network.as_bytes(), 0, target, payload).encode()
}

fn reader_key() -> &'static ed25519::PrivateKey {
    static KEY: std::sync::OnceLock<ed25519::PrivateKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        use commonware_codec::DecodeExt as _;
        use rand::RngCore as _;
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        ed25519::PrivateKey::decode(seed.as_slice()).expect("32 random bytes decode")
    })
}

// ---------- keys on disk ----------

/// Where this device keeps its wallets for `network`: one directory per
/// network under the ducktape home.
pub(crate) fn keystore_root(network: &str) -> Result<PathBuf, String> {
    let name: String = network
        .chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                true => c,
                false => '_',
            },
        )
        .collect();
    if name.is_empty() {
        return Err("the node named no network".into());
    }
    Ok(ducktape_home::root()?.join("remotes").join(name))
}

/// The key file a sign-in opens: `DUCKTAPE_USER_KEY`, else the network's
/// active wallet.
pub(crate) fn session_key_path(network: &str) -> Result<PathBuf, String> {
    if let Some(path) = keystore::wallet::env_user_key() {
        return Ok(path);
    }
    keystore::wallet::active_user_key(&keystore_root(network)?)
}

/// Mints a wallet named `name` for `network` under `password`, makes it the
/// active one, and answers its recovery phrase.
pub(crate) fn create_wallet(network: &str, name: &str, password: &str) -> Result<String, String> {
    let root = keystore_root(network)?;
    keystore::wallet::valid_name(name)?;
    let path = keystore::wallet::key_file(&root, name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let (phrase, _) = keystore::userkey::mint_user_key(&path, password)?;
    keystore::wallet::set_active(&root, name)?;
    Ok(phrase)
}

// ---------- preferences ----------

fn prefs_path() -> Option<PathBuf> {
    super::config_dir().ok().map(|dir| dir.join("prefs.json"))
}

pub(crate) fn read_prefs() -> serde_json::Value {
    let Some(path) = prefs_path() else {
        return serde_json::json!({});
    };
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

pub(crate) fn write_prefs(prefs: &serde_json::Value) -> bool {
    let Some(path) = prefs_path() else {
        return false;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(bytes) = serde_json::to_vec_pretty(prefs) else {
        return false;
    };
    std::fs::write(&path, bytes).is_ok()
}

pub(crate) fn load_appearance() -> crate::Appearance {
    match read_prefs()["appearance"].as_str() {
        Some("light") => crate::Appearance::Light,
        Some("dark") => crate::Appearance::Dark,
        _ => crate::Appearance::System,
    }
}

pub(crate) fn save_appearance(mode: crate::Appearance) -> bool {
    let mut prefs = read_prefs();
    match mode {
        crate::Appearance::System => {
            if let Some(prefs) = prefs.as_object_mut() {
                prefs.remove("appearance");
            }
        }
        crate::Appearance::Light => prefs["appearance"] = serde_json::json!("light"),
        crate::Appearance::Dark => prefs["appearance"] = serde_json::json!("dark"),
    }
    write_prefs(&prefs)
}

pub(crate) const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8844";

pub(crate) const ENDPOINT_REFUSAL: &str =
    "A node URL is http:// or https:// followed by a host and an optional port, and nothing else.";

/// The origin of a node URL, or `None` for anything that is not one.
pub(crate) fn endpoint_origin(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url.trim()).ok()?;
    let origin = matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.path(), "" | "/");
    origin.then(|| url.as_str().trim_end_matches('/').to_string())
}

/// Node URLs this device connected to, most recent first.
pub(crate) fn recent_endpoints() -> Vec<String> {
    read_prefs()["endpoints"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn note_endpoint(endpoint: &str) {
    let mut recent = recent_endpoints();
    recent.retain(|known| known != endpoint);
    recent.insert(0, endpoint.to_owned());
    recent.truncate(8);
    let mut prefs = read_prefs();
    prefs["endpoints"] = serde_json::json!(recent);
    write_prefs(&prefs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_is_an_origin_and_nothing_else() {
        assert_eq!(
            endpoint_origin(" http://127.0.0.1:8844/ ").as_deref(),
            Some("http://127.0.0.1:8844")
        );
        assert_eq!(endpoint_origin("http://a:b@host"), None);
        assert_eq!(endpoint_origin("http://host/v1"), None);
        assert_eq!(endpoint_origin("ftp://host"), None);
    }

    #[tokio::test]
    async fn a_query_frame_names_the_target_and_the_network() {
        let frame = query_frame("net", "demo", vec![1]).await;
        let decoded: Frame = abi::decode(&frame).unwrap();
        assert_eq!(decoded.body.target, "demo");
        assert_eq!(decoded.body.network, b"net");
        assert_eq!(decoded.body.seq, 0);
    }
}
