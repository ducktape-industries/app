//! The app's client of a notification inbox: the service in front of the
//! node at the RPC address the app connected to, answering
//! `<rpc>/inbox/v1/*` (`inbox_api`, the one copy of its shapes). JSON in
//! and out, a bearer token kept in memory for the connection's life, and
//! the `live` socket the pushes come over.
//!
//! Nothing here knows a program: an entry's `program` and `tag` are
//! whatever the inbox hands over.

use std::sync::Mutex;

use futures::{Stream, StreamExt as _};
use inbox_api::{
    Challenge, Changed, Entries, Hide, Login, LoginReply, LoginRequest, Mutes, Push, Read, Status,
};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

use super::RpcClient;
use super::session::seated_sign;

/// The most an inbox answer may carry: a page of entries is a few hundred
/// KiB at most.
const MAX_BODY_BYTES: usize = 4 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    /// `status` answered 404: this address keeps no inbox.
    NoInbox,
    /// Nothing reached the inbox, or the transport failed.
    Unreachable(String),
    /// The inbox refused the login, or the session twice over (401).
    Denied(String),
    /// Any other status the inbox answered.
    Failed { status: u16, sentence: String },
    /// An answer that is not the JSON asked for.
    Decode(String),
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
        Error::Unreachable(error.to_string())
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

pub(crate) struct Client {
    http: reqwest::Client,
    /// The RPC address as the app connected to it, no trailing `/`: what
    /// the login signs.
    origin: String,
    /// The chain id `<network>#<salt>`: what the login signs.
    chain: String,
    /// This device, as the inbox's session list names it.
    label: String,
    token: Mutex<Option<String>>,
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
            origin: rpc.endpoint().to_owned(),
            chain: chain.into(),
            label: label.into(),
            token: Mutex::new(None),
        }
    }

    fn url(&self, call: &str) -> String {
        format!("{}{}/{call}", self.origin, inbox_api::PATH)
    }

    fn token(&self) -> Option<String> {
        self.token.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// `status`; [`Error::NoInbox`] at a 404.
    pub(crate) async fn status(&self) -> Result<Status> {
        match answered(self.http.get(self.url("status"))).await {
            Err(Error::Failed { status: 404, .. }) => Err(Error::NoInbox),
            answer => answer,
        }
    }

    /// `challenge`, the device key's signature over the inbox-api bytes
    /// for this origin and chain, then `login`: the token is kept here.
    pub(crate) async fn login(&self) -> Result<LoginReply> {
        let challenge: Challenge = answered(self.http.post(self.url("challenge"))).await?;
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
            label: self.label.clone(),
        };
        let reply: LoginReply =
            match answered(json(self.http.post(self.url("login")), &request)).await {
                Err(Error::Failed {
                    status: 401,
                    sentence,
                }) => return Err(Error::Denied(sentence)),
                answer => answer?,
            };
        *self.token.lock().unwrap_or_else(|p| p.into_inner()) = Some(reply.token.clone());
        Ok(reply)
    }

    /// A call under the session's token. A 401 logs in again once and
    /// asks again; a second one is [`Error::Denied`].
    async fn authed<T: DeserializeOwned>(
        &self,
        request: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
    ) -> Result<T> {
        for again in [false, true] {
            let token = match self.token() {
                Some(token) => token,
                None => self.login().await?.token,
            };
            let sent = request(&self.http).bearer_auth(&token);
            match answered(sent).await {
                Err(Error::Failed {
                    status: 401,
                    sentence,
                }) => {
                    *self.token.lock().unwrap_or_else(|p| p.into_inner()) = None;
                    if again {
                        return Err(Error::Denied(sentence));
                    }
                }
                answer => return answer,
            }
        }
        unreachable!("the second pass answers or is denied")
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

    /// The `live` socket: each push as the inbox sends it, until the
    /// socket closes (an error ends the stream).
    pub(crate) async fn live(&self) -> Result<impl Stream<Item = Result<Push>> + Unpin> {
        let token = match self.token() {
            Some(token) => token,
            None => self.login().await?.token,
        };
        let url = self.url("live").replacen("http", "ws", 1);
        let mut request = url
            .into_client_request()
            .map_err(|error| Error::Unreachable(error.to_string()))?;
        let bearer = format!("Bearer {token}")
            .parse()
            .map_err(|_| Error::Decode("the token is not a header value".into()))?;
        request.headers_mut().insert("Authorization", bearer);
        let (socket, _) = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| Error::Unreachable("the live socket did not open in time".into()))?
        .map_err(|error| match error {
            tokio_tungstenite::tungstenite::Error::Http(response)
                if response.status() == StatusCode::UNAUTHORIZED =>
            {
                *self.token.lock().unwrap_or_else(|p| p.into_inner()) = None;
                Error::Denied("the inbox ended this session".into())
            }
            error => Error::Unreachable(error.to_string()),
        })?;
        Ok(socket.filter_map(|message| {
            let push = match message {
                Ok(Message::Text(text)) => Some(
                    serde_json::from_str(&text).map_err(|error| Error::Decode(error.to_string())),
                ),
                Ok(Message::Close(_)) => Some(Err(Error::Unreachable(
                    "the inbox closed the live socket".into(),
                ))),
                Ok(_) => None,
                Err(error) => Some(Err(Error::Unreachable(error.to_string()))),
            };
            futures::future::ready(push)
        }))
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

/// The answer as JSON; a status other than 2xx is [`Error::Failed`] with
/// the inbox's sentence.
async fn answered<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T> {
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
            live.next().await.unwrap().unwrap(),
            Push::Read { ids: vec![4] }
        );
        assert_eq!(fake.logins(), 1);
    }
}
