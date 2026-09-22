//! Passkeys: an ACCOUNT's key, held by the person's authenticator. This
//! device's ed25519 key still signs every write; a passkey only creates the
//! account with it, and admits a new device's key into it later.
//!
//! The app speaks no WebAuthn. The ceremony runs on the auth page
//! ([`AUTH_PAGE`], RP ID = its host) in the system browser: the request
//! rides the URL fragment, and the result comes back as a top-level form
//! POST (`result=<JSON>`) to a one-shot loopback [`Listener`]. The contract
//! is core's `ops/auth-page/README.md` (b690a31bf^); the verifier every
//! answer must satisfy is `keyscheme` (`Secp256r1` = the assertion envelope
//! `authenticatorData ‖ clientDataJSON ‖ sig64`, challenge
//! `SHA-256(ns ‖ preimage)`).
//!
//! The contract's `user.id` names the account NUMBER, which the identity
//! program assigns at `Create`. So a passkey account is created by this
//! device's key, and the passkey joins it next: touch 1 `create`s the
//! passkey, touch 2 signs its own `AddKey` frame (the device key consents).
//! A new device is two touches too: touch 1 asks the passkey which account
//! it holds, touch 2 consents to this device's key joining that account.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use identity::{Admission, CONSENT_NAMESPACE, Consent, Op, Query, Reply};
use keyscheme::KeyScheme;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::noded::{Body, FRAME_NAMESPACE, Frame, Layer};
use super::{RpcClient, next_seq, query_frame, seated_frame, seated_key, seated_sign};

/// The live page. Its host IS the RP ID every passkey is scoped to.
pub(crate) const AUTH_PAGE: &str = "https://auth.ducktape.industries/";

/// How long one touch may take before the app stops waiting.
pub(crate) const CEREMONY_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a consent stays good, in the node's milliseconds.
const CONSENT_TTL_MS: u64 = 15 * 60 * 1000;

fn auth_page() -> String {
    std::env::var("DUCKTAPE_AUTH_PAGE").unwrap_or_else(|_| AUTH_PAGE.to_owned())
}

// ---------- the flows ----------

/// A new account held by a passkey, with this device's (seated) key on it
/// too. Retry-safe: a device key that already holds an account keeps it,
/// and the passkey joins that one.
pub(crate) async fn create_account(
    client: &RpcClient,
    network: &str,
    name: &str,
) -> Result<(), String> {
    let device = seated_key().await.map_err(|refusal| refusal.sentence)?;
    let number = match ask(
        client,
        network,
        Query::OfKey {
            key: device.clone(),
        },
    )
    .await?
    {
        Reply::Number(Some(number)) => number,
        _ => {
            let create = Op::Create {
                name: name.to_owned(),
                scheme: abi::Scheme::Ed25519,
            };
            let output = submit_seated(client, network, &create).await?;
            abi::decode::<u64>(&output).map_err(|refusal| refusal.sentence)?
        }
    };
    let passkey = created(Request::Create {
        user: user_handle(network, number),
        name: format!("{name} · {network}"),
    })
    .await?;
    let admission = Admission {
        network: network.as_bytes().to_vec(),
        scheme: abi::Scheme::Secp256r1,
        key: passkey.clone(),
        generation: generation(client, network, &passkey).await?,
        account: number,
        expires_at: expires_at(),
    };
    let (device, proof) = seated_sign(CONSENT_NAMESPACE, &admission.preimage())
        .await
        .map_err(|refusal| refusal.sentence)?;
    let add = Op::AddKey {
        scheme: abi::Scheme::Secp256r1,
        label: Some("Passkey".into()),
        consent: Consent {
            key: device,
            account: number,
            expires_at: admission.expires_at,
            proof,
        },
    };
    let seq = next_seq(client, &passkey)
        .await
        .map_err(|refusal| refusal.sentence)?;
    let body = passkey_body(&passkey, network, seq, abi::encode(&add));
    let assertion = asserted(keyscheme::webauthn_challenge(
        FRAME_NAMESPACE,
        &body.preimage(),
    ))
    .await?;
    let frame = passkey_frame(body, &assertion)
        .ok_or("That was a different passkey than the one just made. Choose the new one.")?;
    submit(client, frame.encode()).await.map(drop)
}

/// This device's (seated) key joins the account a passkey holds.
pub(crate) async fn sign_in(client: &RpcClient, network: &str) -> Result<(), String> {
    let hint = asserted(rand::random()).await?;
    let number = account_of_handle(network, hint.user_handle.as_deref())?;
    let account = match ask(client, network, Query::Get { number }).await? {
        Reply::Account(Some(account)) => account,
        _ => {
            return Err(format!(
                "This passkey names an account {network} does not have."
            ));
        }
    };
    let device = seated_key().await.map_err(|refusal| refusal.sentence)?;
    let admission = Admission {
        network: network.as_bytes().to_vec(),
        scheme: abi::Scheme::Ed25519,
        key: device.clone(),
        generation: generation(client, network, &device).await?,
        account: number,
        expires_at: expires_at(),
    };
    let preimage = admission.preimage();
    let consent = asserted(keyscheme::webauthn_challenge(CONSENT_NAMESPACE, &preimage)).await?;
    let proof = consent.proof();
    let key = consenting_key(&account, &preimage, &proof)
        .ok_or("That passkey isn't on this account. Use the same passkey both times.")?;
    let add = Op::AddKey {
        scheme: abi::Scheme::Ed25519,
        label: Some("Desktop".into()),
        consent: Consent {
            key,
            account: number,
            expires_at: admission.expires_at,
            proof,
        },
    };
    submit_seated(client, network, &add).await.map(drop)
}

/// Which of `account`'s passkeys signed `proof` over the consent `preimage`
/// — the page does not say which credential answered.
fn consenting_key(account: &identity::Account, preimage: &[u8], proof: &[u8]) -> Option<Vec<u8>> {
    account
        .keys()
        .iter()
        .filter(|held| held.scheme == abi::Scheme::Secp256r1)
        .find(|held| KeyScheme::Secp256r1.verify(&held.key, CONSENT_NAMESPACE, preimage, proof))
        .map(|held| held.key.clone())
}

/// A frame body whose signer is the passkey `pubkey`.
fn passkey_body(pubkey: &[u8], network: &str, seq: u64, payload: Vec<u8>) -> Body {
    Body {
        scheme: KeyScheme::Secp256r1,
        signer: pubkey.to_vec(),
        network: network.as_bytes().to_vec(),
        seq,
        target: identity::PROGRAM.to_owned(),
        payload,
    }
}

/// The frame `body` completed by `assertion`, or `None` when the assertion
/// is not the body's signer signing it (another passkey answered).
fn passkey_frame(body: Body, assertion: &Assertion) -> Option<Frame> {
    let proof = assertion.proof();
    KeyScheme::Secp256r1
        .verify(&body.signer, FRAME_NAMESPACE, &body.preimage(), &proof)
        .then_some(Frame { body, proof })
}

// ---------- the node ----------

async fn ask(client: &RpcClient, network: &str, query: Query) -> Result<Reply, String> {
    let frame = query_frame(network, identity::PROGRAM, abi::encode(&query)).await;
    let reply = client
        .query(Layer::Preconfirmed, frame)
        .await
        .map_err(|error| node_error(error.to_string()))?;
    abi::decode(&reply).map_err(|refusal| refusal.sentence)
}

async fn generation(client: &RpcClient, network: &str, key: &[u8]) -> Result<u64, String> {
    match ask(client, network, Query::Generation { key: key.to_vec() }).await? {
        Reply::Generation(generation) => Ok(generation),
        _ => Err("identity answered something other than a generation".into()),
    }
}

/// A consent's deadline, against block time (unix ms). The node's status
/// names only its genesis time, so this reads this device's clock; the TTL
/// absorbs ordinary skew.
fn expires_at() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default();
    now + CONSENT_TTL_MS
}

async fn submit_seated(client: &RpcClient, network: &str, op: &Op) -> Result<Vec<u8>, String> {
    let frame = seated_frame(client, network, identity::PROGRAM, abi::encode(op))
        .await
        .map_err(|refusal| refusal.sentence)?;
    submit(client, frame).await
}

async fn submit(client: &RpcClient, frame: Vec<u8>) -> Result<Vec<u8>, String> {
    let receipt = client
        .submit(frame)
        .await
        .map_err(|error| node_error(error.to_string()))?;
    match receipt.outcome {
        abi::Outcome::Applied { output } => Ok(output),
        abi::Outcome::Rejected(refusal) => Err(node_error(refusal.sentence)),
    }
}

fn node_error(sentence: String) -> String {
    if sentence.contains("already belongs to an account") {
        return "This device's key already belongs to an account. Unlock it instead.".into();
    }
    if sentence.contains("expired") {
        return "That took too long and the consent expired. Try again.".into();
    }
    super::user_error(sentence)
}

// ---------- the account a passkey names ----------

/// `user.id`: `SHA-256("ducktape:passkey-account:v1\0" ‖ network)` ‖ the
/// account number, u64 LE.
fn user_handle(network: &str, number: u64) -> [u8; 40] {
    let mut handle = [0; 40];
    handle[..32].copy_from_slice(&chain_hash(network));
    handle[32..].copy_from_slice(&number.to_le_bytes());
    handle
}

fn chain_hash(network: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"ducktape:passkey-account:v1\0");
    hash.update(network.as_bytes());
    hash.finalize().into()
}

/// The account a passkey's (unsigned) `userHandle` names on `network`. A
/// hint only: the consent that follows must verify against a key the
/// account holds.
fn account_of_handle(network: &str, handle: Option<&[u8]>) -> Result<u64, String> {
    let handle = handle
        .and_then(|bytes| <&[u8; 40]>::try_from(bytes).ok())
        .ok_or("That passkey wasn't made by Ducktape. Choose a Ducktape passkey.")?;
    if handle[..32] != chain_hash(network) {
        return Err(format!(
            "That passkey belongs to another network. Choose one for {network}."
        ));
    }
    Ok(u64::from_le_bytes(
        handle[32..].try_into().expect("8 bytes"),
    ))
}

// ---------- the page ----------

enum Request {
    /// `navigator.credentials.create()`; the challenge is pass-through.
    Create { user: [u8; 40], name: String },
    /// `navigator.credentials.get()` with `allowCredentials: []`.
    Get { challenge: [u8; 32] },
}

fn request_url(page: &str, request: &Request, callback: &str) -> String {
    let fields = match request {
        Request::Create { user, name } => format!(
            "op=create&challenge={}&user={}&name={}",
            B64.encode(rand::random::<[u8; 32]>()),
            B64.encode(user),
            url_encode(name)
        ),
        Request::Get { challenge } => format!("op=get&challenge={}", B64.encode(challenge)),
    };
    format!("{page}#{fields}&cb={}", url_encode(callback))
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .map(
            |byte| match byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                true => (byte as char).to_string(),
                false => format!("%{byte:02X}"),
            },
        )
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// the 33-byte compressed SEC1 point
    Created(Vec<u8>),
    Asserted(Assertion),
}

#[derive(Debug, PartialEq, Eq)]
struct Assertion {
    authenticator_data: Vec<u8>,
    client_data_json: Vec<u8>,
    /// raw `R‖S`
    signature: Vec<u8>,
    user_handle: Option<Vec<u8>>,
}

impl Assertion {
    /// `keyscheme`'s `Secp256r1` proof bytes.
    fn proof(&self) -> Vec<u8> {
        keyscheme::webauthn_proof(
            &self.authenticator_data,
            &self.client_data_json,
            &self.signature,
        )
    }
}

fn parse_result(json: &str) -> Result<Outcome, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|_| "The browser sent back something unreadable. Try again.".to_string())?;
    if let Some(error) = value["error"].as_str() {
        return Err(ceremony_error(error));
    }
    let binary = |field: &str| {
        value[field]
            .as_str()
            .and_then(|text| B64.decode(text).ok())
            .ok_or_else(|| format!("The browser's answer is missing {field}. Try again."))
    };
    match value["op"].as_str() {
        Some("create") => {
            let key = binary("publicKey")?;
            match KeyScheme::Secp256r1.pubkey_wellformed(&key) {
                true => Ok(Outcome::Created(key)),
                false => Err("The browser returned a key Ducktape can't use.".into()),
            }
        }
        Some("get") => Ok(Outcome::Asserted(Assertion {
            authenticator_data: binary("authenticatorData")?,
            client_data_json: binary("clientDataJSON")?,
            signature: binary("signature")?,
            user_handle: binary("userHandle").ok(),
        })),
        _ => Err("The browser's answer names no step. Try again.".into()),
    }
}

/// A sentence for the page's `DOMException` name.
fn ceremony_error(name: &str) -> String {
    match name {
        "NotAllowedError" | "AbortError" => {
            "The passkey step was cancelled or timed out. Try again.".into()
        }
        "InvalidStateError" => {
            "That authenticator already holds a passkey for this account.".into()
        }
        "SecurityError" => "The browser refused a passkey on this page.".into(),
        "NotSupportedError" => "This browser or authenticator doesn't support passkeys.".into(),
        other => format!("The passkey step failed ({other}). Try again."),
    }
}

async fn created(request: Request) -> Result<Vec<u8>, String> {
    match ceremony(request).await? {
        Outcome::Created(key) => Ok(key),
        Outcome::Asserted(_) => Err("The browser answered a different step. Try again.".into()),
    }
}

async fn asserted(challenge: [u8; 32]) -> Result<Assertion, String> {
    match ceremony(Request::Get { challenge }).await? {
        Outcome::Asserted(assertion) => Ok(assertion),
        Outcome::Created(_) => Err("The browser answered a different step. Try again.".into()),
    }
}

/// One touch: open the page on `request`, wait for its answer.
async fn ceremony(request: Request) -> Result<Outcome, String> {
    let listener = Listener::bind()
        .await
        .map_err(|error| format!("Couldn't listen for the browser: {error}"))?;
    open_browser(&request_url(
        &auth_page(),
        &request,
        &listener.callback_url(),
    ))?;
    tokio::time::timeout(CEREMONY_TIMEOUT, listener.wait())
        .await
        .map_err(|_| "Nothing came back from the browser. Try again.".to_string())?
}

/// The system browser, or `DUCKTAPE_BROWSER <url>` when set (a test's
/// headless browser).
fn open_browser(url: &str) -> Result<(), String> {
    match std::env::var_os("DUCKTAPE_BROWSER") {
        Some(command) => std::process::Command::new(command)
            .arg(url)
            .spawn()
            .map(drop)
            .map_err(|error| format!("Couldn't open the browser: {error}")),
        None => {
            crate::shell::open_link(url.to_owned());
            Ok(())
        }
    }
}

// ---------- the loopback listener ----------

/// One-shot, on an ephemeral 127.0.0.1 port. The port is no secret, so the
/// callback path carries 32 random bytes and only a POST to that path ends
/// the wait.
struct Listener {
    listener: TcpListener,
    path: String,
}

impl Listener {
    async fn bind() -> std::io::Result<Listener> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let path = format!("/cb/{}", B64.encode(rand::random::<[u8; 32]>()));
        Ok(Listener { listener, path })
    }

    fn callback_url(&self) -> String {
        let port = self
            .listener
            .local_addr()
            .map(|addr| addr.port())
            .unwrap_or_default();
        format!("http://127.0.0.1:{port}{}", self.path)
    }

    async fn wait(self) -> Result<Outcome, String> {
        loop {
            let (mut stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|error| format!("Lost the browser's connection: {error}"))?;
            let Ok((method, path, body)) = read_request(&mut stream).await else {
                continue;
            };
            let result = (path == self.path && method == "POST")
                .then(|| form_field(&body, "result"))
                .flatten();
            let Some(result) = result else {
                respond(&mut stream, "404 Not Found", "Not found.").await;
                continue;
            };
            let outcome = parse_result(&result);
            let said = match outcome {
                Ok(_) => "Done. You can return to Ducktape.",
                Err(_) => "That didn't finish. Ducktape has the details.",
            };
            respond(&mut stream, "200 OK", said).await;
            return outcome;
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<(String, String, Vec<u8>)> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut length = 0usize;
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    // the page's answer is a few KiB; refuse to buffer anything absurd
    let mut body = vec![0; length.min(64 * 1024)];
    reader.read_exact(&mut body).await?;
    Ok((method, path, body))
}

async fn respond(stream: &mut TcpStream, status: &str, said: &str) {
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Ducktape</title>\
         <body style=\"font:16px system-ui;display:grid;place-items:center;min-height:90vh\">\
         <p>{said}</p>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

/// One `application/x-www-form-urlencoded` field, decoded.
fn form_field(body: &[u8], name: &str) -> Option<String> {
    body.split(|byte| *byte == b'&').find_map(|pair| {
        let at = pair.iter().position(|byte| *byte == b'=')?;
        (form_decode(&pair[..at]) == name.as_bytes())
            .then(|| String::from_utf8(form_decode(&pair[at + 1..])).ok())
            .flatten()
    })
}

fn form_decode(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    let mut i = 0;
    while i < value.len() {
        let escaped = (value[i] == b'%')
            .then(|| value.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match (value[i], escaped) {
            (_, Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', None) => {
                out.push(b' ');
                i += 1;
            }
            (byte, None) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyscheme::testkit;

    const RP: &str = "auth.ducktape.industries";

    /// The page's `get` answer, as JSON, for an assertion made the way an
    /// authenticator makes it.
    fn page_answer(
        sk: &p256::ecdsa::SigningKey,
        ns: &[u8],
        preimage: &[u8],
        handle: &[u8],
    ) -> Assertion {
        let (authenticator_data, client_data_json, signature) =
            testkit::passkey_assertion_parts(sk, RP, ns, preimage);
        let json = serde_json::json!({
            "op": "get",
            "credentialId": B64.encode([9; 16]),
            "authenticatorData": B64.encode(authenticator_data),
            "clientDataJSON": B64.encode(client_data_json),
            "signature": B64.encode(signature),
            "userHandle": B64.encode(handle),
        });
        match parse_result(&json.to_string()).unwrap() {
            Outcome::Asserted(assertion) => assertion,
            Outcome::Created(_) => unreachable!(),
        }
    }

    #[test]
    fn a_passkey_signed_frame_verifies_as_the_node_verifies_it() {
        let sk = testkit::passkey(3);
        let pubkey = testkit::passkey_pubkey(&sk);
        let body = passkey_body(&pubkey, "testkit", 0, vec![1, 2, 3]);
        let assertion = page_answer(&sk, FRAME_NAMESPACE, &body.preimage(), &[]);
        let frame = passkey_frame(body, &assertion).expect("its own signer");
        // the node decodes the bytes and runs `scheme.verify(signer, NS, preimage, proof)`
        let decoded: Frame = abi::decode(&frame.encode()).unwrap();
        assert_eq!(decoded.body.scheme, KeyScheme::Secp256r1);
        assert!(decoded.body.scheme.verify(
            &decoded.body.signer,
            FRAME_NAMESPACE,
            &decoded.body.preimage(),
            &decoded.proof
        ));

        // another passkey answering is caught before the node sees it
        let other = page_answer(
            &testkit::passkey(4),
            FRAME_NAMESPACE,
            &decoded.body.preimage(),
            &[],
        );
        assert!(passkey_frame(decoded.body, &other).is_none());
    }

    #[test]
    fn a_passkey_consent_verifies_and_names_the_key_that_gave_it() {
        let sk = testkit::passkey(5);
        let admission = Admission {
            network: b"testkit".to_vec(),
            scheme: abi::Scheme::Ed25519,
            key: vec![7; 32],
            generation: 0,
            account: 12,
            expires_at: 99,
        };
        let preimage = admission.preimage();
        let assertion = page_answer(
            &sk,
            CONSENT_NAMESPACE,
            &preimage,
            &user_handle("testkit", 12),
        );
        assert_eq!(
            account_of_handle("testkit", assertion.user_handle.as_deref()),
            Ok(12)
        );
        let key = |sk| identity::Key {
            scheme: abi::Scheme::Secp256r1,
            key: testkit::passkey_pubkey(sk),
            label: None,
            added_at: 0,
        };
        let account = identity::Account {
            number: 12,
            name: "ada".into(),
            control: identity::Control::Keys(vec![key(&testkit::passkey(6)), key(&sk)]),
            avatar: None,
            bio: None,
            updated_at: 0,
        };
        let proof = assertion.proof();
        let signer = consenting_key(&account, &preimage, &proof).unwrap();
        assert_eq!(signer, testkit::passkey_pubkey(&sk));
        assert!(KeyScheme::Secp256r1.verify(&signer, CONSENT_NAMESPACE, &preimage, &proof));
        // bound to its admission: another account's preimage does not verify
        let elsewhere = Admission {
            account: 13,
            ..admission
        }
        .preimage();
        assert!(consenting_key(&account, &elsewhere, &proof).is_none());
    }

    #[test]
    fn a_device_consent_admits_a_passkey() {
        use commonware_cryptography::Signer as _;
        let device = commonware_cryptography::ed25519::PrivateKey::from_seed(1);
        let preimage = b"admission".to_vec();
        let proof = device.sign(CONSENT_NAMESPACE, &preimage);
        assert!(KeyScheme::Ed25519.verify(
            device.public_key().as_ref(),
            CONSENT_NAMESPACE,
            &preimage,
            proof.as_ref()
        ));
    }

    #[test]
    fn a_user_handle_names_its_network_and_account() {
        let handle = user_handle("testkit", 258);
        assert_eq!(account_of_handle("testkit", Some(&handle)), Ok(258));
        assert!(account_of_handle("other", Some(&handle)).is_err());
        assert!(account_of_handle("testkit", None).is_err());
        assert!(account_of_handle("testkit", Some(&[1; 39])).is_err());
    }

    #[test]
    fn the_request_rides_the_fragment() {
        let url = request_url(
            AUTH_PAGE,
            &Request::Get {
                challenge: [0xff; 32],
            },
            "http://127.0.0.1:1/cb/x",
        );
        assert_eq!(
            url,
            format!(
                "{AUTH_PAGE}#op=get&challenge={}&cb=http%3A%2F%2F127.0.0.1%3A1%2Fcb%2Fx",
                B64.encode([0xff; 32])
            )
        );
        let create = request_url(
            AUTH_PAGE,
            &Request::Create {
                user: [1; 40],
                name: "ada · testkit".into(),
            },
            "cb",
        );
        assert!(create.contains("&name=ada%20%C2%B7%20testkit&cb=cb"));
        assert!(create.contains(&format!("&user={}&", B64.encode([1; 40]))));
    }

    #[test]
    fn a_page_error_reads_as_a_sentence() {
        let error = parse_result(r#"{"op":"get","error":"NotAllowedError","message":"x"}"#);
        assert_eq!(error, Err(ceremony_error("NotAllowedError")));
        let short_key = format!(r#"{{"op":"create","publicKey":"{}"}}"#, B64.encode([2; 32]));
        assert!(parse_result(&short_key).is_err());
    }

    #[tokio::test]
    async fn the_listener_takes_one_post_on_its_path() {
        let listener = Listener::bind().await.unwrap();
        let callback = listener.callback_url();
        let waiting = tokio::spawn(listener.wait());
        let http = reqwest::Client::new();
        let wrong = callback.rsplit_once('/').unwrap().0.to_owned() + "/nope";
        assert_eq!(
            http.post(&wrong).body("").send().await.unwrap().status(),
            404
        );
        let key = testkit::passkey_pubkey(&testkit::passkey(2));
        let result = format!(r#"{{"op":"create","publicKey":"{}"}}"#, B64.encode(&key));
        let form = format!("result={}", url_encode(&result));
        let answer = http
            .post(&callback)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form)
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), 200);
        assert_eq!(waiting.await.unwrap(), Ok(Outcome::Created(key)));
    }
}
