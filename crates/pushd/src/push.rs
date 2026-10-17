//! The `push` channel: a call to an account's phones (huddle call SPEC §5).
//!
//! A notice the centre lets through whose kind is a call
//! ([`NoticeKind::Call`]) goes to each of the account's devices: an
//! iPhone's PushKit token as an APNs VoIP push, an Android phone's Firebase
//! token as a high-priority FCM data message, both expiring at the call's
//! `until`. Taken down before then (the program's `notify.seen` of its
//! tag), it is withdrawn: a background push to an iPhone's APNs token (never
//! a second VoIP push, which iOS would ring), `kind: withdraw` to an Android
//! phone at normal priority. Past its `until` it is dropped: a phone stops ringing then on its
//! own, and pushd sends no call already past it. Every other notice goes on
//! to the `log` channel.
//!
//! Both carry one payload, `{kind, chain, program, tag, title, body, link,
//! until}` (FCM's data values are strings). pushd names no program: a call
//! is a call whoever posts it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use ducktape_host::contract::Banners;
use ducktape_host::runtime::notify::{Click, Notice, NoticeKind};
use ring::signature::{self, EcdsaKeyPair, RsaKeyPair};

use crate::Log;

/// One device of an account: its token and what kind it is.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct Device {
    /// `voip`: an iPhone's PushKit token (hex), for a call. `apns`: its
    /// remote-notification token (hex), for a withdraw. `fcm`: an Android
    /// phone's Firebase token, for both.
    pub kind: String,
    pub token: String,
}

/// `[apns]` in pushd.toml: the team's APNs auth key, kept out of any repo.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct ApnsConfig {
    /// The auth key's `.p8` file (PKCS#8 PEM).
    pub key_file: PathBuf,
    pub key_id: String,
    pub team_id: String,
    /// The app's bundle id; a VoIP push goes to `<topic>.voip`.
    pub topic: String,
    /// Apple's development server: for a build signed for development.
    #[serde(default)]
    pub sandbox: bool,
}

/// `[fcm]` in pushd.toml: the Firebase project's service account.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct FcmConfig {
    /// The service account's JSON key file.
    pub service_account: PathBuf,
}

/// One HTTP request a channel sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

/// What sends a [`Request`]: the status and the body that came back.
/// Blocks; a channel's calls run on the blocking pool.
pub trait Http: Send + Sync {
    fn send(&self, request: &Request) -> Result<(u16, String), String>;
}

/// The internet: HTTP/2, which APNs requires, through reqwest. Every
/// request gives up in bounded time: the channel's sends run one after
/// another in the program's queue, and one stuck connection would hold
/// every later ring past its `until`. Idle HTTP/2 connections are pinged,
/// so a dead one is found before a ring waits on it.
pub struct Client(reqwest::Client);

impl Client {
    pub fn new() -> Result<Self, String> {
        Self::timed(Duration::from_secs(10))
    }

    /// Each request given up after `timeout`, a connect after half of it.
    fn timed(timeout: Duration) -> Result<Self, String> {
        reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout / 2)
            .http2_keep_alive_interval(Duration::from_secs(30))
            .http2_keep_alive_timeout(Duration::from_secs(10))
            .http2_keep_alive_while_idle(true)
            .build()
            .map(Self)
            .map_err(|error| format!("no HTTP client: {error}"))
    }
}

impl Http for Client {
    fn send(&self, request: &Request) -> Result<(u16, String), String> {
        let mut post = self.0.post(&request.url);
        for (name, value) in &request.headers {
            post = post.header(*name, value);
        }
        ducktape_host::runtime::handle().block_on(async move {
            let answer = post
                .body(request.body.clone())
                .send()
                .await
                .map_err(|error| error.to_string())?;
            let status = answer.status().as_u16();
            Ok((status, answer.text().await.unwrap_or_default()))
        })
    }
}

/// A bearer token and when it is to be made again.
type Kept = Mutex<Option<(String, Instant)>>;

/// The APNs auth key, and the provider token signed with it.
pub struct Apns {
    key: EcdsaKeyPair,
    key_id: String,
    team_id: String,
    topic: String,
    host: &'static str,
    token: Kept,
}

impl Apns {
    pub fn load(config: &ApnsConfig) -> Result<Self, String> {
        let file = &config.key_file;
        let text = std::fs::read_to_string(file)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        let key = EcdsaKeyPair::from_pkcs8(
            &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &der(&text)?,
            &ring::rand::SystemRandom::new(),
        )
        .map_err(|error| format!("{}: not a P-256 auth key: {error}", file.display()))?;
        Ok(Self {
            key,
            key_id: config.key_id.clone(),
            team_id: config.team_id.clone(),
            topic: config.topic.clone(),
            host: match config.sandbox {
                true => "https://api.sandbox.push.apple.com",
                false => "https://api.push.apple.com",
            },
            token: Mutex::default(),
        })
    }

    /// The provider token (an ES256 JWT of the team, signed with the key),
    /// made again every 40 minutes: Apple takes one up to an hour old and
    /// refuses one made again within 20.
    fn bearer(&self, now: u64) -> Result<String, String> {
        kept(&self.token, || {
            let header = serde_json::json!({ "alg": "ES256", "kid": self.key_id });
            let claims = serde_json::json!({ "iss": self.team_id, "iat": now });
            let token = jwt(&header, &claims, |signing| {
                self.key
                    .sign(&ring::rand::SystemRandom::new(), signing)
                    .map(|signed| signed.as_ref().to_vec())
                    .map_err(|_| "the APNs key did not sign".to_string())
            })?;
            Ok((token, Duration::from_secs(40 * 60)))
        })
    }
}

/// What signs the service account's assertion: its RSA key (RS256).
type Signer = Box<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync>;

/// The Firebase service account, and the access token it is exchanged for.
pub struct Fcm {
    project: String,
    email: String,
    token_uri: String,
    sign: Signer,
    token: Kept,
}

#[derive(serde::Deserialize)]
struct ServiceAccount {
    project_id: String,
    client_email: String,
    private_key: String,
    token_uri: String,
}

impl Fcm {
    pub fn load(config: &FcmConfig) -> Result<Self, String> {
        let file = &config.service_account;
        let text = std::fs::read_to_string(file)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        let account: ServiceAccount =
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", file.display()))?;
        let key = RsaKeyPair::from_pkcs8(&der(&account.private_key)?)
            .map_err(|error| format!("{}: not an RSA key: {error}", file.display()))?;
        let sign: Signer = Box::new(move |signing| {
            let mut signed = vec![0; key.public().modulus_len()];
            key.sign(
                &signature::RSA_PKCS1_SHA256,
                &ring::rand::SystemRandom::new(),
                signing,
                &mut signed,
            )
            .map_err(|_| "the service account's key did not sign".to_string())?;
            Ok(signed)
        });
        Ok(Self {
            project: account.project_id,
            email: account.client_email,
            token_uri: account.token_uri,
            sign,
            token: Mutex::default(),
        })
    }

    /// An OAuth access token for FCM, from Google's token endpoint for an
    /// RS256 assertion of the service account; kept until a minute before
    /// it lapses.
    fn bearer(&self, http: &dyn Http, now: u64) -> Result<String, String> {
        kept(&self.token, || {
            let header = serde_json::json!({ "alg": "RS256", "typ": "JWT" });
            let claims = serde_json::json!({
                "iss": self.email,
                "scope": "https://www.googleapis.com/auth/firebase.messaging",
                "aud": self.token_uri,
                "iat": now,
                "exp": now + 3600,
            });
            let assertion = jwt(&header, &claims, &self.sign)?;
            let (status, body) = http.send(&Request {
                url: self.token_uri.clone(),
                headers: vec![(
                    "content-type",
                    "application/x-www-form-urlencoded".into(),
                )],
                body: format!(
                    "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}"
                ),
            })?;
            #[derive(serde::Deserialize)]
            struct Granted {
                access_token: String,
                expires_in: u64,
            }
            let granted: Granted = serde_json::from_str(&body)
                .map_err(|error| format!("token endpoint answered {status}: {error}"))?;
            let lasts = Duration::from_secs(granted.expires_in.saturating_sub(60));
            Ok((granted.access_token, lasts))
        })
    }
}

/// The token kept in `token`, or a new one from `make` when there is none
/// or it lapsed.
fn kept(
    token: &Kept,
    make: impl FnOnce() -> Result<(String, Duration), String>,
) -> Result<String, String> {
    let mut token = token
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((kept, until)) = &*token
        && Instant::now() < *until
    {
        return Ok(kept.clone());
    }
    let (made, lasts) = make()?;
    *token = Some((made.clone(), Instant::now() + lasts));
    Ok(made)
}

/// A JWT of `header` and `claims`, its signature `sign`'s over them.
fn jwt(
    header: &serde_json::Value,
    claims: &serde_json::Value,
    sign: impl FnOnce(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<String, String> {
    let url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let signing = format!(
        "{}.{}",
        url.encode(header.to_string()),
        url.encode(claims.to_string())
    );
    let signature = sign(signing.as_bytes())?;
    Ok(format!("{signing}.{}", url.encode(signature)))
}

/// The DER inside a PEM block.
fn der(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .map_err(|error| format!("not a PEM key: {error}"))
}

/// Unix seconds now.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The `push` channel of one account ([module docs](self)).
pub struct Push {
    /// Where every notice that is not a call goes.
    pub log: Log,
    /// The chain the payload names, as `duck://` links spell it.
    pub chain: String,
    // ponytail: the account's devices come from pushd.toml; M2's registry
    // (a device registered with a key `identity` says acts for the account)
    // replaces the list
    pub devices: Vec<Device>,
    pub apns: Option<Arc<Apns>>,
    pub fcm: Option<Arc<Fcm>>,
    pub http: Arc<dyn Http>,
    /// The calls sent, by tag, and their `until`: what a withdraw takes back.
    calls: Mutex<HashMap<String, u64>>,
}

impl Push {
    pub fn new(
        log: Log,
        chain: String,
        devices: Vec<Device>,
        (apns, fcm): (Option<Arc<Apns>>, Option<Arc<Fcm>>),
        http: Arc<dyn Http>,
    ) -> Self {
        Self {
            log,
            chain,
            devices,
            apns,
            fcm,
            http,
            calls: Mutex::default(),
        }
    }

    /// The account's tokens of `kind`.
    fn tokens(&self, kind: &str) -> impl Iterator<Item = &str> {
        self.devices
            .iter()
            .filter(move |device| device.kind == kind)
            .map(|device| device.token.as_str())
    }

    /// The requests that ring `call` on every device, at `now`.
    fn ring(&self, call: &Notice, now: u64) -> Vec<Request> {
        let payload = serde_json::json!({
            "kind": "call",
            "chain": self.chain,
            "program": call.program,
            "tag": call.tag,
            "title": call.title,
            "body": call.body,
            "link": call.link,
            "until": call.until,
        });
        let mut requests = Vec::new();
        if let Some(apns) = self
            .apns
            .as_deref()
            .filter(|_| self.tokens("voip").next().is_some())
        {
            match apns.bearer(now) {
                Ok(bearer) => requests.extend(self.tokens("voip").map(|token| Request {
                    url: format!("{}/3/device/{token}", apns.host),
                    headers: vec![
                        ("authorization", format!("bearer {bearer}")),
                        ("apns-push-type", "voip".into()),
                        ("apns-topic", format!("{}.voip", apns.topic)),
                        ("apns-priority", "10".into()),
                        ("apns-expiration", call.until.to_string()),
                        ("apns-collapse-id", collapse_id(&call.tag)),
                    ],
                    body: payload.to_string(),
                })),
                Err(error) => tracing::warn!(target: "pushd", %error, "no APNs provider token"),
            }
        }
        // FCM's data values are strings
        let data: serde_json::Map<_, _> = payload
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| {
                let text = value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned);
                (key.clone(), serde_json::Value::String(text))
            })
            .collect();
        requests.extend(self.fcm(&data, &call.tag, call.until, now, "high"));
        requests
    }

    /// The requests that take the call under `tag` back from every device
    /// before its `until`.
    fn withdrawal(&self, tag: &str, until: u64, now: u64) -> Vec<Request> {
        let mut requests = Vec::new();
        if let Some(apns) = self
            .apns
            .as_deref()
            .filter(|_| self.tokens("apns").next().is_some())
        {
            match apns.bearer(now) {
                Ok(bearer) => requests.extend(self.tokens("apns").map(|token| {
                    Request {
                        url: format!("{}/3/device/{token}", apns.host),
                        headers: vec![
                            ("authorization", format!("bearer {bearer}")),
                            ("apns-push-type", "background".into()),
                            ("apns-topic", apns.topic.clone()),
                            ("apns-priority", "5".into()),
                            ("apns-expiration", until.to_string()),
                        ],
                        body: serde_json::json!({
                            "aps": { "content-available": 1 },
                            "kind": "withdraw",
                            "tag": tag,
                        })
                        .to_string(),
                    }
                })),
                Err(error) => tracing::warn!(target: "pushd", %error, "no APNs provider token"),
            }
        }
        let data = serde_json::json!({ "kind": "withdraw", "tag": tag });
        let data = data.as_object().cloned().unwrap_or_default();
        // normal: it shows nothing, and FCM deprioritises an app whose
        // high-priority messages show nothing
        requests.extend(self.fcm(&data, tag, until, now, "normal"));
        requests
    }

    /// One FCM data message of `data` at `priority` per Android token,
    /// living until `until`.
    fn fcm(
        &self,
        data: &serde_json::Map<String, serde_json::Value>,
        tag: &str,
        until: u64,
        now: u64,
        priority: &str,
    ) -> Vec<Request> {
        let Some(fcm) = self
            .fcm
            .as_deref()
            .filter(|_| self.tokens("fcm").next().is_some())
        else {
            return Vec::new();
        };
        let bearer = match fcm.bearer(&*self.http, now) {
            Ok(bearer) => bearer,
            Err(error) => {
                tracing::warn!(target: "pushd", %error, "no FCM access token");
                return Vec::new();
            }
        };
        self.tokens("fcm")
            .map(|token| Request {
                url: format!(
                    "https://fcm.googleapis.com/v1/projects/{}/messages:send",
                    fcm.project
                ),
                headers: vec![
                    ("authorization", format!("Bearer {bearer}")),
                    ("content-type", "application/json".into()),
                ],
                body: serde_json::json!({
                    "message": {
                        "token": token,
                        "data": data,
                        "android": {
                            "priority": priority,
                            "ttl": format!("{}s", until.saturating_sub(now)),
                            "collapse_key": tag,
                        },
                    },
                })
                .to_string(),
            })
            .collect()
    }

    /// Sends each request; whether any was taken.
    fn send(&self, requests: Vec<Request>) -> bool {
        let mut taken = false;
        for request in requests {
            match self.http.send(&request) {
                Ok((200, _)) => taken = true,
                // ponytail: a token APNs or FCM says is gone stays listed
                // until M2's registry drops it
                Ok((status, body)) => {
                    let to = shown(&request.url);
                    tracing::warn!(target: "pushd", to, status, body, "push refused")
                }
                Err(error) => {
                    let to = shown(&request.url);
                    tracing::warn!(target: "pushd", to, %error, "push not sent")
                }
            }
        }
        taken
    }
}

/// Where a request went, for the log: an APNs device token in its URL cut
/// to its last four characters (FCM's token is in the body, never logged).
fn shown(url: &str) -> String {
    match url.rsplit_once("/device/") {
        Some((host, token)) => {
            let tail = token.get(token.len().saturating_sub(4)..).unwrap_or("");
            format!("{host}/device/…{tail}")
        }
        None => url.to_owned(),
    }
}

/// APNs's collapse id is at most 64 bytes: a longer tag goes by its hash.
fn collapse_id(tag: &str) -> String {
    match tag.len() <= 64 {
        true => tag.to_owned(),
        false => ring::digest::digest(&ring::digest::SHA256, tag.as_bytes())
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    }
}

impl Banners for Push {
    fn post(&self, notice: &Notice, click: Option<Click>) -> bool {
        let now = now();
        let over = notice.until != 0 && notice.until <= now;
        if notice.kind != NoticeKind::Call || over {
            return self.log.post(notice, click);
        }
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(notice.tag.clone(), notice.until);
        self.log.post(notice, click);
        self.send(self.ring(notice, now))
    }

    fn withdraw(&self, tag: &str) {
        self.log.withdraw(tag);
        let until = self
            .calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(tag);
        let now = now();
        // a call past its until rings nowhere any more: nothing to take back
        if let Some(until) = until.filter(|until| *until == 0 || *until > now) {
            self.send(self.withdrawal(tag, until, now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair as _;

    /// Records each request and answers 200, the token endpoint with a token.
    #[derive(Default)]
    struct Recorded(Mutex<Vec<Request>>);

    impl Http for Recorded {
        fn send(&self, request: &Request) -> Result<(u16, String), String> {
            self.0.lock().unwrap().push(request.clone());
            match request.url.contains("oauth2") {
                true => Ok((
                    200,
                    r#"{"access_token":"ya29.test","expires_in":3600}"#.into(),
                )),
                false => Ok((200, "{}".into())),
            }
        }
    }

    fn push(devices: &[(&str, &str)]) -> (Push, Arc<Recorded>) {
        let dir = tempfile::tempdir().unwrap();
        // a key made for the test, as Apple's `.p8` holds it (PKCS#8 PEM)
        let key = EcdsaKeyPair::generate_pkcs8(
            &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &ring::rand::SystemRandom::new(),
        )
        .unwrap();
        let pem = base64::engine::general_purpose::STANDARD.encode(key.as_ref());
        let p8 = dir.path().join("AuthKey_TESTKEY123.p8");
        let p8_text = format!("-----BEGIN PRIVATE KEY-----\n{pem}\n-----END PRIVATE KEY-----\n");
        std::fs::write(&p8, p8_text).unwrap();
        let apns = Apns::load(&ApnsConfig {
            key_file: p8,
            key_id: "TESTKEY123".into(),
            team_id: "8464KRUNQG".into(),
            topic: "industries.ducktape.mobile".into(),
            sandbox: false,
        })
        .unwrap();
        // ring makes no RSA key: the assertion's signature is a stand-in,
        // and Google's endpoint is the recorder
        let fcm = Fcm {
            project: "ducktape-test".into(),
            email: "pushd@ducktape-test.iam.gserviceaccount.com".into(),
            token_uri: "https://oauth2.googleapis.com/token".into(),
            sign: Box::new(|_| Ok(b"signed".to_vec())),
            token: Mutex::default(),
        };
        let http = Arc::new(Recorded::default());
        let push = Push::new(
            Log {
                network: "testkit".into(),
                account: 2,
            },
            "testkit-0a1b2c3d".into(),
            devices
                .iter()
                .map(|(kind, token)| Device {
                    kind: (*kind).into(),
                    token: (*token).into(),
                })
                .collect(),
            (Some(Arc::new(apns)), Some(Arc::new(fcm))),
            http.clone(),
        );
        (push, http)
    }

    fn call(until: u64) -> Notice {
        Notice {
            title: "ada is calling".into(),
            body: "Huddle with 2 people".into(),
            tag: "huddle/call/c1".into(),
            program: "huddle".into(),
            link: "duck://testkit-0a1b2c3d/huddle/call/c1".into(),
            kind: NoticeKind::Call,
            until,
        }
    }

    fn header<'a>(request: &'a Request, name: &str) -> &'a str {
        request
            .headers
            .iter()
            .find(|(key, _)| *key == name)
            .map_or("", |(_, value)| value)
    }

    /// A call rings an iPhone as a VoIP push expiring at its `until` and an
    /// Android phone as a high-priority data message living until it, each
    /// carrying the call's payload; the APNs provider token is an ES256 JWT
    /// of the team that the key's public half verifies.
    #[test]
    fn a_call_is_a_voip_push_and_a_high_priority_message() {
        let (push, http) = push(&[("voip", "ab12"), ("fcm", "fcm-token")]);
        let until = now() + 45;
        assert!(push.post(&call(until), Some(Click::Row(1))));
        let sent = http.0.lock().unwrap().clone();
        // the access token first: it is asked for while the requests are made
        let [oauth, apns, fcm] = &sent[..] else {
            panic!("three requests: {sent:#?}");
        };

        assert_eq!(apns.url, "https://api.push.apple.com/3/device/ab12");
        assert_eq!(header(apns, "apns-push-type"), "voip");
        assert_eq!(
            header(apns, "apns-topic"),
            "industries.ducktape.mobile.voip"
        );
        assert_eq!(header(apns, "apns-expiration"), until.to_string());
        assert_eq!(header(apns, "apns-collapse-id"), "huddle/call/c1");
        let body: serde_json::Value = serde_json::from_str(&apns.body).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "kind": "call", "chain": "testkit-0a1b2c3d", "program": "huddle",
                "tag": "huddle/call/c1", "title": "ada is calling", "body": "Huddle with 2 people",
                "link": "duck://testkit-0a1b2c3d/huddle/call/c1", "until": until,
            })
        );
        let jwt = header(apns, "authorization")
            .strip_prefix("bearer ")
            .unwrap();
        let (signing, signature) = jwt.rsplit_once('.').unwrap();
        let url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let claims: serde_json::Value =
            serde_json::from_slice(&url.decode(signing.split('.').nth(1).unwrap()).unwrap())
                .unwrap();
        assert_eq!(claims["iss"], "8464KRUNQG");
        let public = push
            .apns
            .as_ref()
            .unwrap()
            .key
            .public_key()
            .as_ref()
            .to_vec();
        signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, public)
            .verify(signing.as_bytes(), &url.decode(signature).unwrap())
            .expect("the provider token verifies with the key");

        assert_eq!(oauth.url, "https://oauth2.googleapis.com/token");
        let assertion = oauth.body.split_once("&assertion=").unwrap().1;
        let claims = assertion.split('.').nth(1).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&url.decode(claims).unwrap()).unwrap();
        assert_eq!(claims["iss"], "pushd@ducktape-test.iam.gserviceaccount.com");
        assert_eq!(
            claims["scope"],
            "https://www.googleapis.com/auth/firebase.messaging"
        );

        assert_eq!(
            fcm.url,
            "https://fcm.googleapis.com/v1/projects/ducktape-test/messages:send"
        );
        assert_eq!(header(fcm, "authorization"), "Bearer ya29.test");
        let message: serde_json::Value = serde_json::from_str(&fcm.body).unwrap();
        let message = &message["message"];
        assert_eq!(message["token"], "fcm-token");
        assert_eq!(message["android"]["priority"], "high");
        let ttl = message["android"]["ttl"].as_str().unwrap();
        assert!(ttl == "45s" || ttl == "44s", "{ttl}");
        assert_eq!(message["data"]["kind"], "call");
        assert_eq!(message["data"]["until"], until.to_string());
        assert_eq!(
            message["data"]["link"],
            "duck://testkit-0a1b2c3d/huddle/call/c1"
        );
    }

    /// A notifier's wire `Notification` of kind `Call` passes an account's
    /// centre (pushd's own settings) as a call and rings the phones: a
    /// VoIP push and a high-priority FCM message, both until its `until`.
    #[test]
    fn a_notifiers_call_notification_rings_the_phones() {
        let (push, http) = push(&[("voip", "ab12"), ("fcm", "fcm-token")]);
        let until = now() + 45;
        let ring = view_wire::methods::Notification {
            title: "ada is calling".into(),
            body: String::new(),
            tag: "call/c1".into(),
            link: "duck://testkit-0a1b2c3d/huddle/call/c1".into(),
            kind: NoticeKind::Call,
            until,
            end_link: String::new(),
        };
        let center = ducktape_host::runtime::notify::CenterHandle::default();
        let wall = i64::try_from(now()).unwrap();
        let (_, banner) = center.lock().post(
            &crate::settings(),
            "huddle",
            "Huddle",
            ring,
            Instant::now(),
            wall,
        );
        let banner = banner.expect("the call passed the centre");
        assert_eq!((banner.kind, banner.until), (NoticeKind::Call, until));
        assert!(push.post(&banner, None));
        let sent = http.0.lock().unwrap().clone();
        let apns = sent
            .iter()
            .find(|request| request.url.contains("/3/device/ab12"));
        let apns = apns.expect("a push to the iPhone");
        assert_eq!(header(apns, "apns-push-type"), "voip");
        assert_eq!(header(apns, "apns-expiration"), until.to_string());
        let fcm = sent
            .iter()
            .find(|request| request.url.contains("messages:send"));
        let fcm: serde_json::Value =
            serde_json::from_str(&fcm.expect("an FCM message").body).unwrap();
        assert_eq!(fcm["message"]["android"]["priority"], "high");
        assert_eq!(
            (
                &fcm["message"]["data"]["kind"],
                &fcm["message"]["data"]["tag"]
            ),
            (&"call".into(), &"huddle/call/c1".into())
        );
    }

    /// Taken down before its `until`, a call is withdrawn: a background push
    /// to the iPhone's APNs token (no second VoIP push) and `kind: withdraw`
    /// to the Android phone. Past its `until`, nothing is sent.
    #[test]
    fn a_withdraw_before_until_is_a_background_push_and_a_withdraw_message() {
        let (push, http) = push(&[("voip", "ab12"), ("apns", "cd34"), ("fcm", "fcm-token")]);
        push.post(&call(now() + 45), None);
        http.0.lock().unwrap().clear();
        push.withdraw("huddle/call/c1");
        let sent = http.0.lock().unwrap().clone();
        let apns = &sent[0];
        assert_eq!(apns.url, "https://api.push.apple.com/3/device/cd34");
        assert_eq!(header(apns, "apns-push-type"), "background");
        assert_eq!(header(apns, "apns-topic"), "industries.ducktape.mobile");
        let body: serde_json::Value = serde_json::from_str(&apns.body).unwrap();
        assert_eq!(body["aps"]["content-available"], 1);
        assert_eq!(
            (&body["kind"], &body["tag"]),
            (&"withdraw".into(), &"huddle/call/c1".into())
        );
        let fcm: serde_json::Value = serde_json::from_str(&sent.last().unwrap().body).unwrap();
        assert_eq!(
            fcm["message"]["data"],
            serde_json::json!({ "kind": "withdraw", "tag": "huddle/call/c1" })
        );
        assert_eq!(fcm["message"]["android"]["priority"], "normal");
        assert!(
            sent.iter()
                .all(|request| header(request, "apns-push-type") != "voip")
        );

        http.0.lock().unwrap().clear();
        push.withdraw("huddle/call/c1");
        assert!(http.0.lock().unwrap().is_empty(), "withdrawn twice");
        push.post(&call(now() - 1), None);
        push.withdraw("huddle/call/c1");
        assert!(
            http.0.lock().unwrap().is_empty(),
            "a call past its until rung or withdrawn"
        );
    }

    /// An endpoint that takes the connection and never answers is given up
    /// at the client's timeout, and the next request goes out: one stuck
    /// device never holds the program's queue past a call's `until`.
    #[test]
    fn a_hung_endpoint_times_out_and_the_next_request_goes() {
        use std::io::{Read as _, Write as _};
        // the kernel takes the connection into the backlog; nobody answers
        let hung = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let answers = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (hung_at, answers_at) = (hung.local_addr().unwrap(), answers.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut connection, _) = answers.accept().unwrap();
            let _ = connection.read(&mut [0; 4096]);
            let _ = connection.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok");
        });
        let client = Client::timed(Duration::from_millis(300)).unwrap();
        let to = |at: std::net::SocketAddr| Request {
            url: format!("http://{at}/"),
            headers: Vec::new(),
            body: "{}".into(),
        };
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send((client.send(&to(hung_at)), client.send(&to(answers_at))));
        });
        let (first, second) = finished
            .recv_timeout(Duration::from_secs(10))
            .expect("a hung endpoint held the next request");
        assert!(first.is_err(), "the hung endpoint answered: {first:?}");
        assert_eq!(second, Ok((200, "ok".to_owned())));
        drop(hung);
    }

    /// A logged URL shows the last four characters of an APNs token.
    #[test]
    fn a_logged_url_cuts_the_device_token() {
        assert_eq!(
            shown("https://api.push.apple.com/3/device/ab12cd34"),
            "https://api.push.apple.com/3/device/…cd34"
        );
        let fcm = "https://fcm.googleapis.com/v1/projects/p/messages:send";
        assert_eq!(shown(fcm), fcm);
    }

    /// A notice that is not a call sends nothing: it goes to the log.
    #[test]
    fn a_plain_notice_sends_no_push() {
        let (push, http) = push(&[("voip", "ab12"), ("fcm", "fcm-token")]);
        let notice = Notice {
            kind: NoticeKind::Notice,
            ..call(0)
        };
        push.post(&notice, None);
        push.withdraw("huddle/call/c1");
        assert!(http.0.lock().unwrap().is_empty());
    }

    /// A tag past APNs's 64 bytes collapses by its hash.
    #[test]
    fn a_long_tag_collapses_by_its_hash() {
        assert_eq!(collapse_id("huddle/call/c1"), "huddle/call/c1");
        assert_eq!(collapse_id(&"x".repeat(65)).len(), 64);
    }
}
