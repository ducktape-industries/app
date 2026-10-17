//! The device registry: which devices an account's notices go to, and the
//! door they register through.
//!
//! A phone sends a [`Registration`] (`ducktape_host::push`: the shape, the
//! namespace, the preimage) to `POST /v1/devices`. It is taken when:
//! - its kind is `voip`, `apns` (each with an APNs `env`) or `fcm` (none);
//! - its `at` is within [`AT_WINDOW_MS`] of now AND newer than the `at`
//!   kept for that key and kind: an old message replayed (a stale token
//!   put back after a rotation, a leave undone) changes nothing, whatever
//!   the window;
//! - its signature verifies under the key it names;
//! - the chain is one pushd follows, and that chain's identity program says
//!   the key acts as the account (`Query::OfKey`, asked of the network's
//!   node with pushd's reader key).
//!
//! An empty token is the device leaving: its row stays as a tombstone (its
//! `at` is what refuses a replay of the token it withdrew), and nothing is
//! sent to it. Every hour each key is asked of identity again, and one that
//! no longer acts as its account is dropped: a phone taken off an account
//! stops hearing the account's notices then at the latest.
//!
//! The rows of each network live in `<data>/<network>/devices.json`,
//! written whole and renamed into place. The process that runs a network's
//! notifiers ([`Devices::follow`]) reads the file again when it moved, so
//! the door and the notifiers need not share a process: with several
//! networks the supervisor holds the door and each child its network.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use commonware_codec::DecodeExt as _;
use commonware_cryptography::Verifier as _;
use commonware_cryptography::ed25519;
use ducktape_host::push::{AT_WINDOW_MS, NAMESPACE, ROUTE, Registration};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::{Request, Response, StatusCode};

/// The most a registration's body may be: a token is a few hundred bytes.
const BODY_LIMIT: usize = 4096;

/// How often each kept key is asked of identity again.
const RECHECK_EVERY: Duration = Duration::from_secs(60 * 60);

/// One device's token, as the file keeps it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Row {
    pub account: u64,
    pub key: String,
    pub kind: String,
    /// Empty: the device left (a tombstone).
    pub token: String,
    pub env: String,
    pub at: u64,
}

/// A network's rows, shared between the door that writes them and the
/// channels that read them ([`Push`](crate::push::Push)).
#[derive(Default)]
pub struct Devices {
    rows: Mutex<Vec<Row>>,
    /// The file's modification time as last read ([`Devices::follow`]).
    seen: Mutex<Option<SystemTime>>,
}

impl Devices {
    /// The rows `file` holds; none when it is not there yet.
    pub fn load(file: &Path) -> Result<Arc<Self>, String> {
        let rows = match std::fs::read(file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("{}: {error}", file.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(format!("{}: {error}", file.display())),
        };
        let seen = std::fs::metadata(file)
            .and_then(|metadata| metadata.modified())
            .ok();
        Ok(Arc::new(Self {
            rows: Mutex::new(rows),
            seen: Mutex::new(seen),
        }))
    }

    /// Reads `file` again when it moved since the last read: whether it did.
    pub fn follow(&self, file: &Path) -> bool {
        let modified = std::fs::metadata(file)
            .and_then(|metadata| metadata.modified())
            .ok();
        let mut seen = lock(&self.seen);
        if modified.is_none() || modified == *seen {
            return false;
        }
        match std::fs::read(file).and_then(|bytes| {
            serde_json::from_slice(&bytes).map_err(std::io::Error::other)
        }) {
            Ok(rows) => {
                *lock(&self.rows) = rows;
                *seen = modified;
                true
            }
            Err(error) => {
                tracing::warn!(target: "pushd", file = %file.display(), %error, "devices not read");
                false
            }
        }
    }

    /// `account`'s tokens of `kind`, each with its env.
    pub fn tokens(&self, account: u64, kind: &str) -> Vec<(String, String)> {
        lock(&self.rows)
            .iter()
            .filter(|row| row.account == account && row.kind == kind && !row.token.is_empty())
            .map(|row| (row.token.clone(), row.env.clone()))
            .collect()
    }

    /// Each account with a device, and its canonical signer: the smallest
    /// of its keys with a token. One notifier runs per account on that
    /// signer, never one per device (chat keys its store by signer: two
    /// would announce every mention twice).
    pub fn accounts(&self) -> Vec<(u64, String)> {
        // ponytail: the smallest key, so the signer flips when a smaller
        // key registers or the smallest leaves, and the account's notifier
        // starts again on a fresh store (what is there then counts as
        // history: nothing is announced twice, badges start at 0); a kept
        // choice per account if that proves to be felt
        let mut accounts: HashMap<u64, String> = HashMap::new();
        for row in lock(&self.rows).iter().filter(|row| !row.token.is_empty()) {
            let signer = accounts.entry(row.account).or_insert_with(|| row.key.clone());
            if row.key < *signer {
                *signer = row.key.clone();
            }
        }
        let mut accounts: Vec<_> = accounts.into_iter().collect();
        accounts.sort();
        accounts
    }

    /// `row` kept in place of the one of its key and kind, when it is newer;
    /// whether it was.
    pub(crate) fn put(&self, row: Row) -> bool {
        let mut rows = lock(&self.rows);
        match rows
            .iter_mut()
            .find(|kept| kept.key == row.key && kept.kind == row.kind)
        {
            Some(kept) if kept.at >= row.at => false,
            Some(kept) => {
                *kept = row;
                true
            }
            None => {
                rows.push(row);
                true
            }
        }
    }

    /// The rows as the file holds them, written whole and renamed into place.
    fn save(&self, file: &Path) -> std::io::Result<()> {
        let rows = lock(&self.rows).clone();
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&rows)?)?;
        std::fs::rename(&tmp, file)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Why a registration was not taken, and the status that says so.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused(pub u16, pub String);

fn bad(reason: impl Into<String>) -> Refused {
    Refused(400, reason.into())
}

/// `registration` checked for what needs no node: its fields, its time
/// against `now` (unix ms), its signature. The key it names, decoded.
pub fn checked(registration: &Registration, now: u64) -> Result<[u8; 32], Refused> {
    match (registration.kind.as_str(), registration.env.as_str()) {
        ("voip" | "apns", "production" | "sandbox") | ("fcm", "") => {}
        ("voip" | "apns", _) => return Err(bad("an APNs token's env is production or sandbox")),
        ("fcm", _) => return Err(bad("an fcm token has no env")),
        _ => return Err(bad("a token's kind is voip, apns or fcm")),
    }
    if registration.token.len() > 512 || registration.chain.len() > 128 {
        return Err(bad("a token is at most 512 bytes, a chain 128"));
    }
    if registration.at.abs_diff(now) > AT_WINDOW_MS {
        return Err(bad("the registration's time is too far from now"));
    }
    let key: [u8; 32] = hex(&registration.key)?
        .try_into()
        .map_err(|_| bad("the key is 32 bytes"))?;
    let sig: [u8; 64] = hex(&registration.sig)?
        .try_into()
        .map_err(|_| bad("the signature is 64 bytes"))?;
    let public = ed25519::PublicKey::decode(key.as_slice())
        .map_err(|_| bad("the key is not an Ed25519 public key"))?;
    let signature = ed25519::Signature::decode(sig.as_slice())
        .map_err(|_| bad("the signature does not decode"))?;
    if !public.verify(NAMESPACE, &registration.preimage(), &signature) {
        return Err(bad("the signature is not the key's over this registration"));
    }
    Ok(key)
}

fn hex(text: &str) -> Result<Vec<u8>, Refused> {
    noded_client::hex::hex_decode(text).map_err(|_| bad("not hex"))
}

/// What the door knows of one network: its rows, where they are kept, and
/// the node that answers which account a key acts as.
pub struct Followed {
    pub name: String,
    pub devices: Arc<Devices>,
    pub file: PathBuf,
    pub client: noded_client::Client,
    /// The chain as links spell it, and the network's name as frames name
    /// it, once the node answered.
    pub chain: Mutex<Option<(String, String)>>,
}

impl Followed {
    /// The chain id and network name, from the node's status the first
    /// time it answers.
    async fn chain(&self) -> Option<(String, String)> {
        if let Some(chain) = lock(&self.chain).clone() {
            return Some(chain);
        }
        let status = self.client.status().await.ok()?;
        let chain = ducklink::ChainId::of(&status.network, &status.genesis)
            .map_or_else(|| status.network.clone(), |chain| chain.to_string());
        let answer = (chain, status.network);
        *lock(&self.chain) = Some(answer.clone());
        Some(answer)
    }
}

/// The door: the networks it registers devices for and the key its
/// identity queries are signed with.
pub struct Door {
    pub networks: Vec<Arc<Followed>>,
    pub reader: Arc<dyn noded_client::Signer>,
}

impl Door {
    /// `registration` taken, or why not.
    pub async fn register(&self, registration: Registration) -> Result<(), Refused> {
        let key = checked(&registration, crate::push::now_ms())?;
        let mut followed = None;
        for network in &self.networks {
            if let Some((chain, name)) = network.chain().await
                && chain == registration.chain
            {
                followed = Some((network.clone(), name));
                break;
            }
        }
        let Some((network, name)) = followed else {
            return Err(Refused(404, "pushd does not follow this chain".into()));
        };
        let acts = noded_client::identity::account_of_key(
            &network.client,
            &name,
            key.to_vec(),
            &*self.reader,
        )
        .await
        .map_err(|error| Refused(503, format!("the node did not answer: {error}")))?;
        if acts.map(|(number, _)| number) != Some(registration.account) {
            return Err(Refused(403, "the key does not act as this account".into()));
        }
        let kept = network.devices.put(Row {
            account: registration.account,
            key: registration.key,
            kind: registration.kind,
            token: registration.token,
            env: registration.env,
            at: registration.at,
        });
        if !kept {
            return Err(bad("an older registration of this key and kind"));
        }
        network
            .devices
            .save(&network.file)
            .map_err(|error| Refused(500, format!("devices not saved: {error}")))?;
        Ok(())
    }

    /// Every kept key asked of identity again; one that no longer acts as
    /// its account is dropped. A node that did not answer keeps its rows.
    pub async fn recheck(&self) {
        for network in &self.networks {
            let Some((_, name)) = network.chain().await else {
                continue;
            };
            let rows = lock(&network.devices.rows).clone();
            let mut dropped = Vec::new();
            for row in rows.iter().filter(|row| !row.token.is_empty()) {
                let Ok(key) = noded_client::hex::hex_decode(&row.key) else {
                    continue;
                };
                let acts =
                    noded_client::identity::account_of_key(&network.client, &name, key, &*self.reader)
                        .await;
                if let Ok(acts) = acts
                    && acts.map(|(number, _)| number) != Some(row.account)
                {
                    dropped.push((row.key.clone(), row.kind.clone()));
                }
            }
            if dropped.is_empty() {
                continue;
            }
            lock(&network.devices.rows).retain(|row| !dropped.contains(&(row.key.clone(), row.kind.clone())));
            tracing::info!(target: "pushd", network = network.name, dropped = dropped.len(),
                "devices whose key left the account dropped");
            if let Err(error) = network.devices.save(&network.file) {
                tracing::warn!(target: "pushd", %error, "devices not saved");
            }
        }
    }

    /// The door served on `listener` for as long as the runtime runs, and
    /// the hourly recheck beside it.
    pub fn serve(self: Arc<Self>, listener: std::net::TcpListener) -> Result<(), String> {
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let handle = ducktape_host::runtime::handle();
        let door = self.clone();
        handle.spawn(async move {
            loop {
                tokio::time::sleep(RECHECK_EVERY).await;
                door.recheck().await;
            }
        });
        let listener = handle
            .block_on(async { tokio::net::TcpListener::from_std(listener) })
            .map_err(|error| error.to_string())?;
        handle.spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                };
                let door = self.clone();
                tokio::spawn(async move {
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let service = hyper::service::service_fn(move |request| {
                        let door = door.clone();
                        async move { Ok::<_, hyper::Error>(door.answer(request).await) }
                    });
                    let served = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .await;
                    if let Err(error) = served {
                        tracing::debug!(target: "pushd", %error, "a connection to the door ended");
                    }
                });
            }
        });
        Ok(())
    }

    /// One request to the door answered.
    async fn answer(&self, request: Request<hyper::body::Incoming>) -> Response<Full<Bytes>> {
        let reply = |status: u16, text: String| {
            Response::builder()
                .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
                .header("content-type", "text/plain; charset=utf-8")
                .body(Full::new(Bytes::from(text)))
                .unwrap_or_default()
        };
        let (head, body) = request.into_parts();
        match (head.method.as_str(), head.uri.path()) {
            ("GET", "/") => reply(200, "pushd\n".into()),
            ("POST", path) if path == ROUTE => {
                let body = http_body_util::Limited::new(body, BODY_LIMIT)
                    .collect()
                    .await
                    .map(|collected| collected.to_bytes());
                let Ok(body) = body else {
                    return reply(413, "a registration is at most 4 KiB\n".into());
                };
                let registration: Registration = match serde_json::from_slice(&body) {
                    Ok(registration) => registration,
                    Err(error) => return reply(400, format!("not a registration: {error}\n")),
                };
                let (chain, account, kind) = (
                    registration.chain.clone(),
                    registration.account,
                    registration.kind.clone(),
                );
                match self.register(registration).await {
                    Ok(()) => {
                        tracing::info!(target: "pushd", chain, account, kind, "device registered");
                        reply(204, String::new())
                    }
                    Err(Refused(status, reason)) => {
                        tracing::info!(target: "pushd", chain, account, kind, status, reason, "registration refused");
                        reply(status, format!("{reason}\n"))
                    }
                }
            }
            _ => reply(404, "not here\n".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::Signer as _;

    fn key() -> ed25519::PrivateKey {
        ed25519::PrivateKey::decode([7u8; 32].as_slice()).unwrap()
    }

    /// `registration` signed by `key` at `at`.
    pub(crate) fn signed(mut registration: Registration, key: &ed25519::PrivateKey, at: u64) -> Registration {
        use commonware_codec::Encode as _;
        registration.at = at;
        registration.key = noded_client::hex::hex_encode(&key.public_key().encode());
        let signature = key.sign(NAMESPACE, &registration.preimage());
        registration.sig = noded_client::hex::hex_encode(&signature.encode());
        registration
    }

    fn voip() -> Registration {
        Registration {
            chain: "testkit-0a1b2c3d".into(),
            account: 15,
            key: String::new(),
            kind: "voip".into(),
            token: "ab12".into(),
            env: "sandbox".into(),
            at: 0,
            sig: String::new(),
        }
    }

    const NOW: u64 = 1_700_000_000_000;

    /// A registration is taken when its key signed it, its time is near
    /// now, and its kind and env fit; a changed field, another key's
    /// signature, a stale time or an fcm token with an env is refused.
    #[test]
    fn a_registration_is_its_keys_word_at_about_now() {
        let key = key();
        let good = signed(voip(), &key, NOW - 1000);
        assert!(checked(&good, NOW).is_ok());
        let tampered = Registration {
            token: "ab13".into(),
            ..good.clone()
        };
        assert!(checked(&tampered, NOW).unwrap_err().1.contains("signature"));
        let other = ed25519::PrivateKey::decode([8u8; 32].as_slice()).unwrap();
        let forged = Registration {
            key: good.key.clone(),
            ..signed(voip(), &other, NOW)
        };
        assert!(checked(&forged, NOW).is_err());
        let stale = signed(voip(), &key, NOW - AT_WINDOW_MS - 1);
        assert!(checked(&stale, NOW).unwrap_err().1.contains("time"));
        let early = signed(voip(), &key, NOW + AT_WINDOW_MS + 1);
        assert!(checked(&early, NOW).is_err());
        let fcm = signed(
            Registration {
                kind: "fcm".into(),
                env: "sandbox".into(),
                ..voip()
            },
            &key,
            NOW,
        );
        assert!(checked(&fcm, NOW).unwrap_err().1.contains("fcm"));
        let unknown = signed(
            Registration {
                kind: "sms".into(),
                ..voip()
            },
            &key,
            NOW,
        );
        assert_eq!(checked(&unknown, NOW).unwrap_err().0, 400);
    }

    /// The rows keep one token per key and kind, the newest `at` winning:
    /// a rotation replaces, a replay of what came before is refused, a
    /// leave is a tombstone that still refuses the token it withdrew; the
    /// account's signer is its smallest key with a token, and an account
    /// whose devices all left has none.
    #[test]
    fn rows_keep_the_newest_token_per_key_and_kind() {
        let devices = Devices::default();
        let row = |key: &str, kind: &str, token: &str, at: u64| Row {
            account: 15,
            key: key.into(),
            kind: kind.into(),
            token: token.into(),
            env: "production".into(),
            at,
        };
        assert!(devices.put(row("bb", "voip", "t1", 10)));
        assert!(devices.put(row("bb", "apns", "t2", 10)));
        assert!(devices.put(row("aa", "fcm", "t3", 10)));
        assert_eq!(devices.accounts(), vec![(15, "aa".into())]);
        // a rotation
        assert!(devices.put(row("bb", "voip", "t1b", 11)));
        // the old token replayed
        assert!(!devices.put(row("bb", "voip", "t1", 10)));
        assert_eq!(
            devices.tokens(15, "voip"),
            vec![("t1b".to_owned(), "production".to_owned())]
        );
        // aa leaves: bb is the signer now
        assert!(devices.put(row("aa", "fcm", "", 12)));
        assert!(!devices.put(row("aa", "fcm", "t3", 10)));
        assert!(devices.tokens(15, "fcm").is_empty());
        assert_eq!(devices.accounts(), vec![(15, "bb".into())]);
        assert!(devices.put(row("bb", "voip", "", 13)));
        assert!(devices.put(row("bb", "apns", "", 13)));
        assert!(devices.accounts().is_empty());
    }

    /// The file is written whole and read back; one that moved is read
    /// again by a follower, one that did not is not.
    #[test]
    fn the_file_round_trips_and_a_follower_reads_it_when_it_moved() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("testkit").join("devices.json");
        let devices = Devices::load(&file).unwrap();
        assert!(devices.accounts().is_empty());
        devices.put(Row {
            account: 2,
            key: "aa".into(),
            kind: "fcm".into(),
            token: "t".into(),
            env: String::new(),
            at: 1,
        });
        devices.save(&file).unwrap();
        let follower = Devices::load(&file).unwrap();
        assert_eq!(follower.accounts(), vec![(2, "aa".into())]);
        assert!(!follower.follow(&file));
        devices.put(Row {
            account: 3,
            key: "bb".into(),
            kind: "fcm".into(),
            token: "t".into(),
            env: String::new(),
            at: 1,
        });
        // a second within the same instant would read as unmoved on a
        // coarse filesystem clock
        std::thread::sleep(Duration::from_millis(20));
        devices.save(&file).unwrap();
        assert!(follower.follow(&file));
        assert_eq!(follower.accounts().len(), 2);
    }
}
