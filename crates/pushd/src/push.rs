//! The `push` channel: an account's notices to its phones.
//!
//! A notice the centre lets through goes to each of the account's devices
//! the registry lists ([`Devices`]): a call ([`NoticeKind::Call`]) to an
//! iPhone's PushKit token as an APNs VoIP push and to an Android phone's
//! Firebase token as a high-priority data message, both expiring at its
//! `until` (huddle call SPEC §5); any other notice to an iPhone's
//! remote-notification token as an alert and to an Android phone as a
//! high-priority data message the app shows (`Ring.kt`). Each notice is
//! its own notification under its id, the hash of its tag and link
//! (`Notice::os_id`: APNs's collapse id, Android's notification tag), and
//! a tag's notices stack together (iOS's `thread-id` and Android's group
//! are the tag): a chat room's messages stand side by side, and the same
//! notice posted again (a run restarted) replaces itself. The "N more"
//! banner past the burst, posted again on every post, is one notice a
//! program the same way: the centre posts it under `more/<program>` with
//! no link. A second notice under a tag still standing goes quietly (no
//! sound, a lower priority, `quiet` for Android). Taken down (the
//! program's `notify.seen` of its tag, or its `until`), the tag is
//! withdrawn, every notice under it at once: a background push to the
//! remote-notification token (never a second VoIP push, which iOS would
//! ring) and `kind: withdraw` to Android, both naming the tag. Past its
//! `until` a notice is dropped. Every notice is also one `log` line.
//!
//! A notice's title and body are cut to fit: APNs takes a payload of 4 KB
//! at most, and FCM a message's data of 4 KB ([`fitted`]). A token APNs or
//! FCM calls gone is dropped from the registry ([`gone`]).
//!
//! An APNs token is sent to the server its `env` names: Apple's sandbox for
//! a build signed for development, production for the rest. Both carry one
//! payload, `{kind, chain, program, tag, title, body, link, until}`, a
//! notice also its `id` and a call its `end` (the notice's `end_link`);
//! FCM's data values are strings. pushd names no program: a call is a
//! call whoever posts it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use ducktape_host::contract::Banners;
use ducktape_host::runtime::notify::{Click, Notice, NoticeKind};
use ring::signature::{self, EcdsaKeyPair, RsaKeyPair};

use crate::Log;
use crate::registry::Devices;

/// `[apns]` in pushd.toml: the team's APNs auth key, kept out of any repo.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct ApnsConfig {
    /// The auth key's `.p8` file (PKCS#8 PEM).
    pub key_file: PathBuf,
    pub key_id: String,
    pub team_id: String,
    /// The app's bundle id; a VoIP push goes to `<topic>.voip`.
    pub topic: String,
    /// Where every push goes instead of Apple, whatever a token's env: a
    /// stage's recorder.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// `[fcm]` in pushd.toml: the Firebase project's service account.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct FcmConfig {
    /// The service account's JSON key file.
    pub service_account: PathBuf,
    /// Where every message goes instead of Google: a stage's recorder.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// One HTTP request a channel sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
    /// The device's row it goes to, by kind and token: the one dropped when
    /// the answer says the token is gone. None for a request to no device.
    pub device: Option<(&'static str, String)>,
}

/// What sends a [`Request`]: the status and the body that came back.
/// Blocks; a channel's calls run on the blocking pool.
pub trait Http: Send + Sync {
    fn send(&self, request: &Request) -> Result<(u16, String), String>;
}

/// The internet: HTTP/2, which APNs requires, through reqwest. Every
/// request gives up in bounded time: the channel's sends run one after
/// another in the account's queue, and one stuck connection would hold
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
            // reqwest names the URL in its error, and an APNs URL ends in
            // the device's token: the log line names it cut (`shown`)
            let answer = post
                .body(request.body.clone())
                .send()
                .await
                .map_err(|error| error.without_url().to_string())?;
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
    endpoint: Option<String>,
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
            endpoint: config.endpoint.clone(),
            token: Mutex::default(),
        })
    }

    /// Apple's server for a token of `env`, or the configured endpoint.
    pub fn host(&self, env: &str) -> &str {
        match (&self.endpoint, env) {
            (Some(endpoint), _) => endpoint,
            (None, "sandbox") => "https://api.sandbox.push.apple.com",
            (None, _) => "https://api.push.apple.com",
        }
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

    /// One push of `body` to `token` (a `kind` token of `env`) with
    /// `headers` and the provider token.
    fn request(
        &self,
        (kind, token, env): (&'static str, &str, &str),
        bearer: &str,
        mut headers: Vec<(&'static str, String)>,
        body: &serde_json::Value,
    ) -> Request {
        headers.insert(0, ("authorization", format!("bearer {bearer}")));
        Request {
            url: format!("{}/3/device/{token}", self.host(env)),
            headers,
            body: body.to_string(),
            device: Some((kind, token.to_owned())),
        }
    }
}

/// What signs the service account's assertion: its RSA key (RS256).
type Signer = Box<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync>;

/// The Firebase service account, and the access token it is exchanged for.
pub struct Fcm {
    project: String,
    email: String,
    token_uri: String,
    endpoint: Option<String>,
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
            endpoint: config.endpoint.clone(),
            sign,
            token: Mutex::default(),
        })
    }

    /// Where a message is sent.
    fn url(&self) -> String {
        match &self.endpoint {
            Some(endpoint) => format!("{endpoint}/v1/projects/{}/messages:send", self.project),
            None => format!(
                "https://fcm.googleapis.com/v1/projects/{}/messages:send",
                self.project
            ),
        }
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
                device: None,
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

/// Unix milliseconds now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// The `push` channel of one account ([module docs](self)).
pub struct Push {
    /// Where every notice is also written.
    pub log: Log,
    /// The chain the payload names, as `duck://` links spell it.
    pub chain: String,
    pub account: u64,
    /// The network's registry: the account's devices, as registered now.
    pub devices: Arc<Devices>,
    pub apns: Option<Arc<Apns>>,
    pub fcm: Option<Arc<Fcm>>,
    pub http: Arc<dyn Http>,
    /// The tags standing on the phones, each with its last notice's
    /// `until`: what a withdraw takes back, and what a notice goes quietly
    /// under.
    // ponytail: in memory, so a restart alerts once more under a standing
    // tag and withdraws nothing posted before it; kept in the store if felt
    standing: Mutex<HashMap<String, u64>>,
}

/// One APNs send: the credential, its provider token and the tokens (each
/// with its env) to push to.
type ApnsSend<'a> = (&'a Apns, String, Vec<(String, String)>);

/// The two priorities a push is sent at.
const LOUD: &str = "10";
const QUIET: &str = "5";

impl Push {
    pub fn new(
        log: Log,
        chain: String,
        account: u64,
        devices: Arc<Devices>,
        (apns, fcm): (Option<Arc<Apns>>, Option<Arc<Fcm>>),
        http: Arc<dyn Http>,
    ) -> Self {
        Self {
            log,
            chain,
            account,
            devices,
            apns,
            fcm,
            http,
            standing: Mutex::default(),
        }
    }

    /// The account's tokens of `kind`, each with its env.
    fn tokens(&self, kind: &str) -> Vec<(String, String)> {
        self.devices.tokens(self.account, kind)
    }

    /// The APNs provider token, when there are tokens of `kind` to push to.
    fn apns_for(&self, kind: &str, now: u64) -> Option<ApnsSend<'_>> {
        let apns = self.apns.as_deref()?;
        let tokens = self.tokens(kind);
        if tokens.is_empty() {
            return None;
        }
        match apns.bearer(now) {
            Ok(bearer) => Some((apns, bearer, tokens)),
            Err(error) => {
                tracing::warn!(target: "pushd", %error, "no APNs provider token");
                None
            }
        }
    }

    /// The payload every push of `notice` carries.
    fn payload(&self, notice: &Notice, kind: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": kind,
            "chain": self.chain,
            "program": notice.program,
            "tag": notice.tag,
            "title": notice.title,
            "body": notice.body,
            "link": notice.link,
            "until": notice.until,
        })
    }

    /// The requests that ring `call` on every device, at `now`.
    fn ring(&self, call: &Notice, now: u64) -> Vec<Request> {
        let mut payload = self.payload(call, "call");
        payload["end"] = call.end_link.clone().into();
        let mut requests = Vec::new();
        if let Some((apns, bearer, tokens)) = self.apns_for("voip", now) {
            requests.extend(tokens.iter().map(|(token, env)| {
                apns.request(
                    ("voip", token, env),
                    &bearer,
                    vec![
                        ("apns-push-type", "voip".into()),
                        ("apns-topic", format!("{}.voip", apns.topic)),
                        ("apns-priority", LOUD.into()),
                        ("apns-expiration", call.until.to_string()),
                        ("apns-collapse-id", collapse_id(&call.tag)),
                    ],
                    &payload,
                )
            }));
        }
        requests.extend(self.fcm(strings(&payload), Some(&call.tag), call.until, now, "high"));
        requests
    }

    /// The requests that show `notice` on every device, at `now`; `quiet`
    /// when one under its tag already stands there. Its title and body cut
    /// to fit each channel ([`fitted`]).
    fn show(&self, notice: &Notice, now: u64, quiet: bool) -> Vec<Request> {
        let id = notice.os_id();
        let payload = |notice: &Notice| {
            let mut payload = self.payload(notice, "notice");
            payload["id"] = id.clone().into();
            payload
        };
        let mut requests = Vec::new();
        if let Some((apns, bearer, tokens)) = self.apns_for("apns", now) {
            // the title and body twice: the alert iOS shows, and the payload
            // the app reads
            let body = |notice: &Notice| {
                let mut aps = serde_json::json!({
                    "alert": { "title": notice.title, "body": notice.body },
                    "thread-id": notice.tag,
                });
                if !quiet {
                    aps["sound"] = "default".into();
                }
                let mut body = payload(notice);
                body["aps"] = aps;
                body
            };
            let body = body(&fitted(notice, |notice| body(notice).to_string().len()));
            let mut headers = vec![
                ("apns-push-type", "alert".into()),
                ("apns-topic", apns.topic.clone()),
                ("apns-priority", if quiet { QUIET } else { LOUD }.into()),
                ("apns-collapse-id", id.clone()),
            ];
            if notice.until != 0 {
                headers.push(("apns-expiration", notice.until.to_string()));
            }
            requests.extend(tokens.iter().map(|(token, env)| {
                apns.request(("apns", token, env), &bearer, headers.clone(), &body)
            }));
        }
        let data = |notice: &Notice| {
            let mut data = strings(&payload(notice));
            if quiet {
                data.insert("quiet".into(), "1".into());
            }
            data
        };
        let data = data(&fitted(notice, |notice| {
            serde_json::Value::Object(data(notice)).to_string().len()
        }));
        // no collapse key: FCM keeps four per device at most, and the
        // phone replaces the notification under its id itself
        requests.extend(self.fcm(data, None, notice.until, now, "high"));
        requests
    }

    /// The requests that take what stands under `tag` down from every
    /// device before its `until`.
    fn withdrawal(&self, tag: &str, until: u64, now: u64) -> Vec<Request> {
        let mut requests = Vec::new();
        if let Some((apns, bearer, tokens)) = self.apns_for("apns", now) {
            let body = serde_json::json!({
                "aps": { "content-available": 1 },
                "kind": "withdraw",
                "tag": tag,
            });
            let mut headers = vec![
                ("apns-push-type", "background".into()),
                ("apns-topic", apns.topic.clone()),
                ("apns-priority", QUIET.into()),
            ];
            if until != 0 {
                headers.push(("apns-expiration", until.to_string()));
            }
            requests.extend(tokens.iter().map(|(token, env)| {
                apns.request(("apns", token, env), &bearer, headers.clone(), &body)
            }));
        }
        let data = serde_json::json!({ "kind": "withdraw", "tag": tag });
        let data = data.as_object().cloned().unwrap_or_default();
        // normal: it shows nothing, and FCM deprioritises an app whose
        // high-priority messages show nothing
        requests.extend(self.fcm(data, None, until, now, "normal"));
        requests
    }

    /// One FCM data message of `data` at `priority` per Android token,
    /// living until `until` (0: FCM's default), collapsed under
    /// `collapse` when given.
    fn fcm(
        &self,
        data: serde_json::Map<String, serde_json::Value>,
        collapse: Option<&str>,
        until: u64,
        now: u64,
        priority: &str,
    ) -> Vec<Request> {
        let Some(fcm) = self.fcm.as_deref() else {
            return Vec::new();
        };
        let tokens = self.tokens("fcm");
        if tokens.is_empty() {
            return Vec::new();
        }
        let bearer = match fcm.bearer(&*self.http, now) {
            Ok(bearer) => bearer,
            Err(error) => {
                tracing::warn!(target: "pushd", %error, "no FCM access token");
                return Vec::new();
            }
        };
        tokens
            .iter()
            .map(|(token, _)| {
                let mut android = serde_json::json!({ "priority": priority });
                if until != 0 {
                    android["ttl"] = format!("{}s", until.saturating_sub(now)).into();
                }
                if let Some(collapse) = collapse {
                    android["collapse_key"] = collapse.into();
                }
                Request {
                    url: fcm.url(),
                    headers: vec![
                        ("authorization", format!("Bearer {bearer}")),
                        ("content-type", "application/json".into()),
                    ],
                    body: serde_json::json!({
                        "message": { "token": token, "data": data, "android": android },
                    })
                    .to_string(),
                    device: Some(("fcm", token.clone())),
                }
            })
            .collect()
    }

    /// Sends each request; whether any was taken. A token the answer calls
    /// gone loses its row (that kind and token, on this network), and the
    /// log says so in one line.
    fn send(&self, requests: Vec<Request>) -> bool {
        let mut taken = false;
        for request in requests {
            match self.http.send(&request) {
                Ok((200, _)) => taken = true,
                Ok((status, body)) => {
                    let to = shown(&request.url);
                    match &request.device {
                        Some((kind, token)) if gone(kind, status, &body) => {
                            match self.devices.drop_token(kind, token) {
                                Ok(_) => tracing::info!(target: "pushd", to, kind, status, body,
                                    "a token APNs or FCM calls gone: dropped"),
                                Err(error) => tracing::warn!(target: "pushd", to, kind, status,
                                    body, %error, "a token APNs or FCM calls gone: not dropped"),
                            }
                        }
                        _ => tracing::warn!(target: "pushd", to, status, body, "push refused"),
                    }
                }
                Err(error) => {
                    let to = shown(&request.url);
                    tracing::warn!(target: "pushd", to, %error, "push not sent")
                }
            }
        }
        taken
    }

    fn standing(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.standing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Whether APNs's or FCM's answer (`status`, `body`) to a push to a `kind`
/// token says the token is gone: APNs's 410 or its reasons `Unregistered`
/// and `BadDeviceToken` (a token Apple's server for its env does not
/// know), FCM's `UNREGISTERED`.
fn gone(kind: &str, status: u16, body: &str) -> bool {
    match kind {
        "fcm" => body.contains("UNREGISTERED"),
        _ => status == 410 || body.contains("Unregistered") || body.contains("BadDeviceToken"),
    }
}

/// The most an APNs payload, or the data of an FCM message, may be.
const MOST: usize = 4096;

/// `notice` cut so `size` of it is at most [`MOST`]: its body, then its
/// title, each to its longest start that fits, cut on a char and ended with
/// `…` (or emptied, when not even that fits). Its link, tag, `until` and
/// the rest are never cut: a notice too big without its words goes as it
/// is, and APNs or FCM refuses it.
fn fitted(notice: &Notice, size: impl Fn(&Notice) -> usize) -> Notice {
    let words: [fn(&mut Notice) -> &mut String; 2] =
        [|notice| &mut notice.body, |notice| &mut notice.title];
    let mut fit = notice.clone();
    for words in words {
        if size(&fit) <= MOST {
            break;
        }
        let whole = std::mem::take(words(&mut fit));
        let ends: Vec<usize> = whole.char_indices().map(|(at, _)| at).collect();
        let cut = |end: usize| format!("{}…", &whole[..end]);
        let fits = ends.partition_point(|&end| {
            let mut tried = fit.clone();
            *words(&mut tried) = cut(end);
            size(&tried) <= MOST
        });
        *words(&mut fit) = match fits {
            0 => String::new(),
            fits => cut(ends[fits - 1]),
        };
    }
    fit
}

/// `payload`'s fields as strings: FCM's data values.
fn strings(payload: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    payload
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| {
            let text = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            (key.clone(), serde_json::Value::String(text))
        })
        .collect()
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
        false => sha256(tag),
    }
}

/// `text`'s SHA-256, in hex.
fn sha256(text: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl Banners for Push {
    fn post(&self, notice: &Notice, click: Option<Click>) -> bool {
        let now = now();
        self.log.post(notice, click);
        if notice.until != 0 && notice.until <= now {
            return false;
        }
        let quiet = self.standing().contains_key(&notice.tag);
        let taken = match notice.kind {
            NoticeKind::Call => self.send(self.ring(notice, now)),
            NoticeKind::Notice => self.send(self.show(notice, now, quiet)),
        };
        // a tag stands once a device took it: one sent nowhere (an account
        // with no token of its kind yet) leaves the next one loud
        if taken && !notice.tag.is_empty() {
            self.standing().insert(notice.tag.clone(), notice.until);
        }
        taken
    }

    fn withdraw(&self, tag: &str) {
        self.log.withdraw(tag);
        let until = self.standing().remove(tag);
        let now = now();
        // one past its until shows nowhere any more: nothing to take back
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

    /// An `Apns` over a key made for the test, as Apple's `.p8` holds it
    /// (PKCS#8 PEM), sending to `endpoint` when given.
    fn apns_with(endpoint: Option<String>) -> Apns {
        let dir = tempfile::tempdir().unwrap();
        let key = EcdsaKeyPair::generate_pkcs8(
            &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &ring::rand::SystemRandom::new(),
        )
        .unwrap();
        let pem = base64::engine::general_purpose::STANDARD.encode(key.as_ref());
        let p8 = dir.path().join("AuthKey_TESTKEY123.p8");
        let p8_text = format!("-----BEGIN PRIVATE KEY-----\n{pem}\n-----END PRIVATE KEY-----\n");
        std::fs::write(&p8, p8_text).unwrap();
        Apns::load(&ApnsConfig {
            key_file: p8,
            key_id: "TESTKEY123".into(),
            team_id: "8464KRUNQG".into(),
            topic: "industries.ducktape.mobile".into(),
            endpoint,
        })
        .unwrap()
    }

    fn push(devices: &[(&str, &str)]) -> (Push, Arc<Recorded>) {
        let apns = apns_with(None);
        // ring makes no RSA key: the assertion's signature is a stand-in,
        // and Google's endpoint is the recorder
        let fcm = Fcm {
            project: "ducktape-test".into(),
            email: "pushd@ducktape-test.iam.gserviceaccount.com".into(),
            token_uri: "https://oauth2.googleapis.com/token".into(),
            endpoint: None,
            sign: Box::new(|_| Ok(b"signed".to_vec())),
            token: Mutex::default(),
        };
        let http = Arc::new(Recorded::default());
        let registry = Arc::new(Devices::default());
        for (at, (kind, token)) in devices.iter().enumerate() {
            registry
                .put(crate::registry::Row {
                    account: 2,
                    key: format!("key{at}"),
                    kind: (*kind).into(),
                    token: (*token).into(),
                    env: match *kind {
                        "fcm" => String::new(),
                        _ => "production".into(),
                    },
                    at: 1,
                })
                .unwrap();
        }
        let push = Push::new(
            Log {
                network: "testkit".into(),
                account: 2,
            },
            "testkit-0a1b2c3d".into(),
            2,
            registry,
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
            end_link: "duck://testkit-0a1b2c3d/huddle/call/c1/leave".into(),
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
                "end": "duck://testkit-0a1b2c3d/huddle/call/c1/leave",
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
        assert_eq!(
            message["data"]["end"],
            "duck://testkit-0a1b2c3d/huddle/call/c1/leave"
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
            end_link: "duck://testkit-0a1b2c3d/huddle/call/c1/leave".into(),
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
        assert_eq!(
            banner.end_link,
            "duck://testkit-0a1b2c3d/huddle/call/c1/leave"
        );
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
    /// device never holds the program's queue past a call's `until`. The
    /// error names no device token (it is logged).
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
            url: format!("http://{at}/3/device/ab12cd34"),
            headers: Vec::new(),
            body: "{}".into(),
            device: None,
        };
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send((client.send(&to(hung_at)), client.send(&to(answers_at))));
        });
        let (first, second) = finished
            .recv_timeout(Duration::from_secs(10))
            .expect("a hung endpoint held the next request");
        let error = first.expect_err("the hung endpoint answered");
        assert!(
            !error.contains("ab12cd34"),
            "the token in an error: {error}"
        );
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

    /// A notice that is not a call is an alert to the iPhone's
    /// remote-notification token (never its VoIP one), in its tag's thread
    /// under its own id, with a sound, and a high-priority data message the
    /// Android app shows; a second under the same tag goes quietly (no
    /// sound, lower priority, `quiet` for Android); taken down, both get a
    /// withdraw; a tag withdrawn alerts again when posted again.
    #[test]
    fn a_notice_alerts_once_then_replaces_quietly_until_withdrawn() {
        let (push, http) = push(&[("voip", "ab12"), ("apns", "cd34"), ("fcm", "fcm-token")]);
        let notice = Notice {
            kind: NoticeKind::Notice,
            title: "ada mentioned you".into(),
            body: "hey @bob".into(),
            tag: "chat/#general".into(),
            program: "chat".into(),
            link: "duck://testkit-0a1b2c3d/chat/general/7".into(),
            until: 0,
            end_link: "duck://testkit-0a1b2c3d/chat/general/7/end".into(),
        };
        assert!(push.post(&notice, Some(Click::Row(1))));
        let sent = http.0.lock().unwrap().clone();
        let apns: Vec<_> = sent
            .iter()
            .filter(|request| request.url.contains("/3/device/"))
            .collect();
        assert_eq!(apns.len(), 1, "{sent:#?}");
        assert_eq!(apns[0].url, "https://api.push.apple.com/3/device/cd34");
        assert_eq!(header(apns[0], "apns-push-type"), "alert");
        assert_eq!(header(apns[0], "apns-topic"), "industries.ducktape.mobile");
        assert_eq!(header(apns[0], "apns-priority"), "10");
        assert_eq!(header(apns[0], "apns-collapse-id"), SEVEN);
        assert_eq!(header(apns[0], "apns-expiration"), "");
        let body: serde_json::Value = serde_json::from_str(&apns[0].body).unwrap();
        assert_eq!(body["aps"]["alert"]["title"], "ada mentioned you");
        assert_eq!(body["aps"]["alert"]["body"], "hey @bob");
        assert_eq!(body["aps"]["sound"], "default");
        assert_eq!(body["aps"]["thread-id"], "chat/#general");
        assert_eq!(body["kind"], "notice");
        assert_eq!(body["link"], "duck://testkit-0a1b2c3d/chat/general/7");
        // `end` is a call's alone
        assert!(body.get("end").is_none(), "{body}");
        let fcm = sent
            .iter()
            .find(|request| request.url.contains("messages:send"));
        let fcm: serde_json::Value = serde_json::from_str(&fcm.unwrap().body).unwrap();
        assert_eq!(fcm["message"]["android"]["priority"], "high");
        assert!(fcm["message"]["android"].get("ttl").is_none());
        assert!(fcm["message"]["android"].get("collapse_key").is_none());
        assert_eq!(fcm["message"]["data"]["kind"], "notice");
        assert!(fcm["message"]["data"].get("end").is_none());
        assert!(fcm["message"]["data"].get("quiet").is_none());

        http.0.lock().unwrap().clear();
        push.post(&notice, None);
        let sent = http.0.lock().unwrap().clone();
        let apns = sent
            .iter()
            .find(|request| request.url.contains("/3/device/"))
            .unwrap();
        assert_eq!(header(apns, "apns-priority"), "5");
        let body: serde_json::Value = serde_json::from_str(&apns.body).unwrap();
        assert!(body["aps"].get("sound").is_none(), "{body}");
        let fcm = sent
            .iter()
            .find(|request| request.url.contains("messages:send"));
        let fcm: serde_json::Value = serde_json::from_str(&fcm.unwrap().body).unwrap();
        assert_eq!(fcm["message"]["data"]["quiet"], "1");

        http.0.lock().unwrap().clear();
        push.withdraw("chat/#general");
        let sent = http.0.lock().unwrap().clone();
        let apns = sent
            .iter()
            .find(|request| request.url.contains("/3/device/"))
            .unwrap();
        assert_eq!(header(apns, "apns-push-type"), "background");
        assert_eq!(header(apns, "apns-expiration"), "");
        let fcm = sent
            .iter()
            .find(|request| request.url.contains("messages:send"));
        let fcm: serde_json::Value = serde_json::from_str(&fcm.unwrap().body).unwrap();
        assert_eq!(fcm["message"]["data"]["kind"], "withdraw");
        assert!(fcm["message"]["data"].get("end").is_none());

        http.0.lock().unwrap().clear();
        push.post(&notice, None);
        let sent = http.0.lock().unwrap().clone();
        let apns = sent
            .iter()
            .find(|request| request.url.contains("/3/device/"))
            .unwrap();
        assert_eq!(header(apns, "apns-priority"), "10");
    }

    /// The id of the notice under `chat/#general` linking message 7, and
    /// of the one linking message 8: SHA-256 of tag, a newline and link.
    const SEVEN: &str = "91456d85cdd6e18a45793477650461b03e8b429f5051379cbfd1158c169df970";
    const EIGHT: &str = "cd2c0f5bebda5c4d861fe2ed7fcce6bc611e5c293363c5bb41a4bd645473cb73";

    /// Chat's notice of message `seq` in `#general`, as its notifier words
    /// it once the centre took it.
    fn message(seq: u32) -> Notice {
        Notice {
            kind: NoticeKind::Notice,
            title: "ada mentioned you".into(),
            body: "hey @bob".into(),
            tag: "chat/#general".into(),
            program: "chat".into(),
            link: format!("duck://testkit-0a1b2c3d/chat/general/{seq}"),
            until: 0,
            end_link: String::new(),
        }
    }

    /// The alert to the iPhone and the FCM message one post sent, taken
    /// off the recorder.
    fn sent(http: &Recorded) -> (Request, serde_json::Value) {
        let sent = std::mem::take(&mut *http.0.lock().unwrap());
        let mut apns = sent
            .iter()
            .filter(|request| request.url.contains("/3/device/"));
        let mut fcm = sent
            .iter()
            .filter(|request| request.url.contains("messages:send"));
        let (Some(alert), None, Some(message), None) =
            (apns.next(), apns.next(), fcm.next(), fcm.next())
        else {
            panic!("one alert and one message: {sent:#?}");
        };
        let message: serde_json::Value = serde_json::from_str(&message.body).unwrap();
        (alert.clone(), message["message"].clone())
    }

    /// A request's headers but its provider token, which changes.
    fn headers(request: &Request) -> Vec<(&'static str, String)> {
        request
            .headers
            .iter()
            .filter(|(name, _)| *name != "authorization")
            .cloned()
            .collect()
    }

    /// The headers of an alert sent at `priority` as `id`.
    fn alert(priority: &str, id: &str) -> Vec<(&'static str, String)> {
        vec![
            ("apns-push-type", "alert".into()),
            ("apns-topic", "industries.ducktape.mobile".into()),
            ("apns-priority", priority.into()),
            ("apns-collapse-id", id.into()),
        ]
    }

    /// The payload of `notice` as `id`, the alert's or (`strings`) FCM's.
    fn carried(notice: &Notice, id: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "notice", "chain": "testkit-0a1b2c3d", "program": notice.program,
            "tag": notice.tag, "title": notice.title, "body": notice.body,
            "link": notice.link, "until": notice.until, "id": id,
        })
    }

    /// Two messages in one room are two notices: each under its own id
    /// (APNs's collapse id, the FCM data's `id` Android posts it under),
    /// both in the room's thread (iOS's `thread-id`, and the tag Android
    /// groups by); the second goes quietly, its room standing.
    #[test]
    fn two_notices_in_a_room_stand_apart_in_its_thread() {
        let (push, http) = push(&[("apns", "cd34"), ("fcm", "fcm-token")]);
        for (seq, id, quiet) in [(7, SEVEN, false), (8, EIGHT, true)] {
            let notice = message(seq);
            assert!(push.post(&notice, None));
            let (apns, fcm) = sent(&http);
            assert_eq!(apns.url, "https://api.push.apple.com/3/device/cd34");
            assert_eq!(headers(&apns), alert(if quiet { "5" } else { "10" }, id));
            let mut body = carried(&notice, id);
            body["aps"] = serde_json::json!({
                "alert": { "title": "ada mentioned you", "body": "hey @bob" },
                "thread-id": "chat/#general",
            });
            if !quiet {
                body["aps"]["sound"] = "default".into();
            }
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&apns.body).unwrap(),
                body
            );
            let mut data = serde_json::Value::Object(strings(&carried(&notice, id)));
            if quiet {
                data["quiet"] = "1".into();
            }
            assert_eq!(
                fcm,
                serde_json::json!({
                    "token": "fcm-token", "data": data, "android": { "priority": "high" },
                })
            );
        }
    }

    /// The same notice posted again (a run that restarted re-posts what
    /// stands) keeps its id, so it replaces itself on the phones.
    #[test]
    fn a_repost_keeps_its_id() {
        let (push, http) = push(&[("apns", "cd34"), ("fcm", "fcm-token")]);
        push.post(&message(7), None);
        sent(&http);
        push.post(&message(7), None);
        let (apns, fcm) = sent(&http);
        assert_eq!(headers(&apns), alert("5", SEVEN));
        let body: serde_json::Value = serde_json::from_str(&apns.body).unwrap();
        assert_eq!(body["id"], SEVEN);
        assert_eq!(fcm["data"]["id"], SEVEN);
        assert_eq!(fcm["data"]["quiet"], "1");
    }

    /// The burst's "N more", posted again on every notice past the burst,
    /// is one notice a program: under `more/<program>` with no link, it
    /// keeps one id whatever its count, alone in its thread.
    #[test]
    fn the_burst_summary_is_one_notice_a_program() {
        const MORE: &str = "ef7dad463059cafba2349784b2e003dd96d0edf8052009d9dfdfc17ded4b8d1c";
        let (push, http) = push(&[("apns", "cd34"), ("fcm", "fcm-token")]);
        let center = ducktape_host::runtime::notify::CenterHandle::default();
        let (at, wall) = (Instant::now(), i64::try_from(now()).unwrap());
        let summaries: Vec<Notice> = (0..crate::BURST + 2)
            .filter_map(|seq| {
                let post = view_wire::methods::Notification {
                    title: "ada mentioned you".into(),
                    body: "hey @bob".into(),
                    tag: "#general".into(),
                    link: format!("duck://testkit-0a1b2c3d/chat/general/{seq}"),
                    kind: NoticeKind::Notice,
                    until: 0,
                    end_link: String::new(),
                };
                let (_, banner) =
                    center
                        .lock()
                        .post(&crate::settings(), "chat", "Chat", post, at, wall);
                banner.filter(|banner| banner.tag == "more/chat")
            })
            .collect();
        let titles: Vec<_> = summaries.iter().map(|more| more.title.as_str()).collect();
        assert_eq!(titles, ["1 more from Chat", "2 more from Chat"]);
        for (more, priority) in summaries.iter().zip(["10", "5"]) {
            assert!(push.post(more, None));
            let (apns, fcm) = sent(&http);
            assert_eq!(headers(&apns), alert(priority, MORE));
            let body: serde_json::Value = serde_json::from_str(&apns.body).unwrap();
            assert_eq!(body["aps"]["thread-id"], "more/chat");
            assert_eq!((&body["id"], &body["link"]), (&MORE.into(), &"".into()));
            assert_eq!(fcm["data"]["id"], MORE);
        }
    }

    /// A withdraw names the tag alone: one background push to each iPhone
    /// and one `withdraw` to each Android phone take down every notice in
    /// the room, however many stand there.
    #[test]
    fn a_withdraw_takes_down_the_whole_room() {
        let (push, http) = push(&[("apns", "cd34"), ("fcm", "fcm-token")]);
        push.post(&message(7), None);
        push.post(&message(8), None);
        http.0.lock().unwrap().clear();
        push.withdraw("chat/#general");
        let sent = std::mem::take(&mut *http.0.lock().unwrap());
        let [apns, fcm] = &sent[..] else {
            panic!("one push and one message: {sent:#?}");
        };
        assert_eq!(apns.url, "https://api.push.apple.com/3/device/cd34");
        assert_eq!(
            headers(apns),
            vec![
                ("apns-push-type", "background".to_owned()),
                ("apns-topic", "industries.ducktape.mobile".to_owned()),
                ("apns-priority", "5".to_owned()),
            ]
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&apns.body).unwrap(),
            serde_json::json!({
                "aps": { "content-available": 1 }, "kind": "withdraw", "tag": "chat/#general",
            })
        );
        let fcm: serde_json::Value = serde_json::from_str(&fcm.body).unwrap();
        assert_eq!(
            fcm["message"],
            serde_json::json!({
                "token": "fcm-token",
                "data": { "kind": "withdraw", "tag": "chat/#general" },
                "android": { "priority": "normal" },
            })
        );
    }

    /// A token registered under `sandbox` goes to Apple's sandbox server,
    /// one under `production` to the production one; an endpoint set in
    /// the config takes every push whatever the env.
    #[test]
    fn an_apns_token_goes_to_the_server_its_env_names() {
        let (push, http) = push(&[("apns", "cd34")]);
        push.devices
            .put(crate::registry::Row {
                account: 2,
                key: "dev-key".into(),
                kind: "apns".into(),
                token: "ef56".into(),
                env: "sandbox".into(),
                at: 1,
            })
            .unwrap();
        push.post(
            &Notice {
                kind: NoticeKind::Notice,
                ..call(0)
            },
            None,
        );
        let sent = http.0.lock().unwrap().clone();
        let mut urls: Vec<_> = sent
            .iter()
            .filter(|request| request.url.contains("/3/device/"))
            .map(|request| request.url.as_str())
            .collect();
        urls.sort();
        assert_eq!(
            urls,
            [
                "https://api.push.apple.com/3/device/cd34",
                "https://api.sandbox.push.apple.com/3/device/ef56"
            ]
        );
        let apns = push.apns.as_ref().unwrap();
        assert_eq!(apns.host("sandbox"), "https://api.sandbox.push.apple.com");
        let staged = apns_with(Some("http://127.0.0.1:9".into()));
        assert_eq!(staged.host("sandbox"), "http://127.0.0.1:9");
        assert_eq!(staged.host("production"), "http://127.0.0.1:9");
    }

    /// A notice no device took leaves its tag not standing: an account
    /// with only a VoIP token is sent no notice, and the first one after
    /// its remote-notification token registers alerts with a sound.
    #[test]
    fn a_tag_stands_once_a_device_took_it() {
        let (push, http) = push(&[("voip", "ab12")]);
        let notice = Notice {
            kind: NoticeKind::Notice,
            ..call(0)
        };
        assert!(!push.post(&notice, None));
        assert!(http.0.lock().unwrap().is_empty());
        push.devices
            .put(crate::registry::Row {
                account: 2,
                key: "key9".into(),
                kind: "apns".into(),
                token: "cd34".into(),
                env: "production".into(),
                at: 1,
            })
            .unwrap();
        assert!(push.post(&notice, None));
        let sent = http.0.lock().unwrap().clone();
        let [apns] = &sent[..] else {
            panic!("one alert: {sent:#?}");
        };
        assert_eq!(header(apns, "apns-priority"), "10");
        let body: serde_json::Value = serde_json::from_str(&apns.body).unwrap();
        assert_eq!(body["aps"]["sound"], "default");
    }

    /// A notice too long for APNs's 4 KB payload or FCM's 4 KB of data has
    /// its body cut to fit each, on a char and ended with `…`; its title
    /// too when the body alone is not enough. The link, tag, id and `until`
    /// stand whole, and a call is not cut.
    #[test]
    fn a_long_notice_is_cut_to_fit_four_kilobytes() {
        let (push, http) = push(&[("apns", "cd34"), ("fcm", "fcm-token")]);
        // three bytes a char: a cut anywhere but on a char would not parse
        let long = Notice {
            body: "가".repeat(3000),
            ..message(1)
        };
        assert!(push.post(&long, None));
        let (apns, fcm) = sent(&http);
        let id = long.os_id();
        // the body twice in an alert: a char less is six bytes less
        assert_eq!(apns.body.len(), 4091);
        let alert: serde_json::Value = serde_json::from_str(&apns.body).unwrap();
        let body = alert["body"].as_str().unwrap();
        assert!(body.ends_with('…'), "{body}");
        assert!(long.body.starts_with(body.trim_end_matches('…')));
        assert_eq!(alert["aps"]["alert"]["body"], body);
        assert_eq!(alert["title"], long.title);
        for (field, whole) in [("link", &long.link), ("tag", &long.tag), ("id", &id)] {
            assert_eq!(alert[field], **whole, "{field} was cut");
            assert_eq!(fcm["data"][field], **whole, "{field} was cut");
        }
        assert_eq!(alert["until"], 0);
        let data = serde_json::to_string(&fcm["data"]).unwrap();
        assert_eq!(data.len(), 4096);
        assert!(fcm["data"]["body"].as_str().unwrap().ends_with('…'));

        // a title too long to fit beside an empty body: cut as well
        let longer = Notice {
            title: "t".repeat(5000),
            body: "b".repeat(5000),
            ..message(2)
        };
        assert!(push.post(&longer, None));
        let (apns, fcm) = sent(&http);
        // the title twice: a char less is two bytes less
        assert_eq!(apns.body.len(), 4095);
        let alert: serde_json::Value = serde_json::from_str(&apns.body).unwrap();
        assert_eq!(alert["body"], "");
        assert!(alert["title"].as_str().unwrap().ends_with("t…"));
        assert_eq!(serde_json::to_string(&fcm["data"]).unwrap().len(), 4096);

        // a notice that fits goes whole
        assert!(push.post(&message(3), None));
        let (apns, _) = sent(&http);
        let alert: serde_json::Value = serde_json::from_str(&apns.body).unwrap();
        assert_eq!(alert["body"], "hey @bob");

        // a call is never cut
        let ring = Notice {
            body: "c".repeat(5000),
            ..call(now() + 45)
        };
        assert!(push.post(&ring, None));
        let sent = std::mem::take(&mut *http.0.lock().unwrap());
        let message: serde_json::Value = serde_json::from_str(&sent[0].body).unwrap();
        assert_eq!(message["message"]["data"]["body"], ring.body);
    }

    /// A token APNs calls gone (410 `Unregistered`, or `BadDeviceToken`)
    /// or FCM calls gone (`UNREGISTERED`) loses its row, in the registry's
    /// file too; that row only: another token, and one whose push failed
    /// for another reason, stay.
    #[test]
    fn a_token_called_gone_is_dropped() {
        struct Answers;
        impl Http for Answers {
            fn send(&self, request: &Request) -> Result<(u16, String), String> {
                if request.url.contains("oauth2") {
                    let token = r#"{"access_token":"ya29.test","expires_in":3600}"#;
                    return Ok((200, token.into()));
                }
                let to = request.device.as_ref().map_or("", |(_, token)| token);
                Ok(match to {
                    "dead" => (410, r#"{"reason":"Unregistered","timestamp":1700000000000}"#),
                    "bad0" => (400, r#"{"reason":"BadDeviceToken"}"#),
                    "fcm-dead" => (
                        404,
                        r#"{"error":{"code":404,"status":"NOT_FOUND","details":[{"@type":"type.googleapis.com/google.firebase.fcm.v1.FcmError","errorCode":"UNREGISTERED"}]}}"#,
                    ),
                    "fcm-busy" => (503, r#"{"error":{"code":503,"status":"UNAVAILABLE"}}"#),
                    _ => (200, "{}"),
                })
                .map(|(status, body)| (status, body.to_owned()))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("testkit").join("devices.json");
        let (mut push, _) = push(&[]);
        push.devices = Devices::load(&file).unwrap();
        push.http = Arc::new(Answers);
        let rows = [
            ("k1", "apns", "dead"),
            ("k2", "apns", "bad0"),
            ("k3", "apns", "cd34"),
            ("k4", "fcm", "fcm-dead"),
            ("k5", "fcm", "fcm-busy"),
        ];
        for (key, kind, token) in rows {
            let env = if kind == "fcm" { "" } else { "production" };
            push.devices
                .put(crate::registry::Row {
                    account: 2,
                    key: key.into(),
                    kind: kind.into(),
                    token: token.into(),
                    env: env.into(),
                    at: 1,
                })
                .unwrap();
        }
        assert!(push.post(&message(1), None));
        let left = |devices: &Devices, kind: &str| -> Vec<String> {
            let tokens = devices.tokens(2, kind).into_iter();
            tokens.map(|(token, _)| token).collect()
        };
        assert_eq!(left(&push.devices, "apns"), ["cd34"]);
        assert_eq!(left(&push.devices, "fcm"), ["fcm-busy"]);
        let kept = Devices::load(&file).unwrap();
        assert_eq!(left(&kept, "apns"), ["cd34"]);
        assert_eq!(left(&kept, "fcm"), ["fcm-busy"]);
    }

    /// A tag past APNs's 64 bytes collapses by its hash.
    #[test]
    fn a_long_tag_collapses_by_its_hash() {
        assert_eq!(collapse_id("huddle/call/c1"), "huddle/call/c1");
        assert_eq!(collapse_id(&"x".repeat(65)).len(), 64);
    }
}
