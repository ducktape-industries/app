//! The app's client of a node's daemon: the `/v1` surface `noded` serves,
//! borsh in and out, and the signed frame every write and query carries.
//!
//! The types here mirror `crates/noded/src/wire.rs` and
//! `crates/kernel/node/src/frame.rs` on the kernel branch field for field.
//! They are copied, not linked, because the daemon's crates carry the whole
//! kernel (host, state, consensus) and a desktop client has no use for it.
//! ponytail: a `noded-wire` leaf crate in ducktape would end the copy; ask
//! for one when a field moves.
//!
//! Nothing here knows a program. A target is whatever string the caller
//! hands over.

use abi::{BlobId, ProgramId, Refusal, Root};
use borsh::{BorshDeserialize, BorshSerialize};
use commonware_cryptography::{Signer as _, ed25519};
use futures::{Stream, StreamExt as _};
use reqwest::StatusCode;
use tokio_tungstenite::tungstenite::Message;

pub const NODE_CONTRACT: u32 = 1;
pub const FRAME_NAMESPACE: &[u8] = b"ducktape:frame";

/// The most one node answer may carry, unless the call names its own cap
/// (a blob's): a view's reply budget holds no more, so a bigger body is
/// refused before it is read, not after.
pub const MAX_BODY_BYTES: usize = 32 << 20;

/// How long a connection to a node (or the auth host) may take to open.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a caller waits for one node answer before it stops waiting. Not
/// the client's own timeout, and above the hold a recovering node puts on a
/// request: it accepts the connection and answers once it has recovered
/// (app#189's run held its queries ~94 s: node up 13:14:49.9, recovered
/// 13:16:22.0, serving 13:16:23.6), so this is that hold and as much again.
pub const ANSWER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(180);

/// Every HTTP client the app builds: the opening bounded, the answer not
/// (see [`ANSWER_DEADLINE`]), and no redirect followed. A redirect would
/// send the same signed frame on to whatever host the answer names; it is
/// the answer instead, a status this client does not read
/// ([`Error::Failed`], `node_failed`).
pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("the HTTP client")
}

pub mod route {
    pub const STATUS: &str = "/v1/status";
    pub const SUBMIT: &str = "/v1/submit";
    pub const QUERY: &str = "/v1/query";
    pub const GET: &str = "/v1/get";
    pub const BLOB_GET: &str = "/v1/blob/get";
    pub const CHANGES: &str = "/v1/changes";
    pub const BLOCKS: &str = "/v1/blocks";
    pub const BLOCK: &str = "/v1/block";
    pub const NETWORK: &str = "/v1/network";
}

/// `host::Layer`: which state a read sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Layer {
    Confirmed,
    Preconfirmed,
}

/// The signer's scheme, as a frame body encodes it: the device key is
/// `Ed25519`, a passkey `Secp256r1` (`backend::passkey`).
pub use keyscheme::KeyScheme;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Body {
    pub scheme: KeyScheme,
    pub signer: Vec<u8>,
    pub network: Vec<u8>,
    pub seq: u64,
    pub target: String,
    pub payload: Vec<u8>,
}

impl Body {
    /// What the signer signs under [`FRAME_NAMESPACE`].
    pub fn preimage(&self) -> Vec<u8> {
        abi::encode(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Frame {
    pub body: Body,
    pub proof: Vec<u8>,
}

impl Frame {
    pub fn sign(
        key: &ed25519::PrivateKey,
        network: &[u8],
        seq: u64,
        target: &str,
        payload: Vec<u8>,
    ) -> Frame {
        let body = Body {
            scheme: KeyScheme::Ed25519,
            signer: key.public_key().as_ref().to_vec(),
            network: network.to_vec(),
            seq,
            target: target.to_owned(),
            payload,
        };
        let proof = key
            .sign(FRAME_NAMESPACE, &body.preimage())
            .as_ref()
            .to_vec();
        Frame { body, proof }
    }

    pub fn encode(&self) -> Vec<u8> {
        abi::encode(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Status {
    pub network: String,
    /// The chain's founding time (its genesis block's), unix ms — not now.
    /// With `network`, what `key_dir::bind_keyring` tells two chains apart by.
    pub time: u64,
    pub block_time_ms: u64,
    pub epoch_length: u64,
    pub height: u64,
    pub tip: [u8; 32],
    pub root: Root,
    pub epoch: u64,
    pub identity: Vec<u8>,
    pub contract: u32,
    /// The genesis block's digest: with the network name, the chain's id.
    pub genesis: [u8; 32],
}

/// Every member of the current epoch as the node sees it (`/v1/network`):
/// its applied tip when it answered, and the members in key order. No
/// member's `signed` exceeds `height`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Network {
    pub height: u64,
    pub members: Vec<PeerStatus>,
}

/// One member: the newest block the node applied that this key sent a
/// finalize vote for, as the node's consensus engine heard it. A vote for a
/// block not applied yet counts once it is. `None` for a resident, for a
/// validator not heard since the node started or began validating, and for
/// every member while the node is not itself seated as a validator: it runs
/// no engine, so it hears no votes.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PeerStatus {
    pub key: Vec<u8>,
    pub signed: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Query {
    pub layer: Layer,
    pub frame: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Get {
    pub layer: Layer,
    pub program: ProgramId,
    pub key: Vec<u8>,
}

/// `host::Receipt`: what a submit answers.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Receipt {
    pub program: ProgramId,
    pub outcome: abi::Outcome,
    pub events: Vec<Vec<u8>>,
    /// The runs this one's messages caused, in order.
    pub nested: Vec<Receipt>,
}

/// One block's writes to one program, as `/v1/changes/<program>` streams them.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Change {
    pub height: u64,
    pub root: Root,
    pub writes: Vec<(Vec<u8>, Option<Vec<u8>>)>,
}

/// `wire::Blocks`: a page of finalized blocks, newest first.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Blocks {
    pub before: Option<u64>,
    pub limit: u32,
}

/// `wire::BlockRef`: a finalized block by height or by id.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum BlockRef {
    Height(u64),
    Id([u8; 32]),
}

/// `wire::Tx`: one applied frame; `hash` is sha256 of the frame's bytes.
/// `receipt` is its run, where the node kept the block's receipts (none
/// below a state-sync anchor).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Tx {
    pub hash: [u8; 32],
    pub signer: Vec<u8>,
    pub seq: u64,
    pub target: String,
    pub payload: Vec<u8>,
    pub receipt: Option<Receipt>,
}

/// `wire::Finalized`: a block as the node's marshal archive keeps it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Finalized {
    pub height: u64,
    pub id: [u8; 32],
    pub parent: [u8; 32],
    pub time: u64,
    pub epoch: u64,
    pub proposer: Option<Vec<u8>>,
    pub txs: Vec<Tx>,
}

#[derive(Debug)]
pub enum Error {
    /// the node refused: a decoded `Refusal` (HTTP 400)
    Refused(Refusal),
    /// the node failed or answered a status this client does not read
    Failed { status: u16, sentence: String },
    /// no connection was made: nothing reached the node
    Unreachable(String),
    /// the transport failed after the request may have gone out: whether
    /// the node acted on it is not known
    Transport(String),
    /// the body did not decode as the type asked for
    Decode(Refusal),
    /// the answer is longer than the call reads: refused, never buffered
    TooLarge { cap: usize },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused(refusal) | Error::Decode(refusal) => {
                write!(f, "{}: {}", refusal.reason, refusal.sentence)
            }
            Error::Failed { status, sentence } => write!(f, "node answered {status}: {sentence}"),
            Error::Unreachable(sentence) | Error::Transport(sentence) => write!(f, "{sentence}"),
            Error::TooLarge { cap } => write!(
                f,
                "The node's answer is over the {} MiB this app reads.",
                cap >> 20
            ),
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        match error.is_connect() {
            true => Error::Unreachable(error.to_string()),
            false => Error::Transport(error.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    pub fn new(base: impl Into<String>) -> Client {
        let _timed = crate::perf::time(crate::perf::Key::Shell, "io.rpc_client");
        let base = base.into().trim_end_matches('/').to_owned();
        Client { http: http(), base }
    }

    pub fn endpoint(&self) -> &str {
        &self.base
    }

    /// The connection's HTTP client, for a call to another host on its
    /// behalf (the auth host's relay): one pool per connection, not one per
    /// call.
    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub async fn status(&self) -> Result<Status> {
        self.fetch(route::STATUS).await
    }

    /// Every member's sync state as the node sees it.
    pub async fn network(&self) -> Result<Network> {
        self.fetch(route::NETWORK).await
    }

    /// A signed frame, as bytes; the node answers the receipt of its execute.
    pub async fn submit(&self, frame: Vec<u8>) -> Result<Receipt> {
        self.post_raw(route::SUBMIT, frame).await
    }

    /// A signed frame whose target answers a query; the reply is whatever
    /// bytes the program `Respond`ed.
    pub async fn query(&self, layer: Layer, frame: Vec<u8>) -> Result<Vec<u8>> {
        self.post(route::QUERY, &Query { layer, frame }).await
    }

    pub async fn get(&self, layer: Layer, program: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let get = Get {
            layer,
            program: program.to_owned(),
            key: key.to_vec(),
        };
        self.post(route::GET, &get).await
    }

    /// The framed bytes (`kind len\0body`) of a blob the node holds; an
    /// answer over `cap` bytes is refused [`Error::TooLarge`].
    pub async fn blob(&self, id: BlobId, cap: usize) -> Result<Option<Vec<u8>>> {
        answered(
            self.http
                .post(self.url(route::BLOB_GET))
                .body(abi::encode(&id)),
            cap,
        )
        .await
    }

    /// Finalized blocks, newest first: below `before` (the tip when
    /// `None`), at most `limit` (the node caps it).
    pub async fn blocks(&self, page: &Blocks) -> Result<Vec<Finalized>> {
        self.post(route::BLOCKS, page).await
    }

    pub async fn block(&self, by: &BlockRef) -> Result<Option<Finalized>> {
        self.post(route::BLOCK, by).await
    }

    pub async fn changes(
        &self,
        program: &str,
    ) -> Result<impl Stream<Item = Result<Change>> + Unpin> {
        let url = format!(
            "{}{}/{program}",
            self.base.replacen("http", "ws", 1),
            route::CHANGES
        );
        let (socket, _) =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url))
                .await
                .map_err(|_| Error::Transport("the changes socket did not open in time".into()))?
                .map_err(|error| Error::Transport(error.to_string()))?;
        Ok(socket.filter_map(|message| {
            let change = match message {
                Ok(Message::Binary(bytes)) => Some(abi::decode(&bytes).map_err(Error::Decode)),
                Ok(_) => None,
                Err(error) => Some(Err(Error::Transport(error.to_string()))),
            };
            futures::future::ready(change)
        }))
    }

    fn url(&self, route: &str) -> String {
        format!("{}{route}", self.base)
    }

    async fn fetch<T: BorshDeserialize>(&self, route: &str) -> Result<T> {
        answered(self.http.get(self.url(route)), MAX_BODY_BYTES).await
    }

    async fn post<B: BorshSerialize, T: BorshDeserialize>(
        &self,
        route: &str,
        body: &B,
    ) -> Result<T> {
        self.post_raw(route, abi::encode(body)).await
    }

    async fn post_raw<T: BorshDeserialize>(&self, route: &str, body: Vec<u8>) -> Result<T> {
        answered(self.http.post(self.url(route)).body(body), MAX_BODY_BYTES).await
    }
}

/// `request`'s answer, read a chunk at a time and never past `cap` bytes:
/// a `Content-Length` over it is refused before the body is read, a body
/// that streams past it as soon as it does.
async fn answered<T: BorshDeserialize>(request: reqwest::RequestBuilder, cap: usize) -> Result<T> {
    let mut response = request.send().await?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > cap as u64)
    {
        return Err(Error::TooLarge { cap });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > cap {
            return Err(Error::TooLarge { cap });
        }
        body.extend_from_slice(&chunk);
    }
    match status {
        StatusCode::OK => abi::decode(&body).map_err(Error::Decode),
        StatusCode::BAD_REQUEST => Err(Error::Refused(abi::decode(&body).map_err(Error::Decode)?)),
        status => Err(Error::Failed {
            status: status.as_u16(),
            sentence: String::from_utf8_lossy(&body).into_owned(),
        }),
    }
}

/// Unframes a blob the node handed over: `kind len\0body` → body.
pub fn unframe(framed: &[u8]) -> Option<&[u8]> {
    let nul = framed.iter().position(|byte| *byte == 0)?;
    Some(&framed[nul + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wss_gets_tls_not_a_missing_feature() {
        // A port that hangs up: the TLS handshake fails, but only after the
        // connector exists. Without the rustls feature it is refused up front.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("wss://{}/v1/changes/x", port.local_addr().unwrap());
        std::thread::spawn(move || drop(port.accept()));
        let error = tokio_tungstenite::connect_async(url).await.unwrap_err();
        assert!(
            !matches!(
                error,
                tokio_tungstenite::tungstenite::Error::Url(
                    tokio_tungstenite::tungstenite::error::UrlError::TlsFeatureNotEnabled
                )
            ),
            "{error}"
        );
    }

    #[test]
    fn frame_signs_its_body_and_names_the_signer() {
        let key = ed25519::PrivateKey::from_seed(7);
        let frame = Frame::sign(&key, b"net", 3, "demo", vec![1, 2]);
        assert_eq!(frame.body.signer, key.public_key().as_ref().to_vec());
        assert_eq!(frame.body.scheme, KeyScheme::Ed25519);
        let decoded: Frame = abi::decode(&frame.encode()).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(frame.proof.len(), 64);
        assert!(KeyScheme::Ed25519.verify(
            &frame.body.signer,
            FRAME_NAMESPACE,
            &frame.body.preimage(),
            &frame.proof
        ));
    }

    /// The bytes `host::Receipt` encodes a submission with one nested run
    /// to: program "a", Applied [7], no events, nested [program "b",
    /// Applied [], no events, nested []].
    #[test]
    fn receipt_decodes_the_hosts_nested_runs() {
        let bytes = [
            1, 0, 0, 0, b'a', 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 1, 0, 0, 0, //
            1, 0, 0, 0, b'b', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let receipt: Receipt = abi::decode(&bytes).unwrap();
        assert_eq!(receipt.program, "a");
        assert_eq!(receipt.outcome, abi::Outcome::Applied { output: vec![7] });
        assert_eq!(receipt.nested.len(), 1);
        assert_eq!(receipt.nested[0].program, "b");
    }

    /// The bytes noded's own test pins `Network` to (noded/src/network.rs,
    /// `the_route_is_borsh_in_declaration_order`, core 4c12e1eac).
    #[test]
    fn network_decodes_the_nodes_bytes() {
        let mut bytes = Vec::new();
        bytes.extend(2u64.to_le_bytes());
        bytes.extend(2u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend([9, 1]);
        bytes.extend(1u64.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend([7, 0]);
        let network = Network {
            height: 2,
            members: vec![
                PeerStatus {
                    key: vec![9],
                    signed: Some(1),
                },
                PeerStatus {
                    key: vec![7],
                    signed: None,
                },
            ],
        };
        assert_eq!(abi::decode::<Network>(&bytes).unwrap(), network);
        assert_eq!(abi::encode(&network), bytes);
    }

    #[test]
    fn unframe_drops_the_git_header() {
        assert_eq!(unframe(b"blob 3\0abc"), Some(&b"abc"[..]));
        assert_eq!(unframe(b"no header"), None);
    }
}
