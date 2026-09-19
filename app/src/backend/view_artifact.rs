//! Fetch the view belonging to an already-selected registry deployment: a
//! module's embedded view, or a view-only artifact itself.
//!
//! The caller retains the expected hash and request generation, and rechecks
//! both before installing the result. This loader never selects a deployment
//! or substitutes a desktop resource after a failed fetch.
//!
//! A verified artifact is kept on disk under its code hash, per user
//! (`<cache_dir>/views/<hex code hash>`, the app's own cache directory), and
//! is hashed again on every read: a second connect, or another network
//! running the same code, fetches nothing, and a swap fetches only the hash
//! that changed. An entry that no longer hashes to its name is removed and
//! fetched again. At most [`FETCHES_AT_ONCE`] fetches share the link; the
//! view a tab is drawing never queues behind them.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ducktape_rpc::Client;
use module_artifact::{
    Artifact, ArtifactRef, MAX_ARTIFACT_BYTES, ModuleArtifact, ViewArtifact, ViewArtifactRef,
};
use sha2::{Digest as _, Sha256};

/// What an artifact frame is — a module (whose frame may embed a view) or
/// a view alone — as the frame's own tag says and the registry entry's
/// `kind` names, fixed at admission.
#[derive(serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Module,
    View,
}

#[derive(Debug)]
pub enum Error {
    /// The local node does not hold the blob: the bytes have not reached
    /// it (yet).
    NotHeld,
    Transport(String),
    HashMismatch,
    InvalidArtifact(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotHeld => formatter.write_str("view artifact not held by the node"),
            Self::Transport(error) => write!(formatter, "fetch view artifact: {error}"),
            Self::HashMismatch => formatter.write_str("view artifact deployment hash mismatch"),
            Self::InvalidArtifact(error) => write!(formatter, "invalid view artifact: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// One verified artifact frame, reduced to what a seat asks of it: what
/// it is, the identity of its consensus half, and its view. A module's
/// `core` is the hash of the same frame with its view removed — two
/// frames with equal cores run the same component and index byte for
/// byte, so a view read out of one is honest against the other's running
/// core. A view-only frame has no core.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub kind: Kind,
    pub core: Option<[u8; 32]>,
    pub view: Option<ViewArtifact>,
}

/// How many artifact fetches share the link at once: a slow link is not
/// split fifteen ways, and the view on screen skips the queue.
pub const FETCHES_AT_ONCE: usize = 3;

/// What a fetch tells the seat waiting for it, and asks of it.
pub struct Watch<'a> {
    /// Bytes in, of how many when the node said. `(n, Some(n))` is every
    /// byte in: verification is next.
    pub bytes: &'a (dyn Fn(u64, Option<u64>) + Sync),
    /// Whether a tab draws the seat now: its fetch does not wait its turn.
    pub on_screen: &'a (dyn Fn() -> bool + Sync),
}

impl Default for Watch<'_> {
    fn default() -> Self {
        Watch::UNSEEN
    }
}

impl Watch<'static> {
    /// Nobody watching: a background read, e.g. a taste's frame.
    pub const UNSEEN: Self = Self {
        bytes: &|_, _| {},
        on_screen: &|| false,
    };
}

/// The frame under `expected_hash`: from the `cache` directory if an entry
/// there still hashes to it, else fetched off the node, verified and kept
/// there. A 404 is [`Error::NotHeld`], bytes that do not hash to it
/// [`Error::HashMismatch`].
pub async fn fetch(
    cache: Option<&Path>,
    client: &Client,
    expected_hash: [u8; 32],
    watch: &Watch<'_>,
) -> Result<Frame, Error> {
    if let Some(frame) = cache.and_then(|dir| cached(dir, expected_hash, watch)) {
        return Ok(frame);
    }
    let _turn = link_turn(watch.on_screen).await;
    let bytes = download(client, expected_hash, watch.bytes).await?;
    let frame = verified_frame(&bytes, expected_hash)?;
    if let Some(dir) = cache
        && let Err(error) = keep(dir, expected_hash, &bytes)
    {
        tracing::warn!(target: "ducktape::app", error = %error, "view artifact not cached");
    }
    Ok(frame)
}

/// A turn on the link: one of [`FETCHES_AT_ONCE`], or none at all for a
/// seat a tab draws — checked again while it waits, so a tab opened onto a
/// queued view pulls it out of the queue.
async fn link_turn(
    on_screen: &(dyn Fn() -> bool + Sync),
) -> Option<tokio::sync::SemaphorePermit<'static>> {
    static LINK: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(FETCHES_AT_ONCE);
    let mut queued = std::pin::pin!(LINK.acquire());
    loop {
        if on_screen() {
            return None;
        }
        tokio::select! {
            turn = &mut queued => return turn.ok(),
            () = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

/// The blob's bytes, read as they arrive so the seat can say how many are in.
/// No overall deadline: a slow link that keeps sending finishes; one that
/// goes quiet for [`STALLED`] is unreachable.
async fn download(
    client: &Client,
    hash: [u8; 32],
    progress: &(dyn Fn(u64, Option<u64>) + Sync),
) -> Result<Vec<u8>, Error> {
    const STALLED: Duration = Duration::from_secs(30);
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let http = HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(STALLED)
            .read_timeout(STALLED)
            .build()
            .expect("an http client")
    });
    let transport = |error: reqwest::Error| Error::Transport(error.to_string());
    let url = format!("{}/v1/files/blob/{}", client.origin(), hex(&hash));
    let mut response = http.get(url).send().await.map_err(transport)?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::NotHeld);
    }
    if !status.is_success() {
        return Err(Error::Transport(format!("the node answered {status}")));
    }
    let total = response.content_length();
    let past_limit =
        || Error::Transport(format!("past the {MAX_ARTIFACT_BYTES} byte artifact limit"));
    if total.is_some_and(|total| total > MAX_ARTIFACT_BYTES as u64) {
        return Err(past_limit());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        if bytes.len() + chunk.len() > MAX_ARTIFACT_BYTES {
            return Err(past_limit());
        }
        bytes.extend_from_slice(&chunk);
        progress(bytes.len() as u64, total);
    }
    let received = bytes.len() as u64;
    progress(received, Some(received));
    Ok(bytes)
}

/// The cached frame under `hash`, hashed again as it is read. An entry that
/// does not verify is refused and removed, and the caller fetches it anew.
fn cached(dir: &Path, hash: [u8; 32], watch: &Watch<'_>) -> Option<Frame> {
    let path = dir.join(hex(&hash));
    let bytes = std::fs::read(&path).ok()?;
    (watch.bytes)(bytes.len() as u64, Some(bytes.len() as u64));
    match verified_frame(&bytes, hash) {
        Ok(frame) => Some(frame),
        Err(error) => {
            tracing::warn!(
                target: "ducktape::app",
                path = %path.display(),
                error = %error,
                "cached view artifact refused and removed"
            );
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

fn hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Writes a verified artifact under its hash: to a temporary name beside it,
/// then renamed over, so a reader never meets half an entry.
fn keep(dir: &Path, hash: [u8; 32], bytes: &[u8]) -> std::io::Result<()> {
    static TEMP: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(dir)?;
    let name = hex(&hash);
    let temp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let written =
        std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, dir.join(&name)));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

fn verified_frame(bytes: &[u8], expected_hash: [u8; 32]) -> Result<Frame, Error> {
    let actual_hash: [u8; 32] = Sha256::digest(bytes).into();
    if actual_hash != expected_hash {
        return Err(Error::HashMismatch);
    }
    // both arms carry a view the same way: a module's optional one, or the
    // view-only frame itself.
    let artifact = ArtifactRef::decode(bytes).map_err(Error::InvalidArtifact)?;
    Ok(match artifact {
        ArtifactRef::Module(module) => {
            let core = Artifact::Module(ModuleArtifact {
                component: module.component.to_vec(),
                index: module.index.map(<[u8]>::to_vec),
                view: None,
                // the lanes are the CORE's, not the view's: a view has no
                // sockets, and a deployment that asks for different lanes is
                // a different core even when its component bytes match.
                lanes: module.lanes,
            });
            Frame {
                kind: Kind::Module,
                core: Some(core.hash()),
                view: module.view.map(ViewArtifactRef::to_owned),
            }
        }
        ArtifactRef::View(view) => Frame {
            kind: Kind::View,
            core: None,
            view: Some(view.to_owned()),
        },
    })
}

/// Only a verified module artifact with no view is `Ok(None)`; a view-only
/// artifact always carries one.
#[cfg(test)]
fn verified_view(bytes: &[u8], expected_hash: [u8; 32]) -> Result<Option<ViewArtifact>, Error> {
    verified_frame(bytes, expected_hash).map(|frame| frame.view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_artifact::{Artifact, ModuleArtifact};

    #[test]
    fn a_verified_view_preserves_assets_and_removal_is_explicit() {
        let view = ViewArtifact {
            component: vec![4, 5, 6],
            assets: [("icons/action.svg".to_owned(), b"<svg/>".to_vec())].into(),
        };
        let artifact = Artifact::Module(ModuleArtifact {
            component: vec![1, 2, 3],
            index: None,
            view: Some(view.clone()),
            lanes: Vec::new(),
        });
        assert_eq!(
            verified_view(&artifact.encode(), artifact.hash()).unwrap(),
            Some(view.clone())
        );
        let removed = Artifact::Module(ModuleArtifact {
            component: vec![1, 2, 3],
            index: None,
            view: None,
            lanes: Vec::new(),
        });
        assert_eq!(
            verified_view(&removed.encode(), removed.hash()).unwrap(),
            None
        );
        let view_only = Artifact::View(view.clone());
        assert_eq!(
            verified_view(&view_only.encode(), view_only.hash()).unwrap(),
            Some(view)
        );
    }

    #[test]
    fn a_frames_core_is_the_module_without_its_view() {
        let view = ViewArtifact {
            component: vec![4, 5, 6],
            assets: Default::default(),
        };
        let without = Artifact::Module(ModuleArtifact {
            component: vec![1, 2, 3],
            index: Some(vec![9]),
            view: None,
            lanes: Vec::new(),
        });
        let with = Artifact::Module(ModuleArtifact {
            component: vec![1, 2, 3],
            index: Some(vec![9]),
            view: Some(view.clone()),
            lanes: Vec::new(),
        });
        let other_core = Artifact::Module(ModuleArtifact {
            component: vec![1, 2, 3, 4],
            index: Some(vec![9]),
            view: Some(view.clone()),
            lanes: Vec::new(),
        });
        let frame =
            |artifact: &Artifact| verified_frame(&artifact.encode(), artifact.hash()).unwrap();
        assert_eq!(frame(&with).core, Some(without.hash()));
        assert_eq!(frame(&without).core, Some(without.hash()));
        assert_ne!(frame(&other_core).core, frame(&with).core);
        let view_only = Artifact::View(view.clone());
        assert_eq!(
            frame(&view_only),
            Frame {
                kind: Kind::View,
                core: None,
                view: Some(view),
            }
        );
    }

    /// A node that answers `bytes` to every blob asked of it, counting the asks.
    async fn serving(bytes: Vec<u8>) -> (Client, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = asked.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut head = Vec::new();
                let mut byte = [0];
                while !head.ends_with(b"\r\n\r\n") && socket.read_exact(&mut byte).await.is_ok() {
                    head.push(byte[0]);
                }
                counted.fetch_add(1, Ordering::SeqCst);
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                let _ = socket.write_all(answer.as_bytes()).await;
                let _ = socket.write_all(&bytes).await;
            }
        });
        (client, asked)
    }

    /// The cache by code hash: a miss fetches and keeps the entry — written
    /// whole under its hash, nothing left beside it — a hit asks the node
    /// nothing, and an entry that no longer hashes to its name is refused,
    /// removed and fetched again.
    #[tokio::test]
    async fn the_cache_serves_a_verified_entry_and_refetches_a_corrupted_one() {
        let artifact = Artifact::module(vec![1, 2, 3]);
        let (hash, encoded) = (artifact.hash(), artifact.encode());
        let (client, asked) = serving(encoded.clone()).await;
        let cache = tempfile::tempdir().unwrap();
        let entry = cache.path().join(hex(&hash));
        let listed = || {
            std::fs::read_dir(cache.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect::<Vec<_>>()
        };

        let first = fetch(Some(cache.path()), &client, hash, &Watch::UNSEEN)
            .await
            .unwrap();
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        assert_eq!(listed(), [hex(&hash)], "a temporary was left behind");
        assert_eq!(std::fs::read(&entry).unwrap(), encoded);

        // a second connect, or another network running the same code
        let said = std::sync::Mutex::new(Vec::new());
        let watch = Watch {
            bytes: &|received, total| said.lock().unwrap().push((received, total)),
            on_screen: &|| false,
        };
        let hit = fetch(Some(cache.path()), &client, hash, &watch)
            .await
            .unwrap();
        assert_eq!(hit, first);
        assert_eq!(asked.load(Ordering::SeqCst), 1, "a hit asked the node");
        let length = encoded.len() as u64;
        assert_eq!(*said.lock().unwrap(), [(length, Some(length))]);

        let mut corrupted = encoded.clone();
        corrupted[5] ^= 1;
        std::fs::write(&entry, &corrupted).unwrap();
        assert_eq!(cached(cache.path(), hash, &Watch::UNSEEN), None);
        assert!(!entry.exists(), "the corrupted entry was not removed");
        std::fs::write(&entry, &corrupted).unwrap();
        let refetched = fetch(Some(cache.path()), &client, hash, &Watch::UNSEEN)
            .await
            .unwrap();
        assert_eq!(refetched, first);
        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(std::fs::read(&entry).unwrap(), encoded);
    }

    /// An entry is written beside its name and renamed over it: whatever was
    /// there is replaced whole, and no temporary outlives the write.
    #[test]
    fn a_cache_write_replaces_the_entry_whole() {
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join("views");
        let hash = [7; 32];
        keep(&dir, hash, b"first").unwrap();
        keep(&dir, hash, b"second, longer").unwrap();
        let listed: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(listed, [hex(&hash)]);
        assert_eq!(
            std::fs::read(dir.join(hex(&hash))).unwrap(),
            b"second, longer"
        );
    }

    #[test]
    fn tampering_and_malformed_artifacts_are_errors_not_removal() {
        let artifact = Artifact::module(vec![1, 2, 3]);
        let mut bytes = artifact.encode();
        bytes[5] ^= 1;
        assert!(matches!(
            verified_view(&bytes, artifact.hash()),
            Err(Error::HashMismatch)
        ));
        let malformed = b"not an artifact";
        assert!(matches!(
            verified_view(malformed, Sha256::digest(malformed).into()),
            Err(Error::InvalidArtifact(_))
        ));
    }
}
