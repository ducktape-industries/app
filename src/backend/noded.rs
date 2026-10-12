//! The app's client of a node's daemon: the `/v1` surface `noded` serves,
//! borsh in and out, over `node_wire`'s types (the one copy of the node's
//! wire every client reads; the signed frame is `node_wire::Frame`).
//!
//! Nothing here knows a program. A target is whatever string the caller
//! hands over.

use abi::{BlobId, Refusal};
use borsh::{BorshDeserialize, BorshSerialize};
use futures::{Stream, StreamExt as _};
use reqwest::StatusCode;
use tokio_tungstenite::tungstenite::Message;

#[cfg(test)]
pub use node_wire::Tx;
pub use node_wire::{
    BlockRef, Blocks, Body, Change, FRAME_NAMESPACE, Finalized, Frame, Layer, NODE_CONTRACT,
    Network, Query, Receipt, Status, Tip, route,
};

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

#[cfg(test)]
pub fn test_roles() -> abi::Roles {
    abi::Roles {
        registry: "module-registry".into(),
        validators: "valset".into(),
        identity: "identity".into(),
    }
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
    fn unframe_drops_the_git_header() {
        assert_eq!(unframe(b"blob 3\0abc"), Some(&b"abc"[..]));
        assert_eq!(unframe(b"no header"), None);
    }
}
