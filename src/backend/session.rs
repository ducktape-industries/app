//! This device: its signing key, opened from the keystore and held here
//! while the person is signed in; the frames it signs; and the small
//! preferences file. Every write a view submits is signed here — a view
//! never sees the private key or a password.

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
    let seq = next_seq(client, signer.key.public_key().as_ref()).await?;
    Ok(Frame::sign(&signer.key, network.as_bytes(), seq, target, payload).encode())
}

/// The sequence the node expects next from `signer`.
pub(crate) async fn next_seq(client: &RpcClient, signer: &[u8]) -> Result<u64, Refusal> {
    Ok(client
        .get(Layer::Preconfirmed, SIGNERS, signer)
        .await
        .map_err(refused)?
        .map(|bytes| abi::decode::<u64>(&bytes))
        .transpose()
        .map_err(|refusal| Refusal::new("malformed_reply", refusal.sentence))?
        .unwrap_or(0))
}

/// The seated key's public half.
pub(crate) async fn seated_key() -> Result<Vec<u8>, Refusal> {
    let session = SIGNER.lock().await;
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    Ok(signer.key.public_key().as_ref().to_vec())
}

/// The seated key's public half and its signature over `message` under
/// `namespace` — a consent the device key gives (`backend::passkey`).
pub(crate) async fn seated_sign(
    namespace: &[u8],
    message: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), Refusal> {
    let session = SIGNER.lock().await;
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    Ok((
        signer.key.public_key().as_ref().to_vec(),
        signer.key.sign(namespace, message).as_ref().to_vec(),
    ))
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

/// Whether this device already holds a signing key for `network` — the
/// question the sign-in screen answers before offering Unlock or Create.
pub(crate) fn key_exists(network: &str) -> bool {
    let Ok(path) = session_key_path(network) else {
        return false;
    };
    !matches!(
        keystore::userkey::key_file_state(&path),
        keystore::userkey::KeyFileState::Absent
    )
}

/// Restores a wallet from its 24-word phrase for `network`, under
/// `password`, and makes it the active key. Never overwrites a key that is
/// already active: if this device already has one, the phrase lands under a
/// fresh wallet name instead, and THAT becomes active.
pub(crate) fn restore_wallet(network: &str, mnemonic: &str, password: &str) -> Result<(), String> {
    let root = keystore_root(network)?;
    let had_one = keystore::wallet::active_name(&root).is_some();
    let name = match had_one {
        false => "default".to_string(),
        true => restore_wallet_name(&root),
    };
    keystore::wallet::import(&root, &name, mnemonic, password)?;
    if had_one {
        keystore::wallet::activate(&root, &name)?;
    }
    Ok(())
}

/// The first unused `restored`, `restored-2`, … name in `root` — so a
/// restore next to an existing key never collides with it.
fn restore_wallet_name(root: &std::path::Path) -> String {
    (1..)
        .map(|n| match n {
            1 => "restored".to_string(),
            n => format!("restored-{n}"),
        })
        .find(|name| !keystore::wallet::key_file(root, name).exists())
        .expect("an unbounded search finds an unused name")
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

/// A node URL this device connected to, and the network name it reported —
/// empty for an entry saved before the app kept names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecentEndpoint {
    pub(crate) url: String,
    pub(crate) network: String,
}

/// Node URLs this device connected to, most recent first. Reads either
/// shape a prefs file may hold: a bare string (what this list held before
/// it kept names) or `{"url", "network"}`.
pub(crate) fn recent_endpoints() -> Vec<RecentEndpoint> {
    read_prefs()["endpoints"]
        .as_array()
        .map(|list| list.iter().filter_map(endpoint_of_json).collect())
        .unwrap_or_default()
}

fn endpoint_of_json(value: &serde_json::Value) -> Option<RecentEndpoint> {
    if let Some(url) = value.as_str() {
        return Some(RecentEndpoint {
            url: url.to_owned(),
            network: String::new(),
        });
    }
    Some(RecentEndpoint {
        url: value.get("url")?.as_str()?.to_owned(),
        network: value["network"].as_str().unwrap_or_default().to_owned(),
    })
}

/// Moves `endpoint` to the front, tagged with the network it just reported.
pub(crate) fn note_endpoint(endpoint: &str, network: &str) {
    let mut recent = recent_endpoints();
    note(&mut recent, endpoint, network);
    write_endpoints(&recent);
}

fn note(recent: &mut Vec<RecentEndpoint>, endpoint: &str, network: &str) {
    recent.retain(|known| known.url != endpoint);
    recent.insert(
        0,
        RecentEndpoint {
            url: endpoint.to_owned(),
            network: network.to_owned(),
        },
    );
    recent.truncate(8);
}

/// Drops `endpoint` from the recent list — the row's Forget button.
pub(crate) fn forget_endpoint(endpoint: &str) {
    let mut recent = recent_endpoints();
    recent.retain(|known| known.url != endpoint);
    write_endpoints(&recent);
}

fn write_endpoints(recent: &[RecentEndpoint]) {
    let mut prefs = read_prefs();
    let entries: Vec<serde_json::Value> = recent
        .iter()
        .map(|entry| serde_json::json!({"url": entry.url, "network": entry.network}))
        .collect();
    prefs["endpoints"] = serde_json::Value::Array(entries);
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

    #[test]
    fn recent_endpoints_read_both_the_old_bare_list_and_the_named_shape() {
        let old = serde_json::json!(["http://a", "http://b"]);
        let parsed: Vec<_> = old
            .as_array()
            .unwrap()
            .iter()
            .filter_map(endpoint_of_json)
            .collect();
        assert_eq!(
            parsed.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(),
            ["http://a", "http://b"]
        );
        assert!(parsed.iter().all(|e| e.network.is_empty()));

        let named = serde_json::json!([{"url": "http://a", "network": "testkit"}]);
        let parsed: Vec<_> = named
            .as_array()
            .unwrap()
            .iter()
            .filter_map(endpoint_of_json)
            .collect();
        assert_eq!(parsed[0].network, "testkit");
    }

    #[test]
    fn noting_an_endpoint_moves_it_to_the_front_and_dedupes() {
        let mut recent = vec![RecentEndpoint {
            url: "http://a".into(),
            network: "x".into(),
        }];
        note(&mut recent, "http://b", "testkit");
        note(&mut recent, "http://a", "renamed");
        assert_eq!(
            recent.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(),
            ["http://a", "http://b"]
        );
        assert_eq!(recent[0].network, "renamed");
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
