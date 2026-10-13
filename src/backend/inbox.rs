//! The app's client of a notification inbox: the service in front of the
//! node at the RPC address the app connected to, answering
//! `<rpc>/inbox/v1/*` (`inbox_api`, the one copy of its shapes). JSON in
//! and out, a bearer token kept in memory for the connection's life, and
//! the `live` socket the pushes come over. Every call has a deadline, and
//! the socket is pinged when quiet, so a dead path is a failed call.
//!
//! Nothing here knows a program: an entry's `program` and `tag` are
//! whatever the inbox hands over.

use std::sync::Mutex;
use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use inbox_api::{
    Challenge, Changed, Entries, Hide, Login, LoginReply, LoginRequest, Mutes, Push, Read, Status,
};
use serde::de::DeserializeOwned;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::RpcClient;
use super::session::seated_sign;

/// The most an inbox answer may carry: a page of entries is a few hundred
/// KiB at most.
const MAX_BODY_BYTES: usize = 4 << 20;

/// The inbox's pace past its burst, one call every `PACE_EVERY_MS` per
/// account: what the pacer mirrors, and what a 429 waits once (the
/// backstop, for another device of the account spending the budget).
const PACE: Duration = Duration::from_millis(inbox_api::PACE_EVERY_MS);

/// How long a call may take to answer, the `live` upgrade included: past
/// it the path is dead, not slow (a page is a few hundred KiB).
const DEADLINE: Duration = Duration::from_secs(30);

/// The `live` socket quiet this long is pinged; quiet for another, with no
/// pong, it is dead (a laptop woke, a NAT forgot the path): the runner
/// connects again, `status` first.
const PING: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    /// `status` answered 404: this address keeps no inbox.
    NoInbox,
    /// Nothing reached the inbox, the transport failed, or the deadline
    /// passed.
    Unreachable(String),
    /// The inbox refused the login, or the session twice over (401).
    Denied(String),
    /// Any other status the inbox answered.
    Failed { status: u16, sentence: String },
    /// An answer that is not the JSON asked for.
    Decode(String),
}

impl Error {
    /// What went wrong, for a log line: never the inbox's sentence, which
    /// may name its origins.
    pub(crate) fn kind(&self) -> String {
        match self {
            Error::NoInbox => "no_inbox".into(),
            Error::Unreachable(_) => "unreachable".into(),
            Error::Denied(_) => "denied".into(),
            Error::Failed { status, .. } => format!("failed_{status}"),
            Error::Decode(_) => "decode".into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoInbox => write!(f, "this address keeps no inbox"),
            Error::Unreachable(sentence) | Error::Denied(sentence) | Error::Decode(sentence) => {
                write!(f, "{sentence}")
            }
            Error::Failed { status, sentence } => {
                write!(f, "the inbox answered {status}: {sentence}")
            }
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        // the sentence is logged: never the address, which may carry a
        // tunnel token or userinfo
        Error::Unreachable(error.without_url().to_string())
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// The session's bearer token: formats as nothing, so no log line, at any
/// level, can carry it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Token(String);

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(..)")
    }
}

pub(crate) struct Client {
    http: reqwest::Client,
    /// The RPC address as the app connected to it, no trailing `/`: what
    /// the login signs.
    origin: String,
    /// The chain id `<network>#<salt>`: what the login signs.
    chain: String,
    /// This device, as the inbox's session list names it.
    label: String,
    token: Mutex<Option<Token>>,
    /// One login at a time: a second caller whose token went stale waits
    /// and takes the first's.
    relogin: tokio::sync::Mutex<()>,
    /// Logged out: a call still in flight at the drop gets a 401 on the
    /// old token and must not sign in again.
    closed: std::sync::atomic::AtomicBool,
    /// The inbox's budget, kept here too: a call waits its turn instead
    /// of being refused.
    pacer: Mutex<Pacer>,
    deadline: Duration,
    ping: Duration,
}

/// The inbox's budget per account (`PACE_BURST` at once, then one every
/// `PACE_EVERY_MS`), as the client sees it: `due` is when the burst is
/// spent, moved by every call; a call is sent at `due - burst`.
struct Pacer {
    due: Option<std::time::Instant>,
}

impl Pacer {
    /// How long the next call waits before it may go.
    fn turn(&mut self, now: std::time::Instant) -> Duration {
        let burst = PACE * (inbox_api::PACE_BURST as u32 - 1);
        let due = self.due.map_or(now, |due| due.max(now));
        self.due = Some(due + PACE);
        due.saturating_duration_since(now + burst)
    }
}

impl Client {
    /// The inbox under the node `rpc` is connected to, for `chain`.
    pub(crate) fn new(
        rpc: &RpcClient,
        chain: impl Into<String>,
        label: impl Into<String>,
    ) -> Client {
        Client {
            http: rpc.http().clone(),
            // the one origin rule, the inbox's own: what the login signs
            origin: inbox_api::origin(rpc.endpoint()).unwrap_or_else(|_| rpc.endpoint().to_owned()),
            chain: chain.into(),
            label: label.into(),
            token: Mutex::new(None),
            relogin: tokio::sync::Mutex::new(()),
            closed: std::sync::atomic::AtomicBool::new(false),
            pacer: Mutex::new(Pacer { due: None }),
            deadline: DEADLINE,
            ping: PING,
        }
    }

    /// A test's deadline and ping interval, in place of the real ones.
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, deadline: Duration, ping: Duration) -> Client {
        self.deadline = deadline;
        self.ping = ping;
        self
    }

    fn url(&self, call: &str) -> String {
        format!("{}{}/{call}", self.origin, inbox_api::PATH)
    }

    fn token(&self) -> Option<Token> {
        self.token.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn set_token(&self, token: Option<Token>) {
        *self.token.lock().unwrap_or_else(|p| p.into_inner()) = token;
    }

    async fn answered<T: DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T> {
        answered(request, self.deadline).await
    }

    /// This call's turn under the budget: at once within the burst, then
    /// one every `PACE`.
    async fn turn(&self) {
        let wait = self
            .pacer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .turn(std::time::Instant::now());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// `status`; [`Error::NoInbox`] at a 404.
    pub(crate) async fn status(&self) -> Result<Status> {
        match self.answered(self.http.get(self.url("status"))).await {
            Err(Error::Failed { status: 404, .. }) => Err(Error::NoInbox),
            answer => answer,
        }
    }

    /// `challenge`, the device key's signature over the inbox-api bytes
    /// for this origin and chain, then `login`: the token is kept here.
    pub(crate) async fn login(&self) -> Result<LoginReply> {
        let challenge: Challenge = self.answered(self.http.post(self.url("challenge"))).await?;
        let nonce: [u8; 32] = abi::unhex(&challenge.nonce)
            .and_then(|nonce| nonce.try_into().ok())
            .ok_or_else(|| Error::Decode("the challenge's nonce is not 32 hex bytes".into()))?;
        let login = Login {
            origin: self.origin.clone(),
            chain: self.chain.clone(),
            nonce,
            expires: challenge.expires,
        };
        let (pubkey, signature) = seated_sign(inbox_api::LOGIN_NAMESPACE, &login.preimage())
            .await
            .map_err(|refusal| Error::Denied(refusal.message))?;
        let request = LoginRequest {
            pubkey: abi::hex(&pubkey),
            signature: abi::hex(&signature),
            nonce: challenge.nonce,
            expires: challenge.expires,
            label: self.label.clone(),
        };
        let reply: LoginReply = match self
            .answered(json(self.http.post(self.url("login")), &request))
            .await
        {
            Err(Error::Failed {
                status: 401,
                sentence,
            }) => return Err(Error::Denied(sentence)),
            answer => answer?,
        };
        self.set_token(Some(Token(reply.token.clone())));
        Ok(reply)
    }

    /// The session's token, logging in when there is none, or when the one
    /// in hand is the `stale` one a call was just refused with. One login
    /// at a time: a caller that waited takes the token the first got.
    async fn fresh_token(&self, stale: Option<&Token>) -> Result<Token> {
        let _one = self.relogin.lock().await;
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::Denied("this session was logged out".into()));
        }
        if let Some(token) = self.token()
            && Some(&token) != stale
        {
            return Ok(token);
        }
        self.set_token(None);
        Ok(Token(self.login().await?.token))
    }

    /// Ends the session on the inbox: the token is forgotten here.
    pub(crate) async fn logout(&self) -> Result<()> {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        let Some(token) = self.token() else {
            return Ok(());
        };
        self.set_token(None);
        self.turn().await;
        self.answered(self.http.post(self.url("logout")).bearer_auth(&token.0))
            .await
    }

    /// A call under the session's token. A 401 logs in again once and
    /// asks again; a second one is [`Error::Denied`]. A 429 (the inbox's
    /// pacing: 20 calls at once, then one every 200 ms) waits [`PACE`]
    /// and sends once more; a second one is the answer, never a spin.
    async fn authed<T: DeserializeOwned>(
        &self,
        request: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
    ) -> Result<T> {
        let mut stale: Option<Token> = None;
        let mut paced = false;
        loop {
            let token = self.fresh_token(stale.as_ref()).await?;
            self.turn().await;
            let sent = request(&self.http).bearer_auth(&token.0);
            match self.answered(sent).await {
                Err(Error::Failed {
                    status: 401,
                    sentence,
                }) => {
                    if stale.is_some() {
                        return Err(Error::Denied(sentence));
                    }
                    stale = Some(token);
                }
                Err(Error::Failed {
                    status: 429,
                    sentence,
                }) => {
                    if paced {
                        return Err(Error::Failed {
                            status: 429,
                            sentence,
                        });
                    }
                    paced = true;
                    tokio::time::sleep(PACE).await;
                }
                answer => return answer,
            }
        }
    }

    /// Newest first, below `before` (the newest when `None`).
    pub(crate) async fn entries(&self, before: Option<i64>, limit: u32) -> Result<Entries> {
        let mut url = format!("{}?limit={limit}", self.url("entries"));
        if let Some(before) = before {
            url.push_str(&format!("&before={before}"));
        }
        self.authed(|http| http.get(&url)).await
    }

    pub(crate) async fn read(&self, read: &Read) -> Result<Changed> {
        let url = self.url("read");
        self.authed(|http| json(http.post(&url), read)).await
    }

    /// Hides every read entry, on every device.
    pub(crate) async fn hide_read(&self) -> Result<Changed> {
        let url = self.url("hide");
        self.authed(|http| json(http.post(&url), &Hide { read: true }))
            .await
    }

    pub(crate) async fn mutes(&self) -> Result<Mutes> {
        let url = self.url("prefs");
        self.authed(|http| http.get(&url)).await
    }

    /// Replaces the account's mutes.
    pub(crate) async fn set_mutes(&self, mutes: &Mutes) -> Result<Mutes> {
        let url = self.url("prefs");
        self.authed(|http| json(http.put(&url), mutes)).await
    }

    /// The `live` socket, opened under the token; a 401 logs in again once,
    /// as any call does.
    pub(crate) async fn live(&self) -> Result<Live> {
        let mut stale: Option<Token> = None;
        loop {
            let token = self.fresh_token(stale.as_ref()).await?;
            self.turn().await;
            match self.upgrade(&token).await {
                Err(Error::Failed { status: 401, .. }) if stale.is_none() => stale = Some(token),
                Err(Error::Failed {
                    status: 401,
                    sentence,
                }) => return Err(Error::Denied(sentence)),
                Err(error) => return Err(error),
                Ok(socket) => {
                    return Ok(Live {
                        socket,
                        ping: self.ping,
                        pinged: false,
                    });
                }
            }
        }
    }

    async fn upgrade(&self, token: &Token) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        let url = self.url("live").replacen("http", "ws", 1);
        let mut request = url
            .into_client_request()
            .map_err(|error| Error::Unreachable(error.to_string()))?;
        let bearer = format!("Bearer {}", token.0)
            .parse()
            .map_err(|_| Error::Decode("the token is not a header value".into()))?;
        request.headers_mut().insert("Authorization", bearer);
        let (socket, _) =
            tokio::time::timeout(self.deadline, tokio_tungstenite::connect_async(request))
                .await
                .map_err(|_| Error::Unreachable("the live socket did not open in time".into()))?
                .map_err(|error| match error {
                    tokio_tungstenite::tungstenite::Error::Http(response) => Error::Failed {
                        status: response.status().as_u16(),
                        sentence: String::new(),
                    },
                    error => Error::Unreachable(error.to_string()),
                })?;
        Ok(socket)
    }
}

/// The `live` socket: each push as the inbox sends it, until the socket
/// closes or goes dead.
#[derive(Debug)]
pub(crate) struct Live {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ping: Duration,
    /// A ping is out, unanswered.
    pinged: bool,
}

impl Live {
    /// The next push; `None` when the inbox closed the socket. Quiet for
    /// `ping`, the socket is pinged; quiet for another, it is dead:
    /// [`Error::Unreachable`].
    pub(crate) async fn next(&mut self) -> Result<Option<Push>> {
        loop {
            let frame = match tokio::time::timeout(self.ping, self.socket.next()).await {
                Ok(frame) => frame,
                Err(_quiet) => {
                    if self.pinged {
                        return Err(Error::Unreachable(
                            "the live socket answered no ping".into(),
                        ));
                    }
                    self.pinged = true;
                    self.socket
                        .send(Message::Ping(Vec::new()))
                        .await
                        .map_err(|error| Error::Unreachable(error.to_string()))?;
                    continue;
                }
            };
            // any frame is a live path
            self.pinged = false;
            match frame {
                None => return Ok(None),
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str(&text)
                        .map(Some)
                        .map_err(|error| Error::Decode(error.to_string()));
                }
                Some(Ok(Message::Close(_))) => {
                    return Err(Error::Unreachable(
                        "the inbox closed the live socket".into(),
                    ));
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(Error::Unreachable(error.to_string())),
            }
        }
    }
}

fn json<B: serde::Serialize>(
    request: reqwest::RequestBuilder,
    body: &B,
) -> reqwest::RequestBuilder {
    request
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(body).expect("an in-memory value encodes"))
}

/// The answer as JSON, within `deadline`; a status other than 2xx is
/// [`Error::Failed`] with the inbox's sentence.
async fn answered<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    deadline: Duration,
) -> Result<T> {
    tokio::time::timeout(deadline, answer(request))
        .await
        .map_err(|_| Error::Unreachable("the inbox did not answer in time".into()))?
}

async fn answer<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T> {
    let mut response = request.send().await?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(Error::Decode("the inbox's answer is over 4 MiB".into()));
        }
        body.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(Error::Failed {
            status: status.as_u16(),
            sentence: String::from_utf8_lossy(&body).into_owned(),
        });
    }
    if body.is_empty() {
        // a 204: the caller asked for `()`
        return serde_json::from_str("null").map_err(|error| Error::Decode(error.to_string()));
    }
    serde_json::from_slice(&body).map_err(|error| Error::Decode(error.to_string()))
}

#[cfg(test)]
pub(crate) mod fake;

#[cfg(test)]
mod tests {
    use super::fake::FakeInbox;
    use super::*;
    use commonware_cryptography::{Signer as _, ed25519::PrivateKey};
    use inbox_api::Read;
    use node_wire::keyscheme::KeyScheme;

    /// The login signs exactly inbox-api's bytes for the origin the app
    /// connected to (no trailing `/`) and the chain, under the login
    /// namespace: the fake verifies with the shared shape.
    #[tokio::test]
    async fn login_signs_the_inbox_api_bytes_for_the_origin_and_chain() {
        let _seat = crate::backend::seat_serial();
        let key = PrivateKey::from_seed(11);
        let pubkey = key.public_key().as_ref().to_vec();
        crate::backend::seat_key(key).await;
        let fake = FakeInbox::start().await;
        let rpc = RpcClient::new(format!("{}/", fake.origin()));
        let client = Client::new(&rpc, "net#0a0b0c0d", "Desktop");
        let reply = client.login().await.unwrap();
        assert_eq!(reply.account, 7);
        let (request, nonce, expires) = fake.last_login();
        assert_eq!(abi::unhex(&request.pubkey).unwrap(), pubkey);
        assert_eq!(request.label, "Desktop");
        let signed = Login {
            origin: fake.origin(),
            chain: "net#0a0b0c0d".into(),
            nonce,
            expires,
        };
        assert!(KeyScheme::Ed25519.verify(
            &pubkey,
            inbox_api::LOGIN_NAMESPACE,
            &signed.preimage(),
            &abi::unhex(&request.signature).unwrap(),
        ));
        // a different origin would not verify: the bytes name this one
        let other = Login {
            origin: format!("{}/", fake.origin()),
            ..signed
        };
        assert!(!KeyScheme::Ed25519.verify(
            &pubkey,
            inbox_api::LOGIN_NAMESPACE,
            &other.preimage(),
            &abi::unhex(&request.signature).unwrap(),
        ));
    }

    /// A 404 at `status` is "no inbox here"; any other failure is not.
    #[tokio::test]
    async fn a_404_status_means_no_inbox() {
        let fake = FakeInbox::start().await;
        let rpc = RpcClient::new(fake.origin());
        let client = Client::new(&rpc, "net#0", "d");
        assert_eq!(client.status().await.unwrap().chain, "net#0");
        fake.set_no_inbox(true);
        assert_eq!(client.status().await.unwrap_err(), Error::NoInbox);
        let gone = RpcClient::new("http://127.0.0.1:1");
        assert!(matches!(
            Client::new(&gone, "net#0", "d").status().await.unwrap_err(),
            Error::Unreachable(_)
        ));
    }

    /// A 401 on a call logs in again once and the call goes through; a
    /// second 401 in a row is denied, not a loop.
    #[tokio::test]
    async fn a_401_logs_in_again_once() {
        let _seat = crate::backend::seat_serial();
        crate::backend::seat_key(PrivateKey::from_seed(12)).await;
        let fake = FakeInbox::start().await;
        let rpc = RpcClient::new(fake.origin());
        let client = Client::new(&rpc, "net#0", "d");
        client.login().await.unwrap();
        fake.expire_tokens();
        let changed = client.read(&Read::All { all: true }).await.unwrap();
        assert_eq!(changed.ids, Vec::<i64>::new());
        assert_eq!(fake.logins(), 2);
        fake.deny_logins(true);
        fake.expire_tokens();
        assert!(matches!(
            client.read(&Read::All { all: true }).await.unwrap_err(),
            Error::Denied(_)
        ));
        assert_eq!(fake.logins(), 3, "one more login, not a loop");
    }

    /// A 429 is waited out once and the call sent again; two in a row are
    /// the answer, not a spin.
    #[tokio::test]
    async fn a_429_waits_once_then_sends_once() {
        let _seat = crate::backend::seat_serial();
        crate::backend::seat_key(PrivateKey::from_seed(14)).await;
        let fake = FakeInbox::start().await;
        let rpc = RpcClient::new(fake.origin());
        let client = Client::new(&rpc, "net#0", "d");
        client.login().await.unwrap();
        fake.refuse_pace(1);
        let started = std::time::Instant::now();
        client.read(&Read::All { all: true }).await.unwrap();
        assert!(started.elapsed() >= PACE, "the second send waited");
        assert_eq!(fake.reads().len(), 1);
        fake.refuse_pace(2);
        assert!(matches!(
            client.read(&Read::All { all: true }).await.unwrap_err(),
            Error::Failed { status: 429, .. }
        ));
        assert_eq!(fake.reads().len(), 1, "two 429s in a row end the call");
    }

    /// The live socket carries the token and streams the fake's pushes.
    #[tokio::test]
    async fn live_streams_pushes_under_the_token() {
        let _seat = crate::backend::seat_serial();
        crate::backend::seat_key(PrivateKey::from_seed(13)).await;
        let fake = FakeInbox::start().await;
        let rpc = RpcClient::new(fake.origin());
        let client = Client::new(&rpc, "net#0", "d");
        let mut live = client.live().await.unwrap();
        fake.push(Push::Read { ids: vec![4] });
        assert_eq!(
            live.next().await.unwrap(),
            Some(Push::Read { ids: vec![4] })
        );
        assert_eq!(fake.logins(), 1);
        // a 401 on the upgrade logs in again once, as any call does
        fake.expire_tokens();
        let mut live = client.live().await.unwrap();
        assert_eq!(fake.logins(), 2);
        fake.push(Push::Read { ids: vec![5] });
        assert_eq!(
            live.next().await.unwrap(),
            Some(Push::Read { ids: vec![5] })
        );
        fake.deny_logins(true);
        fake.expire_tokens();
        assert!(matches!(client.live().await.unwrap_err(), Error::Denied(_)));
        assert_eq!(fake.logins(), 3, "one more login, not a loop");
    }

    /// A quiet socket is pinged and kept while the inbox answers; one that
    /// answers no ping is dead within two intervals.
    #[tokio::test]
    async fn a_live_socket_that_answers_no_ping_is_dead() {
        let _seat = crate::backend::seat_serial();
        crate::backend::seat_key(PrivateKey::from_seed(15)).await;
        let fake = FakeInbox::start().await;
        let rpc = RpcClient::new(fake.origin());
        let client = Client::new(&rpc, "net#0", "d")
            .with_clock(Duration::from_secs(5), Duration::from_millis(100));
        let mut live = client.live().await.unwrap();
        // three intervals of quiet, pinged and answered: still live
        let kept = tokio::time::timeout(Duration::from_millis(350), live.next()).await;
        assert!(kept.is_err(), "a pinged socket that pongs stays open");
        fake.set_quiet(true);
        let started = std::time::Instant::now();
        assert!(matches!(
            live.next().await.unwrap_err(),
            Error::Unreachable(_)
        ));
        assert!(started.elapsed() < Duration::from_millis(400));
    }

    /// A call past the deadline is unreachable, not a wait without end.
    #[tokio::test]
    async fn a_call_has_a_deadline() {
        let _seat = crate::backend::seat_serial();
        crate::backend::seat_key(PrivateKey::from_seed(16)).await;
        let fake = FakeInbox::start().await;
        let rpc = RpcClient::new(fake.origin());
        let client = Client::new(&rpc, "net#0", "d").with_clock(Duration::from_millis(100), PING);
        client.login().await.unwrap();
        fake.hold_reads(2_000);
        let started = std::time::Instant::now();
        assert!(matches!(
            client.read(&Read::All { all: true }).await.unwrap_err(),
            Error::Unreachable(_)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// Past the burst, calls go one every `PACE`: a burst of 25 reads takes
    /// five paces, and the inbox refuses none.
    #[tokio::test]
    async fn calls_past_the_burst_are_paced() {
        let _seat = crate::backend::seat_serial();
        crate::backend::seat_key(PrivateKey::from_seed(18)).await;
        let fake = FakeInbox::start().await;
        let rpc = RpcClient::new(fake.origin());
        let client = std::sync::Arc::new(Client::new(&rpc, "net#0", "d"));
        client.login().await.unwrap();
        fake.set_budget(true);
        let started = std::time::Instant::now();
        let calls: Vec<_> = (0..inbox_api::PACE_BURST + 5)
            .map(|_| {
                let client = client.clone();
                tokio::spawn(async move { client.read(&Read::All { all: true }).await })
            })
            .collect();
        for call in calls {
            call.await.unwrap().unwrap();
        }
        let took = started.elapsed();
        assert!(took >= PACE * 5, "five calls past the burst, one a pace");
        assert!(took < PACE * 9, "the burst went at once");
        assert_eq!(fake.reads().len() as u64, inbox_api::PACE_BURST + 5);
    }

    /// The signed origin is the one rule's: the host lowercased, the
    /// default port dropped, no trailing `/`.
    #[test]
    fn the_signed_origin_follows_the_one_rule() {
        let rpc = RpcClient::new("https://Duckpond.Example:443/");
        let client = Client::new(&rpc, "net#0", "d");
        assert_eq!(client.origin, "https://duckpond.example");
        assert_eq!(
            client.origin,
            inbox_api::origin("https://duckpond.example/").unwrap()
        );
    }

    /// The token formats as nothing; a logout ends the session and forgets it.
    #[tokio::test]
    async fn the_token_never_formats_and_a_logout_forgets_it() {
        let _seat = crate::backend::seat_serial();
        crate::backend::seat_key(PrivateKey::from_seed(17)).await;
        let fake = FakeInbox::start().await;
        let rpc = RpcClient::new(fake.origin());
        let client = Client::new(&rpc, "net#0", "d");
        let reply = client.login().await.unwrap();
        let token = client.token().unwrap();
        assert!(!format!("{token:?}").contains(&reply.token));
        client.logout().await.unwrap();
        assert_eq!((fake.logouts(), client.token()), (1, None));
        client.logout().await.unwrap();
        assert_eq!(fake.logouts(), 1, "nothing to end twice");
        // a call still in flight at the drop never signs in again
        assert!(matches!(
            client.read(&Read::All { all: true }).await.unwrap_err(),
            Error::Denied(_)
        ));
        assert_eq!(fake.logins(), 1);
    }
}
