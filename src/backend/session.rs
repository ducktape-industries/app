//! This device: its signing key, opened from the keystore and held here
//! while the person is signed in; the frames it signs; and the small
//! preferences file. Every write a view submits is signed here — a view
//! never sees the private key or a password.

use std::path::PathBuf;

use commonware_cryptography::{Signer as _, ed25519};
use view_wire::Refusal;

use super::noded::{Frame, Layer};
use super::{RpcClient, hex_encode, refused};

/// The host namespace that holds each signer's next sequence.
const SIGNERS: &str = "$signers";

struct Signer {
    key: ed25519::PrivateKey,
}

static SIGNER: tokio::sync::Mutex<Option<Signer>> = tokio::sync::Mutex::const_new(None);

pub(crate) const LOCKED: &str = "this device's key is locked; unlock it first";

pub(crate) fn locked_seat() -> Refusal {
    Refusal::new("session_locked", LOCKED)
}

/// Seats `key` as the one that signs; answers its public key.
pub(crate) async fn seat_key(key: ed25519::PrivateKey) -> String {
    let pubkey = hex_encode(key.public_key().as_ref());
    *SIGNER.lock().await = Some(Signer { key });
    pubkey
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

/// Where this device keeps its wallets for a network: the directory
/// [`bind_keyring`] named, under the ducktape home.
pub(crate) fn keystore_root(keyring: &str) -> Result<PathBuf, String> {
    let named = !keyring.is_empty() && !keyring.contains(['/', '\\']) && keyring != "..";
    if !named {
        return Err("the node named no network".into());
    }
    Ok(ducktape_home::root()?.join("remotes").join(keyring))
}

/// The file in a network's key directory that says which chain it belongs
/// to: that chain's founding time, as the node's status reports it.
const FOUNDED: &str = "network-founded";

/// Where a network's keys live on this device, and whether its NAME was
/// already here for another chain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Keyring {
    /// The directory under `remotes/` ([`keystore_root`]).
    pub(crate) dir: String,
    /// Another chain holds this name's directory: this one is a different
    /// network that happens to share it.
    pub(crate) other_chain: bool,
}

/// Binds `network` as the node names it, founded at `founded` (the node's
/// `Status::time`), to a key directory. A name is not an identity — two
/// chains can both be called "testkit" — but a name AND its founding time
/// are: the genesis block is built from exactly those two
/// (`Block::genesis(name, time)` in core). So the first chain seen under a
/// name keeps `remotes/<name>` (a directory from before this binding is
/// claimed by whichever chain connects first, and keeps working for it),
/// and any other chain with that name gets `remotes/<name>+<founded>` —
/// its own keys, never the first chain's.
pub(crate) fn bind_keyring(network: &str, founded: u64) -> Result<Keyring, String> {
    bind_in(&ducktape_home::root()?.join("remotes"), network, founded)
}

fn bind_in(remotes: &std::path::Path, network: &str, founded: u64) -> Result<Keyring, String> {
    // `+` is never in a sanitized name, so `<name>+<founded>` can not be
    // another network's own directory.
    let name: String = network
        .chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                true => c,
                false => '_',
            },
        )
        .collect();
    if name.is_empty() || name.chars().all(|c| c == '.') {
        return Err("the node named no network".into());
    }
    let mark = remotes.join(&name).join(FOUNDED);
    let known = std::fs::read_to_string(&mark)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok());
    match known {
        Some(known) if known != founded => Ok(Keyring {
            dir: format!("{name}+{founded}"),
            other_chain: true,
        }),
        Some(_) => Ok(Keyring {
            dir: name,
            other_chain: false,
        }),
        None => {
            // An unwritten mark only means the claim is made again next
            // time; the keys themselves are where they always were.
            let written = std::fs::create_dir_all(remotes.join(&name))
                .and_then(|()| std::fs::write(&mark, founded.to_string()));
            if let Err(error) = written {
                tracing::warn!(target: "ducktape::app", %error, network, "chain mark not written");
            }
            Ok(Keyring {
                dir: name,
                other_chain: false,
            })
        }
    }
}

/// The key file a sign-in opens: `DUCKTAPE_USER_KEY`, else the keyring's
/// active wallet.
pub(crate) fn session_key_path(keyring: &str) -> Result<PathBuf, String> {
    if let Some(path) = keystore::wallet::env_user_key() {
        return Ok(path);
    }
    keystore::wallet::active_user_key(&keystore_root(keyring)?)
}

/// Whether this device already holds a signing key in `keyring` — the
/// question the sign-in screen answers before offering Unlock or Create.
pub(crate) fn key_exists(keyring: &str) -> bool {
    let Ok(path) = session_key_path(keyring) else {
        return false;
    };
    !matches!(
        keystore::userkey::key_file_state(&path),
        keystore::userkey::KeyFileState::Absent
    )
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

/// Whether the launcher's drawings move; on unless turned off.
pub(crate) fn load_motion() -> bool {
    read_prefs()["motion"].as_bool().unwrap_or(true)
}

pub(crate) fn save_motion(on: bool) -> bool {
    let mut prefs = read_prefs();
    prefs["motion"] = serde_json::json!(on);
    write_prefs(&prefs)
}

pub(crate) const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8844";

pub(crate) const ENDPOINT_REFUSAL: &str = "A node address is a host and an optional port (127.0.0.1:8844), with http:// or https:// in front if you like, and nothing else.";

/// The origin of a node URL, or `None` for anything that is not one. A bare
/// `host:port` is what a node prints and what people paste, so no scheme
/// means `http://` — checked by `://`, since `localhost:8844` would
/// otherwise parse as a URL whose scheme is `localhost`.
pub(crate) fn endpoint_origin(url: &str) -> Option<String> {
    let url = url.trim();
    let url = match url.contains("://") {
        true => reqwest::Url::parse(url).ok()?,
        false => reqwest::Url::parse(&format!("http://{url}")).ok()?,
    };
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RecentEndpoint {
    pub(crate) url: String,
    pub(crate) network: String,
    /// The network's founding time (0 for an entry from before the app
    /// kept it), which tells two chains with one name apart.
    pub(crate) founded: u64,
    /// This node's chain is not the one this device first met under its
    /// name ([`Keyring::other_chain`]).
    pub(crate) other_chain: bool,
}

impl RecentEndpoint {
    /// The node's host and port — the URL without its scheme.
    pub(crate) fn host(&self) -> &str {
        self.url
            .split_once("://")
            .map_or(self.url.as_str(), |(_, host)| host)
    }

    /// How a list names this node: its network and host, and — when two
    /// chains share the name — which one is the other chain. Two rows
    /// never read the same: the list holds one row per URL.
    pub(crate) fn label(&self) -> String {
        match (self.network.is_empty(), self.other_chain) {
            (true, _) => self.host().to_owned(),
            (false, false) => format!("{} · {}", self.network, self.host()),
            (false, true) => format!("{} · {} · different network", self.network, self.host()),
        }
    }
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
            ..RecentEndpoint::default()
        });
    }
    Some(RecentEndpoint {
        url: value.get("url")?.as_str()?.to_owned(),
        network: value["network"].as_str().unwrap_or_default().to_owned(),
        founded: value["founded"].as_u64().unwrap_or_default(),
        other_chain: value["other_chain"].as_bool().unwrap_or_default(),
    })
}

/// Moves `entry` to the front: its URL, with the network it just reported.
pub(crate) fn note_endpoint(entry: RecentEndpoint) {
    let mut recent = recent_endpoints();
    note(&mut recent, entry);
    write_endpoints(&recent);
}

fn note(recent: &mut Vec<RecentEndpoint>, entry: RecentEndpoint) {
    recent.retain(|known| known.url != entry.url);
    recent.insert(0, entry);
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
        .map(|entry| {
            serde_json::json!({
                "url": entry.url,
                "network": entry.network,
                "founded": entry.founded,
                "other_chain": entry.other_chain,
            })
        })
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
        assert_eq!(
            endpoint_origin("127.0.0.1:34329").as_deref(),
            Some("http://127.0.0.1:34329")
        );
        assert_eq!(
            endpoint_origin(" localhost:8844 ").as_deref(),
            Some("http://localhost:8844")
        );
        for junk in [
            "",
            "hello world",
            "host:port",
            "host/v1",
            "a:b@host",
            "host?x=1",
        ] {
            assert_eq!(endpoint_origin(junk), None, "{junk:?}");
        }
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
        let entry = |url: &str, network: &str| RecentEndpoint {
            url: url.into(),
            network: network.into(),
            ..RecentEndpoint::default()
        };
        let mut recent = vec![entry("http://a", "x")];
        note(&mut recent, entry("http://b", "testkit"));
        note(&mut recent, entry("http://a", "renamed"));
        assert_eq!(
            recent.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(),
            ["http://a", "http://b"]
        );
        assert_eq!(recent[0].network, "renamed");
    }

    #[test]
    fn a_name_binds_to_the_chain_first_seen_under_it() {
        let remotes = tempfile::tempdir().unwrap();
        let first = bind_in(remotes.path(), "testkit", 100).unwrap();
        assert_eq!(first.dir, "testkit");
        assert!(!first.other_chain);
        // the same chain again: the same keys
        assert_eq!(bind_in(remotes.path(), "testkit", 100).unwrap(), first);
        // another chain with the name: its own directory, never the first's
        let other = bind_in(remotes.path(), "testkit", 200).unwrap();
        assert_eq!(other.dir, "testkit+200");
        assert!(other.other_chain);
        assert_eq!(bind_in(remotes.path(), "testkit", 200).unwrap(), other);
        // the first chain still owns the name afterwards
        assert_eq!(bind_in(remotes.path(), "testkit", 100).unwrap(), first);
        assert!(bind_in(remotes.path(), "", 1).is_err());
        assert!(bind_in(remotes.path(), "..", 1).is_err());
    }

    #[test]
    fn a_key_directory_from_before_the_binding_goes_to_the_first_chain_that_connects() {
        let remotes = tempfile::tempdir().unwrap();
        let keys = keystore::wallet::key_file(&remotes.path().join("testkit"), "default");
        std::fs::create_dir_all(keys.parent().unwrap()).unwrap();
        std::fs::write(&keys, "sealed").unwrap();
        assert_eq!(
            bind_in(remotes.path(), "testkit", 7).unwrap().dir,
            "testkit"
        );
        assert!(keys.exists(), "the old key is left where it was");
        assert!(bind_in(remotes.path(), "testkit", 8).unwrap().other_chain);
    }

    #[test]
    fn recent_rows_name_the_host_and_mark_the_other_chain() {
        let row = |url: &str, other_chain| RecentEndpoint {
            url: url.into(),
            network: "testkit".into(),
            founded: 1,
            other_chain,
        };
        assert_eq!(
            row("http://127.0.0.1:36817", false).label(),
            "testkit · 127.0.0.1:36817"
        );
        assert_eq!(
            row("http://127.0.0.1:34329", true).label(),
            "testkit · 127.0.0.1:34329 · different network"
        );
        let bare = RecentEndpoint {
            url: "https://node.example".into(),
            ..RecentEndpoint::default()
        };
        assert_eq!(bare.label(), "node.example");
        let saved = serde_json::json!({"url": "http://c", "network": "testkit", "founded": 9, "other_chain": true});
        assert_eq!(endpoint_of_json(&saved).unwrap(), {
            let mut c = row("http://c", true);
            c.founded = 9;
            c
        });
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
