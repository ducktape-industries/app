//! A ducktape client's side of a node: the `/v1` surface `noded` serves
//! ([`wire`], [`Client`]), and the flows onto an account every client runs
//! the same way: the identity program's asks ([`identity`]), a new device
//! joining from one already on the account through the auth host's relay
//! ([`join`], [`relay`]), and the rule that binds a network's keys on this
//! device to one chain ([`keyring`]). The desktop app and the phone both
//! link it.
//!
//! It links no core crate. What signs is a [`Signer`]: the desktop's
//! device key (commonware's ed25519 key, feature `commonware`) or the
//! phone's, which never leaves the phone's key store. The sentences the
//! flows answer are product copy both apps show.

pub mod endpoint;
pub mod fs;
pub mod hex;
pub mod identity;
pub mod ids;
pub mod join;
pub mod keyring;
mod net;
pub mod relay;
pub mod wire;
pub mod words;

use abi::{BlobId, Refusal};
use borsh::{BorshDeserialize, BorshSerialize};
use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use std::time::Duration;

use wire::{
    BlockAsk, Blocks, Change, Finalized, Layer, Located, Network, Page, Preconfirm, Query, Receipt,
    Status, Submit, Submitted, TxAsk, route,
};

/// What signs a frame or a consent: an ed25519 key, wherever it is kept.
/// `sign` answers the ed25519 signature over commonware's
/// `union_unique(namespace, message)`, which is what the node checks a
/// frame's proof and identity a consent against, or why it could not sign
/// (the key is locked, the key store refused). `public_key` cannot fail: a
/// signer is made for one key, known when it is made, so a frame never
/// names one key and carries another's proof.
pub trait Signer: Send + Sync {
    fn public_key(&self) -> [u8; 32];
    fn sign(&self, namespace: &[u8], message: &[u8]) -> std::result::Result<[u8; 64], SignError>;
}

/// Why a [`Signer`] did not sign, in words for the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignError(pub String);

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SignError {}

/// Feature `commonware`: commonware-cryptography links aws-lc-rs on
/// x86_64 and aarch64 whatever its features, which an Android build cannot
/// compile, so a client whose key is another (the phone's) leaves it off.
#[cfg(feature = "commonware")]
impl Signer for commonware_cryptography::ed25519::PrivateKey {
    fn public_key(&self) -> [u8; 32] {
        let key = commonware_cryptography::Signer::public_key(self);
        key.as_ref()
            .try_into()
            .expect("an ed25519 public key is 32 bytes")
    }

    /// Never refuses: the key is in memory.
    fn sign(&self, namespace: &[u8], message: &[u8]) -> std::result::Result<[u8; 64], SignError> {
        let signature = commonware_cryptography::Signer::sign(self, namespace, message);
        Ok(signature
            .as_ref()
            .try_into()
            .expect("an ed25519 signature is 64 bytes"))
    }
}

/// A blob read's connect deadline: a node that has not taken the
/// connection (TCP and TLS) in 10 s is not reached; the handshake takes
/// well under a second even on a weak phone link.
const READ_CONNECT: Duration = Duration::from_secs(10);

/// A blob read's idle deadline: 30 s with no byte at all ends it, as the
/// live link's own idle does (`backend::live`); it starts again on every
/// byte, so a slow blob that keeps coming never trips it.
const READ_IDLE: Duration = Duration::from_secs(30);

/// [`Error::Failed`]'s sentence when what answered is not a node: its body is someone else's page.
pub const NOT_A_NODE: &str = "this address does not answer as a ducktape node";

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
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused(refusal) | Error::Decode(refusal) => {
                write!(f, "{}: {}", refusal.reason, refusal.sentence)
            }
            Error::Failed { status, sentence } => write!(f, "node answered {status}: {sentence}"),
            Error::Unreachable(sentence) | Error::Transport(sentence) => write!(f, "{sentence}"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct Client {
    http: net::Net,
    reads: net::Net,
    base: String,
}

impl Client {
    pub fn new(base: impl Into<String>) -> Client {
        let base = base.into().trim_end_matches('/').to_owned();
        Client {
            http: net::Net::new(None),
            reads: net::Net::new(Some((READ_CONNECT, READ_IDLE))),
            base,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.base
    }

    pub(crate) fn http(&self) -> &net::Net {
        &self.http
    }

    pub async fn status(&self) -> Result<Status> {
        self.fetch(route::STATUS).await
    }

    /// Every member's sync state as the node sees it.
    pub async fn network(&self) -> Result<Network> {
        self.fetch(route::NETWORK).await
    }

    pub async fn live_port(&self) -> Result<Option<u16>> {
        self.fetch(route::LIVE_PORT).await
    }

    pub async fn preconfirm(&self, frame: Vec<u8>) -> Result<Receipt> {
        let submit = Submit {
            frame,
            preconfirm: Preconfirm::On,
        };
        match self.post(route::SUBMIT, &submit).await? {
            Submitted::Preconfirmed(receipt) => Ok(receipt),
            Submitted::Pooled => Err(Error::Decode(Refusal::new(
                abi::reason::PROTOCOL,
                "the node pooled a frame this app asked it to preconfirm",
            ))),
        }
    }

    pub async fn query(&self, layer: Layer, frame: Vec<u8>) -> Result<Vec<u8>> {
        self.post(route::QUERY, &Query { layer, frame }).await
    }

    /// The framed bytes (`kind len\0body`) of a blob the node holds, read
    /// under the reads' deadlines ([`READ_CONNECT`], [`READ_IDLE`]).
    pub async fn blob(&self, id: BlobId) -> Result<Option<Vec<u8>>> {
        answered(
            self.reads
                .post(&self.url(route::BLOB_GET), abi::encode(&id)),
        )
        .await
    }

    /// [`Client::blob`]'s framed bytes in the pieces they come in
    /// ([`BlobStream::next`]), never held whole; `None` when the node holds
    /// no such blob.
    pub async fn blob_stream(&self, id: BlobId) -> Result<Option<BlobStream>> {
        let url = self.url(route::BLOB_GET);
        let asked = self.reads.post(&url, abi::encode(&id));
        let mut response = accepted(asked.await?).await?;
        // borsh's `Option<Vec<u8>>`: a tag, the length (u32 LE), the bytes;
        // the head is cut off the first pieces, what follows it kept as it came
        let mut head = Vec::new();
        let mut first = Bytes::new();
        while head.len() < 5 && head.first() != Some(&0) {
            let Some(mut chunk) = response.chunk().await? else {
                break;
            };
            let took = chunk.len().min(5 - head.len());
            head.extend_from_slice(&chunk.split_to(took));
            first = chunk;
        }
        match head[..] {
            [0] => match response.chunk().await? {
                None => Ok(None),
                Some(_) => Err(undecoded("the blob answer runs past its end")),
            },
            [1, a, b, c, d, ..] => Ok(Some(BlobStream {
                left: u32::from_le_bytes([a, b, c, d]) as usize,
                first,
                response,
            })),
            _ => Err(undecoded("the blob answer did not decode")),
        }
    }

    pub async fn blobs_stream(&self, ids: &[BlobId]) -> Result<BlobsStream> {
        let url = self.url(route::BLOBS);
        let asked = self.reads.post(&url, abi::encode(&ids.to_vec()));
        BlobsStream::of(accepted(asked.await?).await?.pieces()).await
    }

    /// A blob upload: `frame`, a signed blobs-role `Put` naming each blob's
    /// id, length and kind, then each blob it names, framed
    /// (`kind len\0body`), in its order, sent as `blobs` yields them, so
    /// neither side holds the bundle whole. The node stages the bytes and
    /// pools the frame unpredicted ([`Submitted::Pooled`]): the bundle is put
    /// once a block carries the frame.
    pub async fn put_blobs(
        &self,
        frame: Vec<u8>,
        blobs: impl Stream<Item = std::io::Result<Vec<u8>>> + rt::MaybeSend + 'static,
    ) -> Result<Submitted> {
        let opening = [&(frame.len() as u32).to_le_bytes()[..], &frame].concat();
        let body = futures::stream::once(async { Ok(opening) }).chain(blobs);
        answered(self.http.post_stream(&self.url(route::BLOB_PUT), body)).await
    }

    pub async fn blocks(&self, page: &Blocks) -> Result<Page> {
        self.post(route::BLOCKS, page).await
    }

    pub async fn block(&self, ask: &BlockAsk) -> Result<Option<Finalized>> {
        self.post(route::BLOCK, ask).await
    }

    pub async fn tx(&self, ask: &TxAsk) -> Result<Option<Located>> {
        self.post(route::TX, ask).await
    }

    /// Each block's writes to `program`, values and all.
    pub async fn changes(
        &self,
        program: &str,
    ) -> Result<impl Stream<Item = Result<Change>> + Unpin> {
        self.follow(format!("{}/{program}", route::CHANGES)).await
    }

    /// Each block's writes to `program`, the keys alone (`?keys`): every
    /// value `None`.
    pub async fn changed_keys(
        &self,
        program: &str,
    ) -> Result<impl Stream<Item = Result<Change>> + Unpin> {
        self.follow(format!("{}/{program}?keys", route::CHANGES))
            .await
    }

    async fn follow(&self, path: String) -> Result<impl Stream<Item = Result<Change>> + Unpin> {
        let url = format!("{}{path}", self.base.replacen("http", "ws", 1));
        let socket = self.http.socket(&url).await?;
        Ok(socket
            .map(|message| message.and_then(|bytes| abi::decode(&bytes).map_err(Error::Decode))))
    }

    fn url(&self, route: &str) -> String {
        format!("{}{route}", self.base)
    }

    async fn fetch<T: BorshDeserialize>(&self, route: &str) -> Result<T> {
        answered(self.http.get(&self.url(route))).await
    }

    async fn post<B: BorshSerialize, T: BorshDeserialize>(
        &self,
        route: &str,
        body: &B,
    ) -> Result<T> {
        self.post_raw(route, abi::encode(body)).await
    }

    async fn post_raw<T: BorshDeserialize>(&self, route: &str, body: Vec<u8>) -> Result<T> {
        answered(self.http.post(&self.url(route), body)).await
    }
}

/// A blob read in pieces ([`Client::blob_stream`]).
pub struct BlobStream {
    response: net::Answer,
    /// The bytes still to come, as the answer said.
    left: usize,
    /// What came with the answer's head.
    first: Bytes,
}

impl BlobStream {
    /// The next piece of the framed bytes, as the connection read it (a
    /// piece is up to the connection's read buffer, ~400 KB, and a copy
    /// would be a second one on the reader's thread); `None` once all of
    /// them came. Fewer than the answer said, or more, fail as an answer
    /// that does not decode.
    pub async fn next(&mut self) -> Result<Option<Bytes>> {
        let piece = match std::mem::take(&mut self.first) {
            first if !first.is_empty() => first,
            _ => match self.response.chunk().await? {
                Some(chunk) => chunk,
                None if self.left == 0 => return Ok(None),
                None => return Err(undecoded("the blob ended before its length")),
            },
        };
        self.left = self
            .left
            .checked_sub(piece.len())
            .ok_or_else(|| undecoded("the blob runs past its length"))?;
        Ok(Some(piece))
    }
}

pub struct BlobsStream {
    chunks: rt::BoxStream<'static, Result<Bytes>>,
    buffered: Bytes,
    blobs: usize,
    left: usize,
}

impl BlobsStream {
    async fn of(chunks: rt::BoxStream<'static, Result<Bytes>>) -> Result<Self> {
        let mut blobs = BlobsStream {
            chunks,
            buffered: Bytes::new(),
            blobs: 0,
            left: 0,
        };
        blobs.blobs = u32::from_le_bytes(blobs.head::<4>().await?) as usize;
        Ok(blobs)
    }

    pub async fn next_blob(&mut self) -> Result<Option<bool>> {
        while self.piece().await?.is_some() {}
        if self.blobs == 0 {
            return match self.filled().await? {
                true => Err(undecoded("the blobs answer runs past its end")),
                false => Ok(None),
            };
        }
        self.blobs -= 1;
        match self.head::<1>().await? {
            [0] => Ok(Some(false)),
            [1] => {
                self.left = u32::from_le_bytes(self.head::<4>().await?) as usize;
                Ok(Some(true))
            }
            _ => Err(undecoded("the blobs answer did not decode")),
        }
    }

    pub async fn piece(&mut self) -> Result<Option<Bytes>> {
        if self.left == 0 {
            return Ok(None);
        }
        if !self.filled().await? {
            return Err(undecoded("the blob ended before its length"));
        }
        let piece = self.buffered.split_to(self.buffered.len().min(self.left));
        self.left -= piece.len();
        Ok(Some(piece))
    }

    async fn head<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut head = [0; N];
        let mut at = 0;
        while at < N {
            if !self.filled().await? {
                return Err(undecoded("the blobs answer ended inside an item's head"));
            }
            let took = self.buffered.len().min(N - at);
            head[at..at + took].copy_from_slice(&self.buffered.split_to(took));
            at += took;
        }
        Ok(head)
    }

    async fn filled(&mut self) -> Result<bool> {
        while self.buffered.is_empty() {
            match self.chunks.next().await {
                Some(chunk) => self.buffered = chunk?,
                None => return Ok(false),
            }
        }
        Ok(true)
    }
}

fn undecoded(sentence: &str) -> Error {
    Error::Decode(Refusal::new(abi::reason::PROTOCOL, sentence))
}

async fn answered<T: BorshDeserialize>(
    sent: impl std::future::Future<Output = Result<net::Answer>>,
) -> Result<T> {
    let body = accepted(sent.await?).await?.bytes().await?;
    abi::decode(&body).map_err(Error::Decode)
}

async fn accepted(answer: net::Answer) -> Result<net::Answer> {
    let status = answer.status();
    if status == 200 {
        return Ok(answer);
    }
    let plain = answer.plain();
    let body = answer.bytes().await?;
    Err(refusal(status, plain, &body))
}

pub(crate) fn refusal(status: u16, plain: bool, body: &[u8]) -> Error {
    match status {
        400 => match abi::decode(body) {
            Ok(refusal) => Error::Refused(refusal),
            Err(refusal) => Error::Decode(refusal),
        },
        status => Error::Failed {
            status,
            sentence: match plain || body.is_empty() {
                true => String::from_utf8_lossy(body).into_owned(),
                false => NOT_A_NODE.into(),
            },
        },
    }
}

/// A key that signs with fixed bytes, for the tests that read a frame's
/// shape, not its proof.
#[cfg(test)]
pub(crate) struct FixedSigner;

#[cfg(test)]
impl Signer for FixedSigner {
    fn public_key(&self) -> [u8; 32] {
        [1; 32]
    }

    fn sign(&self, _: &[u8], _: &[u8]) -> std::result::Result<[u8; 64], SignError> {
        Ok([2; 64])
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    /// One request off a socket, its body read whole whether sent by
    /// length or chunked (an upload is chunked), then `answer` with 200.
    async fn served_once(listener: tokio::net::TcpListener, answer: Vec<u8>) -> (String, Vec<u8>) {
        use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = tokio::io::BufReader::new(stream);
        let mut head = String::new();
        let (mut length, mut chunked) = (0, false);
        loop {
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            if line == "\r\n" {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                length = value.trim().parse().unwrap();
            }
            chunked |= lower.starts_with("transfer-encoding:") && lower.contains("chunked");
            head.push_str(&line);
        }
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        while chunked {
            let mut size = String::new();
            stream.read_line(&mut size).await.unwrap();
            let size = usize::from_str_radix(size.trim(), 16).unwrap();
            let mut chunk = vec![0; size + 2];
            stream.read_exact(&mut chunk).await.unwrap();
            body.extend_from_slice(&chunk[..size]);
            chunked = size > 0;
        }
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            answer.len()
        );
        stream.write_all(reply.as_bytes()).await.unwrap();
        stream.write_all(&answer).await.unwrap();
        (head, body)
    }

    /// An upload is the frame's length (u32 LE), the frame, then each blob
    /// as it came, in order, to `/v1/blob/put`; the node's `Submitted` is
    /// the answer.
    #[tokio::test]
    async fn a_blob_upload_is_the_frame_by_its_length_then_each_blob_in_order() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::new(format!("http://{}", listener.local_addr().unwrap()));
        let served = tokio::spawn(served_once(listener, abi::encode(&Submitted::Pooled)));
        let blobs = [b"chunk 3\0abc".to_vec(), b"chunk 0\0".to_vec()];
        let submitted = client
            .put_blobs(vec![7; 300], futures::stream::iter(blobs.clone().map(Ok)))
            .await
            .unwrap();
        assert_eq!(submitted, Submitted::Pooled);
        let (head, body) = served.await.unwrap();
        assert!(head.starts_with("POST /v1/blob/put "), "{head}");
        let mut sent = 300u32.to_le_bytes().to_vec();
        sent.extend([7; 300]);
        sent.extend(blobs.concat());
        assert_eq!(body, sent);
    }

    /// A client whose blob reads give up after `idle` with no byte, and
    /// the node it asks: a socket whose one connection `answer` serves.
    async fn reading<F: Future<Output = ()> + Send + 'static>(
        idle: Duration,
        answer: impl FnOnce(tokio::net::TcpStream) -> F + Send + 'static,
    ) -> Client {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client {
            reads: net::Net::new(Some((Duration::from_secs(5), idle))),
            ..Client::new(format!("http://{}", listener.local_addr().unwrap()))
        };
        tokio::spawn(async move { answer(listener.accept().await.unwrap().0).await });
        client
    }

    /// A blob read the node takes and never answers gives up once nothing
    /// has come for the idle deadline: a transport failure, not a wait for
    /// good.
    #[tokio::test]
    async fn a_blob_read_gives_up_on_a_node_that_says_nothing() {
        let client = reading(Duration::from_millis(200), |stream| async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(stream);
        })
        .await;
        let read =
            tokio::time::timeout(Duration::from_secs(5), client.blob(BlobId::Sha256([0; 32])))
                .await
                .expect("the read never gave up");
        assert!(matches!(read, Err(Error::Transport(_))), "{read:?}");
    }

    // A body cut short after its head says why, not only that the body did
    // not read: reqwest's causes come with its own words.
    #[tokio::test]
    async fn a_body_cut_short_says_its_cause() {
        use tokio::io::AsyncWriteExt as _;
        let client = reading(Duration::from_secs(5), |mut stream| async move {
            let head = "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n";
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(&[1; 10]).await.unwrap();
        })
        .await;
        let read = client.blob(BlobId::Sha256([0; 32])).await;
        let Err(Error::Transport(sentence)) = read else {
            panic!("{read:?}");
        };
        let step = "error decoding response body: ";
        assert!(sentence.starts_with(step), "{sentence}");
        assert!(sentence.len() > step.len(), "{sentence}");
    }

    /// A blob that keeps coming, a byte at a time, for longer than the idle
    /// deadline is read whole: the deadline is on silence, not on the read.
    #[tokio::test]
    async fn a_slow_blob_that_keeps_coming_is_read_whole() {
        use tokio::io::AsyncWriteExt as _;
        let blob = abi::encode(&Some(vec![7u8; 6]));
        let sent = blob.clone();
        let client = reading(Duration::from_millis(200), |mut stream| async move {
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", sent.len());
            stream.write_all(head.as_bytes()).await.unwrap();
            for byte in sent {
                tokio::time::sleep(Duration::from_millis(80)).await;
                stream.write_all(&[byte]).await.unwrap();
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        })
        .await;
        let read = client.blob(BlobId::Sha256([0; 32])).await;
        assert_eq!(read.ok(), Some(Some(vec![7u8; 6])));
    }

    /// A blob streamed is the framed bytes as they come, in pieces; one
    /// that ends before the length its answer gave fails, never reads as
    /// whole.
    #[tokio::test]
    async fn a_blob_streams_in_pieces_and_a_short_one_fails() {
        use tokio::io::AsyncWriteExt as _;
        let answer = |sent: Vec<u8>| {
            move |mut stream: tokio::net::TcpStream| async move {
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", sent.len());
                stream.write_all(head.as_bytes()).await.unwrap();
                for piece in sent.chunks(3) {
                    stream.write_all(piece).await.unwrap();
                    stream.flush().await.unwrap();
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        };
        let read = |client: Client| async move {
            let mut stream = client.blob_stream(BlobId::Sha256([0; 32])).await?.unwrap();
            let mut pieces = Vec::new();
            while let Some(piece) = stream.next().await? {
                pieces.push(piece);
            }
            Ok::<_, Error>(pieces)
        };
        let blob = b"program 9\0\0asm\x01\0\0\0!".to_vec();
        let whole = abi::encode(&Some(blob.clone()));
        let client = reading(Duration::from_secs(5), answer(whole.clone())).await;
        let pieces = read(client).await.unwrap();
        assert!(pieces.len() > 1, "{pieces:?}");
        assert_eq!(pieces.concat(), blob);
        let short = whole[..whole.len() - 2].to_vec();
        let client = reading(Duration::from_secs(5), answer(short)).await;
        assert!(matches!(read(client).await, Err(Error::Decode(_))));
        let none = reading(
            Duration::from_secs(5),
            answer(abi::encode(&None::<Vec<u8>>)),
        )
        .await;
        assert!(
            none.blob_stream(BlobId::Sha256([0; 32]))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_blobs_answer_reads_the_same_whatever_its_pieces() {
        let (first, second) = (b"program 3\0abc".to_vec(), b"program 1\0z".to_vec());
        let answer = [
            3u32.to_le_bytes().to_vec(),
            abi::encode(&Some(first.clone())),
            vec![0],
            abi::encode(&Some(second.clone())),
        ]
        .concat();
        let streamed = |bytes: Vec<u8>, size: usize| {
            let chunks: Vec<Result<Bytes>> = bytes
                .chunks(size)
                .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
                .collect();
            BlobsStream::of(futures::stream::iter(chunks).boxed())
        };
        for size in 1..=answer.len() {
            let mut blobs = streamed(answer.clone(), size).await.unwrap();
            let mut read = Vec::new();
            while let Some(held) = blobs.next_blob().await.unwrap() {
                let mut bytes = Vec::new();
                while let Some(piece) = blobs.piece().await.unwrap() {
                    bytes.extend_from_slice(&piece);
                }
                read.push(held.then_some(bytes));
            }
            assert_eq!(
                read,
                [Some(first.clone()), None, Some(second.clone())],
                "pieces of {size}"
            );
        }
        let mut cut = streamed(answer[..answer.len() - 1].to_vec(), 4)
            .await
            .unwrap();
        assert_eq!(cut.next_blob().await.unwrap(), Some(true));
        assert_eq!(cut.next_blob().await.unwrap(), Some(false));
        assert_eq!(cut.next_blob().await.unwrap(), Some(true));
        let ended = loop {
            match cut.piece().await {
                Ok(Some(_)) => continue,
                other => break other,
            }
        };
        assert!(matches!(ended, Err(Error::Decode(_))));
        let mut longer = streamed([answer.clone(), vec![9]].concat(), 5)
            .await
            .unwrap();
        for _ in 0..3 {
            longer.next_blob().await.unwrap();
        }
        assert!(matches!(longer.next_blob().await, Err(Error::Decode(_))));
    }

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
}
