//! The device registry: which devices an account's notices go to, and the
//! door they register through.
//!
//! A phone sends a [`Registration`] (`ducktape_host::push`: the shape, the
//! namespace, the preimage) to `POST /v1/devices`. It is taken when:
//! - its kind is `voip`, `apns` (each with an APNs `env`, its token hex)
//!   or `fcm` (no env);
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
//! sent to it. A token registered under another key (the phone seated
//! another account) leaves the old key's row the same way. Every hour each
//! key is asked of identity again, and one that no longer acts as its
//! account is dropped: a phone taken off an account stops hearing the
//! account's notices then at the latest.
//!
//! The rows of each network live in `<data>/<network>/devices.json`,
//! written whole and renamed into place. The process that runs a network's
//! notifiers ([`Devices::follow`]) reads the file again when it moved, so
//! the door and the notifiers need not share a process: with several
//! networks the supervisor holds the door and each child its network. Both
//! write it (the door a registration, the child a token APNs or FCM calls
//! gone), so each write is one [`Devices::edit`] under the file's lock.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

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

/// Registrations one client may send a minute, and all clients together.
/// Each costs the node up to two identity queries and the log a line. A
/// phone sends one or two at launch (voip and apns, or fcm) and retries a
/// refused one 30 s later: 30 lets 15 iPhones behind one address launch in
/// the same minute; 300 holds the node to 10 queries a second from here.
const PER_CLIENT: u32 = 30;
const OVERALL: u32 = 300;

/// How long a client has for a request's headers, and then for its body:
/// a registration is under 4 KiB, so one held back longer is let go
/// (slowloris).
const DEADLINE: Duration = Duration::from_secs(10);

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
    /// The file's modification time as last read ([`Devices::follow`]);
    /// none once written here, so it is read again.
    seen: Mutex<Option<SystemTime>>,
    /// Where the rows are kept; none for a test's rows or the probe's,
    /// which live in memory only.
    file: Option<PathBuf>,
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
            file: Some(file.to_owned()),
        }))
    }

    /// Reads `file` again when it moved since the last read, or was
    /// written here since ([`Devices::edit`]): whether it did.
    pub fn follow(&self, file: &Path) -> bool {
        let modified = std::fs::metadata(file)
            .and_then(|metadata| metadata.modified())
            .ok();
        let mut seen = lock(&self.seen);
        if modified.is_none() || modified == *seen {
            return false;
        }
        match std::fs::read(file)
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(std::io::Error::other))
        {
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
            let signer = accounts
                .entry(row.account)
                .or_insert_with(|| row.key.clone());
            if row.key < *signer {
                *signer = row.key.clone();
            }
        }
        let mut accounts: Vec<_> = accounts.into_iter().collect();
        accounts.sort();
        accounts
    }

    /// `row` kept in place of the one of its key and kind, when it is newer;
    /// whether it was ([`put`]).
    pub(crate) fn put(&self, row: Row) -> std::io::Result<bool> {
        self.edit(|rows| put(rows, row))
    }

    /// The row of `kind` holding `token` made a tombstone: APNs or FCM said
    /// the token is gone. Its `at` stays, so a replay of the registration
    /// is still refused (as a leave's is, [`put`]). Whether one was there.
    pub(crate) fn drop_token(&self, kind: &str, token: &str) -> std::io::Result<bool> {
        self.edit(|rows| {
            let mut dropped = false;
            for row in rows
                .iter_mut()
                .filter(|row| row.kind == kind && row.token == token && !token.is_empty())
            {
                row.token.clear();
                dropped = true;
            }
            dropped
        })
    }

    /// `change` made to the rows as the file holds them now, and the file
    /// written whole and renamed into place. The door and a network's
    /// process each write it, so each holds `<file>.lock` from its read to
    /// its rename: a write never lands over one it did not read. The rows
    /// stay held throughout too: two writes from one process would write one
    /// temporary file together and rename a torn one into place. Rows with
    /// no file are changed in memory only.
    pub(crate) fn edit<T>(&self, change: impl FnOnce(&mut Vec<Row>) -> T) -> std::io::Result<T> {
        let mut rows = lock(&self.rows);
        let Some(file) = &self.file else {
            return Ok(change(&mut rows));
        };
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(file.with_extension("json.lock"))?;
        held.lock()?;
        let mut fresh = match std::fs::read(file) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        let answer = change(&mut fresh);
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&fresh)?)?;
        std::fs::rename(&tmp, file)?;
        *rows = fresh;
        drop(rows);
        // moved here: the next `follow` reads the file back and says so,
        // whatever its clock, which is what stops the notifier of an
        // account a drop left with no token (`carry`, main.rs)
        *lock(&self.seen) = None;
        Ok(answer)
    }
}

/// `row` kept in `rows` in place of the one of its key and kind, when it is
/// newer; whether it was. A token is one install's, on the key seated there
/// now: the same token kept under another key (the phone seated another
/// account, and the key it left signs no leave) becomes a tombstone, whose
/// own `at` still refuses a replay.
fn put(rows: &mut Vec<Row>, row: Row) -> bool {
    let at = rows
        .iter()
        .position(|kept| kept.key == row.key && kept.kind == row.kind);
    if at.is_some_and(|at| rows[at].at >= row.at) {
        return false;
    }
    if !row.token.is_empty() {
        rows.iter_mut()
            .filter(|kept| kept.kind == row.kind && kept.token == row.token)
            .for_each(|kept| kept.token.clear());
    }
    match at {
        Some(at) => rows[at] = row,
        None => rows.push(row),
    }
    true
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
    // an APNs token ends the URL it is pushed to (`/3/device/<token>`):
    // hex, never a `/`, `..` or `?` that moves the path
    if registration.kind != "fcm"
        && !registration
            .token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(bad("an APNs token is hex"));
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

/// What the door knows of one network: its rows and the node that answers
/// which account a key acts as.
pub struct Followed {
    pub name: String,
    pub devices: Arc<Devices>,
    pub client: noded_client::Client,
    /// The chain id (`<network>#<salt>`) and the network's name as frames
    /// name it, once the node answered.
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

/// This minute's registrations, per client and in all.
#[derive(Default)]
struct Counts {
    since: Option<Instant>,
    all: u32,
    per: HashMap<IpAddr, u32>,
}

impl Counts {
    /// One more from `client` counted when it is under both limits: whether
    /// it was. No client is kept past the overall limit, so the map holds
    /// at most [`OVERALL`].
    fn take(&mut self, client: IpAddr) -> bool {
        // ponytail: a fixed minute, so a client may send twice its limit
        // across a minute's edge; a sliding window if that is ever felt
        if self
            .since
            .is_none_or(|since| since.elapsed() >= Duration::from_secs(60))
        {
            *self = Counts {
                since: Some(Instant::now()),
                ..Counts::default()
            };
        }
        if self.all >= OVERALL {
            return false;
        }
        let sent = self.per.entry(client).or_default();
        if *sent >= PER_CLIENT {
            return false;
        }
        *sent += 1;
        self.all += 1;
        true
    }
}

/// Who sent a request that came from `peer`. Behind the tunnel every
/// request comes from cloudflared on this host (127.0.0.1), so the client
/// is the address Cloudflare names in `CF-Connecting-IP`. Trusting that
/// header is safe only because the door listens on loopback (`listen =
/// 127.0.0.1:9201`): a peer elsewhere could name any address, so only a
/// loopback peer's is read. An IPv6 client is its /64, the block one host
/// is handed.
fn client(head: &hyper::http::request::Parts, peer: IpAddr) -> IpAddr {
    let named = head
        .headers
        .get("cf-connecting-ip")
        .and_then(|value| value.to_str().ok()?.parse().ok());
    match named.filter(|_| peer.is_loopback()).unwrap_or(peer) {
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & !u128::from(u64::MAX)).into()),
        v4 => v4,
    }
}

/// The door: the networks it registers devices for and the key its
/// identity queries are signed with.
pub struct Door {
    pub networks: Vec<Arc<Followed>>,
    pub reader: Arc<dyn noded_client::Signer>,
    counts: Mutex<Counts>,
    deadline: Duration,
}

impl Door {
    pub fn new(networks: Vec<Arc<Followed>>, reader: Arc<dyn noded_client::Signer>) -> Self {
        Self {
            networks,
            reader,
            counts: Mutex::default(),
            deadline: DEADLINE,
        }
    }

    /// `registration` taken, or why not.
    pub async fn register(&self, registration: Registration) -> Result<(), Refused> {
        let key = checked(&registration, crate::push::now_ms())?;
        let (mut followed, mut unknown) = (None, false);
        for network in &self.networks {
            match network.chain().await {
                Some((chain, name)) if chain == registration.chain => {
                    followed = Some((network.clone(), name));
                    break;
                }
                Some(_) => {}
                None => unknown = true,
            }
        }
        // a 404 is final to a phone, which tries no more: said only when
        // every node followed named its chain, and none is this one
        let Some((network, name)) = followed else {
            return Err(match unknown {
                true => Refused(503, "the network's node did not answer".into()),
                false => Refused(404, "pushd does not follow this chain".into()),
            });
        };
        let acts = noded_client::identity::account_of_key(
            &network.client,
            &name,
            key.to_vec(),
            &*self.reader,
        )
        .await
        // the reply goes to the internet: the node's own words (its
        // address among them) only to the log
        .map_err(|error| {
            tracing::warn!(target: "pushd", network = network.name, %error, "identity not asked");
            Refused(503, "the network's node did not answer".into())
        })?;
        if acts.map(|(number, _)| number) != Some(registration.account) {
            return Err(Refused(403, "the key does not act as this account".into()));
        }
        let kept = network
            .devices
            .put(Row {
                account: registration.account,
                key: registration.key,
                kind: registration.kind,
                token: registration.token,
                env: registration.env,
                at: registration.at,
            })
            .map_err(|error| {
                tracing::warn!(target: "pushd", network = network.name, %error, "devices not saved");
                Refused(500, "the registration was not kept".into())
            })?;
        if !kept {
            return Err(bad("an older registration of this key and kind"));
        }
        Ok(())
    }

    /// Every kept key asked of identity again; one identity says no longer
    /// acts as its account is dropped. A node that did not answer, or that
    /// refused the query for a reason that is not the key's, keeps its rows.
    pub async fn recheck(&self) {
        for network in &self.networks {
            let Some((_, name)) = network.chain().await else {
                continue;
            };
            let rows = lock(&network.devices.rows).clone();
            let (mut dropped, mut missed) = (Vec::new(), Vec::new());
            for row in rows.iter().filter(|row| !row.token.is_empty()) {
                let Ok(key) = noded_client::hex::hex_decode(&row.key) else {
                    continue;
                };
                let acts = noded_client::identity::account_of_key(
                    &network.client,
                    &name,
                    key,
                    &*self.reader,
                )
                .await;
                match acts {
                    Ok(Some((number, _))) if number == row.account => {}
                    Ok(_) => dropped.push((row.key.clone(), row.kind.clone())),
                    Err(error) => missed.push(error),
                }
            }
            if let Some(error) = missed.last() {
                tracing::warn!(target: "pushd", network = network.name, kept = missed.len(), %error,
                    "identity not asked again: those devices stay");
            }
            if dropped.is_empty() {
                continue;
            }
            let edited = network.devices.edit(|rows| {
                rows.retain(|row| !dropped.contains(&(row.key.clone(), row.kind.clone())))
            });
            match edited {
                Ok(()) => tracing::info!(target: "pushd", network = network.name,
                    dropped = dropped.len(), "devices whose key left the account dropped"),
                Err(error) => tracing::warn!(target: "pushd", %error, "devices not saved"),
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
                let Ok((stream, peer)) = listener.accept().await else {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                };
                let door = self.clone();
                let deadline = self.deadline;
                tokio::spawn(async move {
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let service = hyper::service::service_fn(move |request| {
                        let door = door.clone();
                        async move { Ok::<_, hyper::Error>(door.answer(request, peer.ip()).await) }
                    });
                    // a client that holds its headers back is let go; the
                    // timer also closes a keep-alive connection left idle
                    let served = hyper::server::conn::http1::Builder::new()
                        .timer(hyper_util::rt::TokioTimer::new())
                        .header_read_timeout(deadline)
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
    async fn answer(
        &self,
        request: Request<hyper::body::Incoming>,
        peer: IpAddr,
    ) -> Response<Full<Bytes>> {
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
                // counted before anything is read or asked, and never logged:
                // a flood reaches neither the node nor the log
                if !lock(&self.counts).take(client(&head, peer)) {
                    return reply(429, "too many registrations: retry in a minute\n".into());
                }
                let body = http_body_util::Limited::new(body, BODY_LIMIT).collect();
                let body = match tokio::time::timeout(self.deadline, body).await {
                    Err(_) => {
                        return reply(408, "the registration did not arrive in time\n".into());
                    }
                    Ok(Err(_)) => return reply(413, "a registration is at most 4 KiB\n".into()),
                    Ok(Ok(collected)) => collected.to_bytes(),
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
    pub(crate) fn signed(
        mut registration: Registration,
        key: &ed25519::PrivateKey,
        at: u64,
    ) -> Registration {
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
    /// signature, a stale time, an fcm token with an env or an APNs token
    /// that is not hex is refused.
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
        let path = signed(
            Registration {
                token: "../../3/device/ab12".into(),
                ..voip()
            },
            &key,
            NOW,
        );
        assert!(checked(&path, NOW).unwrap_err().1.contains("hex"));
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
    /// whose devices all left has none; a token registered under another
    /// key leaves the old key's row.
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
        assert!(devices.put(row("bb", "voip", "t1", 10)).unwrap());
        assert!(devices.put(row("bb", "apns", "t2", 10)).unwrap());
        assert!(devices.put(row("aa", "fcm", "t3", 10)).unwrap());
        assert_eq!(devices.accounts(), vec![(15, "aa".into())]);
        // a rotation
        assert!(devices.put(row("bb", "voip", "t1b", 11)).unwrap());
        // the old token replayed
        assert!(!devices.put(row("bb", "voip", "t1", 10)).unwrap());
        assert_eq!(
            devices.tokens(15, "voip"),
            vec![("t1b".to_owned(), "production".to_owned())]
        );
        // aa leaves: bb is the signer now
        assert!(devices.put(row("aa", "fcm", "", 12)).unwrap());
        assert!(!devices.put(row("aa", "fcm", "t3", 10)).unwrap());
        assert!(devices.tokens(15, "fcm").is_empty());
        assert_eq!(devices.accounts(), vec![(15, "bb".into())]);
        assert!(devices.put(row("bb", "voip", "", 13)).unwrap());
        assert!(devices.put(row("bb", "apns", "", 13)).unwrap());
        assert!(devices.accounts().is_empty());
        // one install's token registered under another key (the phone
        // seated another account): the old key's row is a tombstone, and
        // its registration replayed changes nothing
        assert!(devices.put(row("dd", "apns", "t4", 20)).unwrap());
        assert!(
            devices
                .put(Row {
                    account: 16,
                    ..row("ee", "apns", "t4", 21)
                })
                .unwrap()
        );
        assert!(devices.tokens(15, "apns").is_empty());
        assert_eq!(
            devices.tokens(16, "apns"),
            vec![("t4".to_owned(), "production".to_owned())]
        );
        assert!(!devices.put(row("dd", "apns", "t4", 20)).unwrap());
        assert_eq!(devices.accounts(), vec![(16, "ee".into())]);
    }

    /// The file is written whole and read back; one that moved is read
    /// again by a follower, one that did not is not.
    #[test]
    fn the_file_round_trips_and_a_follower_reads_it_when_it_moved() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("testkit").join("devices.json");
        let devices = Devices::load(&file).unwrap();
        assert!(devices.accounts().is_empty());
        let fcm = |account, key: &str, token: &str| Row {
            account,
            key: key.into(),
            kind: "fcm".into(),
            token: token.into(),
            env: String::new(),
            at: 1,
        };
        devices.put(fcm(2, "aa", "t2")).unwrap();
        let follower = Devices::load(&file).unwrap();
        assert_eq!(follower.accounts(), vec![(2, "aa".into())]);
        assert!(!follower.follow(&file));
        // a second within the same instant would read as unmoved on a
        // coarse filesystem clock
        std::thread::sleep(Duration::from_millis(20));
        devices.put(fcm(3, "bb", "t3")).unwrap();
        assert!(follower.follow(&file));
        assert_eq!(follower.accounts().len(), 2);
    }

    /// Registrations kept at once leave the file whole, with every row.
    #[test]
    fn saves_at_once_leave_the_file_whole() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("testkit").join("devices.json");
        let devices = Devices::load(&file).unwrap();
        std::thread::scope(|scope| {
            for account in 0..8 {
                let devices = &devices;
                scope.spawn(move || {
                    for at in 1..=20 {
                        devices
                            .put(Row {
                                account,
                                key: format!("key{account}"),
                                kind: "fcm".into(),
                                token: account.to_string().repeat(200),
                                env: String::new(),
                                at,
                            })
                            .unwrap();
                    }
                });
            }
        });
        assert_eq!(Devices::load(&file).unwrap().accounts().len(), 8);
    }

    /// The door and a network's process each hold the rows and each write
    /// the file: a token the process drops takes no registration the door
    /// kept since the process last read the file, and the door's next write
    /// does not bring the dropped token back.
    #[test]
    fn two_writers_keep_each_others_rows() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("testkit").join("devices.json");
        let row = |key: &str, token: &str| Row {
            account: 15,
            key: key.into(),
            kind: "apns".into(),
            token: token.into(),
            env: "production".into(),
            at: 1,
        };
        let door = Devices::load(&file).unwrap();
        door.put(row("aa", "dead")).unwrap();
        let child = Devices::load(&file).unwrap();
        door.put(row("bb", "new")).unwrap();
        assert!(child.drop_token("apns", "dead").unwrap());
        assert!(
            !child.drop_token("voip", "new").unwrap(),
            "another kind's row went"
        );
        door.put(row("cc", "newer")).unwrap();
        let kept = Devices::load(&file).unwrap().tokens(15, "apns");
        let tokens: Vec<_> = kept.iter().map(|(token, _)| token.as_str()).collect();
        assert_eq!(tokens, ["new", "newer"]);
    }

    /// A token APNs or FCM calls gone leaves a tombstone, as a leave does:
    /// the row's `at` still refuses a replay of the registration that put
    /// the token there.
    #[test]
    fn a_dropped_token_still_refuses_its_replay() {
        let devices = Devices::default();
        let row = Row {
            account: 15,
            key: "aa".into(),
            kind: "apns".into(),
            token: "dead".into(),
            env: "production".into(),
            at: 5,
        };
        assert!(devices.put(row.clone()).unwrap());
        assert!(devices.drop_token("apns", "dead").unwrap());
        assert!(devices.tokens(15, "apns").is_empty());
        assert!(
            !devices.put(row).unwrap(),
            "a replay brought the dropped token back"
        );
    }

    /// A network's process that drops a token reads its rows back at its
    /// next look, and says they moved (what runs `carry`), even when the
    /// file's clock did not tick between its read and its write.
    #[test]
    fn a_drop_here_is_a_move_the_next_look_sees() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("testkit").join("devices.json");
        let door = Devices::load(&file).unwrap();
        door.put(Row {
            account: 15,
            key: "aa".into(),
            kind: "apns".into(),
            token: "dead".into(),
            env: "production".into(),
            at: 1,
        })
        .unwrap();
        let child = Devices::load(&file).unwrap();
        let read = std::fs::metadata(&file).unwrap().modified().unwrap();
        assert!(child.drop_token("apns", "dead").unwrap());
        // a coarse clock: the write lands in the tick the read saw
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(read)
            .unwrap();
        assert!(child.follow(&file), "a drop here was no move");
        assert!(child.accounts().is_empty());
        assert!(!child.follow(&file));
    }

    /// A chain pushd does not follow is a 404, which a phone takes as final;
    /// while a followed node has not named its chain yet, the chain may be
    /// that one's: a 503, which the phone tries again.
    #[test]
    fn an_unfollowed_chain_is_a_404_only_once_every_node_named_its_chain() {
        let network = |chain: Option<&str>| {
            Arc::new(Followed {
                name: "testkit".into(),
                devices: Arc::new(Devices::default()),
                // nothing listens: a chain not named yet stays unnamed
                client: noded_client::Client::new("http://127.0.0.1:1"),
                chain: Mutex::new(chain.map(|chain| (chain.into(), "testkit".into()))),
            })
        };
        let registration = signed(voip(), &key(), crate::push::now_ms());
        let answer = |networks| {
            let door = Door::new(networks, Arc::new(key()));
            let refused =
                ducktape_host::runtime::handle().block_on(door.register(registration.clone()));
            refused.unwrap_err().0
        };
        let elsewhere = Some("elsewhere-0a1b2c3d");
        assert_eq!(answer(vec![network(elsewhere)]), 404);
        assert_eq!(answer(vec![network(elsewhere), network(None)]), 503);
    }

    /// A door on a loopback port, giving a client `deadline` for its
    /// headers and then its body: its address.
    fn door(networks: Vec<Arc<Followed>>, deadline: Duration) -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let at = listener.local_addr().unwrap();
        let door = Door {
            deadline,
            ..Door::new(networks, Arc::new(key()))
        };
        Arc::new(door).serve(listener).unwrap();
        at
    }

    /// `request` sent on a connection of its own: the status answered.
    fn sent(at: std::net::SocketAddr, request: &str) -> u16 {
        use std::io::{Read as _, Write as _};
        let mut stream = std::net::TcpStream::connect(at).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer[9..12].parse().unwrap()
    }

    fn post(client: &str, length: usize, body: &str) -> String {
        format!(
            "POST {ROUTE} HTTP/1.1\r\nhost: push\r\ncf-connecting-ip: {client}\r\n\
             content-length: {length}\r\nconnection: close\r\n\r\n{body}"
        )
    }

    /// A client past its registrations a minute is answered 429 before its
    /// body is read, and another client is not: the client is the address
    /// Cloudflare names (the peer is the tunnel on this host), an IPv6 one
    /// its /64. All clients together stop at the overall limit, and the
    /// counts hold no client past it.
    #[test]
    fn a_client_over_its_limit_is_told_to_wait() {
        let at = door(Vec::new(), DEADLINE);
        for _ in 0..PER_CLIENT {
            assert_eq!(sent(at, &post("203.0.113.7", 2, "{}")), 400);
        }
        assert_eq!(sent(at, &post("203.0.113.7", 2, "{}")), 429);
        assert_eq!(sent(at, &post("203.0.113.8", 2, "{}")), 400);
        for host in 0..PER_CLIENT {
            assert_eq!(sent(at, &post(&format!("2001:db8::{host}"), 2, "{}")), 400);
        }
        assert_eq!(sent(at, &post("2001:db8::ffff", 2, "{}")), 429);
        let mut counts = Counts::default();
        for n in 0..OVERALL {
            assert!(counts.take(IpAddr::from([10, 0, (n >> 8) as u8, n as u8])));
        }
        assert!(!counts.take(IpAddr::from([10, 1, 0, 0])));
        assert_eq!(counts.per.len(), OVERALL as usize);
    }

    /// A client that never finishes its headers is let go at the deadline
    /// (slowloris), and one whose body stalls is answered 408.
    #[test]
    fn a_client_that_holds_its_request_back_is_let_go() {
        use std::io::{Read as _, Write as _};
        let at = door(Vec::new(), Duration::from_millis(200));
        let mut stream = std::net::TcpStream::connect(at).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(format!("POST {ROUTE} HTTP/1.1\r\nhost: push\r\n").as_bytes())
            .unwrap();
        let ended = stream.read_to_end(&mut Vec::new());
        assert!(
            !matches!(&ended, Err(error) if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )),
            "the door held a connection whose headers never came: {ended:?}"
        );
        assert_eq!(sent(at, &post("203.0.113.7", 100, "{")), 408);
    }

    /// A node whose identity answers `OfKey` for each key as `answer` says,
    /// a reply (200) or a refusal (400): its URL.
    fn node(answer: fn(&[u8]) -> Result<identity::Reply, abi::Refusal>) -> String {
        use std::io::{BufRead as _, Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = std::io::BufReader::new(stream.unwrap());
                let (mut line, mut length) = (String::new(), 0);
                while stream.read_line(&mut line).unwrap() > 2 {
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap();
                    }
                    line.clear();
                }
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let query: noded_client::wire::Query = abi::decode(&body).unwrap();
                let frame: noded_client::wire::Frame = abi::decode(&query.frame).unwrap();
                let identity::Query::OfKey { key } = abi::decode(&frame.body.payload).unwrap()
                else {
                    panic!("the recheck asks OfKey first");
                };
                let (status, body) = match answer(&key) {
                    Ok(reply) => ("200 OK", abi::encode(&abi::encode(&reply))),
                    Err(refusal) => ("400 Bad Request", abi::encode(&refusal)),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/octet-stream\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let stream = stream.get_mut();
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        url
    }

    /// The hourly recheck drops a row only on identity's word that its key
    /// acts as no account (it holds none, or its account is suspended); a
    /// refusal that says nothing of the key (a body cut short, a trap)
    /// keeps the row.
    #[test]
    fn a_recheck_drops_a_leaver_and_keeps_a_row_the_node_did_not_judge() {
        let url = node(|key| match key[0] {
            0xaa => Ok(identity::Reply::Number(None)),
            0xbb => Err(abi::Refusal::new(
                abi::reason::UNAUTHORIZED,
                "account 16 is suspended",
            )),
            _ => Err(abi::Refusal::new(
                abi::reason::INVALID_INPUT,
                "the request body did not arrive whole",
            )),
        });
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("devices.json");
        let devices = Devices::load(&file).unwrap();
        for (account, key) in [(15, "aa"), (16, "bb"), (17, "cc")] {
            devices
                .put(Row {
                    account,
                    key: key.repeat(32),
                    kind: "fcm".into(),
                    token: format!("t{account}"),
                    env: String::new(),
                    at: 1,
                })
                .unwrap();
        }
        let network = Followed {
            name: "testkit".into(),
            devices: devices.clone(),
            client: noded_client::Client::new(url),
            chain: Mutex::new(Some(("testkit-0a1b2c3d".into(), "testkit".into()))),
        };
        let door = Door::new(vec![Arc::new(network)], Arc::new(key()));
        ducktape_host::runtime::handle().block_on(door.recheck());
        let kept = vec![(17, "cc".repeat(32))];
        assert_eq!(devices.accounts(), kept);
        assert_eq!(Devices::load(&file).unwrap().accounts(), kept);
    }
}
