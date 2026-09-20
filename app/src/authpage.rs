//! The app-owned client half of the browser authentication ceremony.
//!
//! The auth page is a public HTTP/fragment contract. Keep the small client
//! codec here so the desktop app does not link the Core authpage crate (and
//! its module wire dependency).

use std::net::Ipv4Addr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::interfaces::identity;

pub const AUTH_PAGE: &str = "https://auth.ducktape.industries/";
pub const REVEAL_NS: &[u8] = b"ducktape:reveal-key:v1";
const MAX_BODY_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserHandle {
    chain_hash: [u8; 32],
    number: u64,
}

impl UserHandle {
    pub fn new(chain_id: &str, number: u64) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"ducktape:passkey-account:v1\0");
        hash.update(chain_id.as_bytes());
        Self {
            chain_hash: hash.finalize().into(),
            number,
        }
    }

    fn to_bytes(self) -> [u8; 40] {
        let mut bytes = [0; 40];
        bytes[..32].copy_from_slice(&self.chain_hash);
        bytes[32..].copy_from_slice(&self.number.to_le_bytes());
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Create {
        challenge: [u8; 32],
        user: u64,
        chain_id: String,
        name: String,
    },
    Get {
        challenge: [u8; 32],
    },
    Eth {
        message: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Create {
        credential_id: Vec<u8>,
        public_key: Vec<u8>,
    },
    Get {
        authenticator_data: Vec<u8>,
        client_data_json: Vec<u8>,
        signature: Vec<u8>,
        user_handle: Option<UserHandle>,
    },
    Eth {
        address: String,
        signature: Vec<u8>,
        message: Vec<u8>,
    },
}

pub fn request_url(page: &str, request: &Request, callback: &str) -> String {
    let mut params = Vec::new();
    match request {
        Request::Create {
            challenge,
            user,
            chain_id,
            name,
        } => {
            params.push("op=create".to_string());
            params.push(format!("challenge={}", B64.encode(challenge)));
            params.push(format!(
                "user={}",
                B64.encode(UserHandle::new(chain_id, *user).to_bytes())
            ));
            params.push(format!(
                "name={}",
                url_encode(&format!("{name} · {chain_id}"))
            ));
        }
        Request::Get { challenge } => {
            params.push("op=get".to_string());
            params.push(format!("challenge={}", B64.encode(challenge)));
        }
        Request::Eth { message } => {
            params.push("op=eth".to_string());
            params.push(format!("challenge={}", B64.encode(message)));
        }
    }
    params.push(format!("cb={}", url_encode(callback)));
    format!("{page}#{}", params.join("&"))
}

fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn create_challenge() -> [u8; 32] {
    rand::random()
}

pub fn reveal_message() -> Vec<u8> {
    let nonce: [u8; 16] = rand::random();
    let mut message = REVEAL_NS.to_vec();
    message.extend_from_slice(&nonce);
    message
}

pub fn parse_result(json: &str) -> Result<Outcome, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("auth page result is not JSON: {e}"))?;
    let op = value["op"].as_str().unwrap_or_default().to_string();
    if let Some(error) = value.get("error") {
        return Err(format!(
            "the {op} ceremony failed: {}: {}",
            error.as_str().unwrap_or("error"),
            value["message"].as_str().unwrap_or_default()
        ));
    }
    match op.as_str() {
        "create" => {
            let public_key = binary(&value, "publicKey")?;
            if !keyscheme::KeyScheme::Secp256r1.pubkey_wellformed(&public_key) {
                return Err(format!(
                    "the auth page returned {} bytes that are not a compressed SEC1 P-256 point",
                    public_key.len()
                ));
            }
            Ok(Outcome::Create {
                credential_id: binary(&value, "credentialId")?,
                public_key,
            })
        }
        "get" => Ok(Outcome::Get {
            authenticator_data: binary(&value, "authenticatorData")?,
            client_data_json: binary(&value, "clientDataJSON")?,
            signature: binary(&value, "signature")?,
            user_handle: user_handle(&value)?,
        }),
        "eth" => Ok(Outcome::Eth {
            address: value["address"].as_str().unwrap_or_default().to_string(),
            signature: hex_0x(value["signature"].as_str().unwrap_or_default())?,
            message: binary(&value, "message")?,
        }),
        other => Err(format!("auth page result names an unknown op {other:?}")),
    }
}

fn binary(value: &serde_json::Value, field: &str) -> Result<Vec<u8>, String> {
    let Some(text) = value[field].as_str() else {
        return Err(format!("auth page result is missing {field:?}"));
    };
    B64.decode(text)
        .map_err(|e| format!("auth page result field {field:?} is not base64url: {e}"))
}

fn user_handle(value: &serde_json::Value) -> Result<Option<UserHandle>, String> {
    let Some(text) = value["userHandle"].as_str() else {
        return Ok(None);
    };
    let bytes = B64
        .decode(text)
        .map_err(|e| format!("auth page userHandle is not base64url: {e}"))?;
    let Ok(bytes) = <[u8; 40]>::try_from(bytes.as_slice()) else {
        return Err(
            "the passkey has an unsupported userHandle; recreate it with ducktape account key add --passkey from a member device".into(),
        );
    };
    let mut chain_hash = [0; 32];
    chain_hash.copy_from_slice(&bytes[..32]);
    let mut number = [0; 8];
    number.copy_from_slice(&bytes[32..]);
    Ok(Some(UserHandle {
        chain_hash,
        number: u64::from_le_bytes(number),
    }))
}

fn hex_0x(text: &str) -> Result<Vec<u8>, String> {
    let hex = text.strip_prefix("0x").unwrap_or(text);
    if !hex.len().is_multiple_of(2) {
        return Err("auth page eth signature has odd hex length".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| format!("auth page eth signature is not hex: {e}"))
        })
        .collect()
}

pub struct Listener {
    listener: TcpListener,
    state: String,
}

impl Listener {
    pub async fn bind() -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let raw: [u8; 32] = rand::random();
        Ok(Self {
            listener,
            state: B64.encode(raw),
        })
    }

    pub fn callback_url(&self) -> String {
        let port = self
            .listener
            .local_addr()
            .map(|addr| addr.port())
            .unwrap_or_default();
        format!("http://127.0.0.1:{port}/cb/{}", self.state)
    }

    pub async fn wait(self) -> Result<Outcome, String> {
        let path = format!("/cb/{}", self.state);
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|e| format!("auth callback listener: {e}"))?;
            if let Some(outcome) = serve_one(stream, &path).await? {
                return Ok(outcome);
            }
        }
    }
}

async fn serve_one(mut stream: TcpStream, expected_path: &str) -> Result<Option<Outcome>, String> {
    let (method, path, body) = read_request(&mut stream).await?;
    if path != expected_path {
        respond(&mut stream, 404, "Not found.").await;
        return Ok(None);
    }
    if method != "POST" {
        respond(&mut stream, 200, "Waiting for the ceremony to finish…").await;
        return Ok(None);
    }
    let Some(result) = form_field(&body, "result") else {
        respond(&mut stream, 400, "The callback carried no result.").await;
        return Ok(None);
    };
    match parse_result(&result) {
        Ok(outcome) => {
            respond(&mut stream, 200, "Done — you can return to ducktape.").await;
            Ok(Some(outcome))
        }
        Err(message) => {
            respond(
                &mut stream,
                200,
                "The ceremony did not complete; ducktape has the details.",
            )
            .await;
            Err(message)
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| format!("auth callback: {e}"))?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut content_length = 0usize;
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("auth callback: {e}"))?;
        if read == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > MAX_BODY_BYTES {
        return Err(format!("auth callback body exceeds {MAX_BODY_BYTES} bytes"));
    }
    let mut body = vec![0u8; content_length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|e| format!("auth callback body: {e}"))?;
    Ok((method, path, body))
}

async fn respond(stream: &mut TcpStream, status: u16, text: &str) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Bad Request",
    };
    // the same card the page and the relay's "Done" wear (ops/auth-page).
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>ducktape</title>\
         <style>:root{{color-scheme:light dark;--fg:#1b1b1f;--bg:#f3f3f6;--card:#fff;--muted:#6b6b76;--line:#e2e2e8}}\
         @media (prefers-color-scheme:dark){{:root{{--fg:#ececf1;--bg:#111114;--card:#1b1b20;--muted:#9a9aa6;--line:#2a2a33}}}}\
         body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--fg);\
         font:16px/1.5 system-ui,-apple-system,\"Segoe UI\",sans-serif}}\
         main{{width:min(26rem,calc(100vw - 2rem));background:var(--card);border:1px solid var(--line);border-radius:16px;padding:2rem}}\
         .brand{{font-size:.8rem;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:var(--muted)}}\
         p{{margin:1rem 0 0}}</style></head>\
         <body><main><div class=\"brand\">🦆 ducktape</div><p>{text}</p></main></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    // the page has already delivered its result; a peer that hung up before
    // reading the acknowledgement lost nothing.
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

fn form_field(body: &[u8], name: &str) -> Option<String> {
    body.split(|b| *b == b'&').find_map(|pair| {
        let at = pair.iter().position(|b| *b == b'=')?;
        if form_decode(&pair[..at]) != name.as_bytes() {
            return None;
        }
        String::from_utf8(form_decode(&pair[at + 1..])).ok()
    })
}

fn form_decode(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    let mut i = 0;
    while i < value.len() {
        if value[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        if value[i] == b'%'
            && i + 2 < value.len()
            && let Ok(decoded) = u8::from_str_radix(
                std::str::from_utf8(&value[i + 1..i + 3]).unwrap_or_default(),
                16,
            )
        {
            out.push(decoded);
            i += 3;
            continue;
        }
        out.push(value[i]);
        i += 1;
    }
    out
}

pub fn open_browser(url: &str) -> bool {
    let attempts: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else if cfg!(target_os = "windows") {
        &[("cmd", &["/C", "start", ""])]
    } else {
        &[("xdg-open", &[])]
    };
    attempts.iter().any(|(program, args)| {
        std::process::Command::new(program)
            .args(*args)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
    })
}

pub const RELAY_POLL: Duration = Duration::from_millis(1500);

pub struct Relay {
    base: String,
    pub id: String,
}

impl Default for Relay {
    fn default() -> Self {
        Self::new()
    }
}

impl Relay {
    pub fn new() -> Self {
        Self::at(AUTH_PAGE)
    }

    pub fn at(base: &str) -> Self {
        let raw: [u8; 32] = rand::random();
        Self {
            base: base.to_string(),
            id: B64.encode(raw),
        }
    }

    pub fn callback_url(&self) -> String {
        format!("{}r/{}", self.base, self.id)
    }

    pub async fn wait(self, deadline: Duration) -> Result<Outcome, String> {
        self.wait_reporting(deadline, |_| {}).await
    }

    pub async fn wait_reporting(
        self,
        deadline: Duration,
        mut progress: impl FnMut(Duration),
    ) -> Result<Outcome, String> {
        let url = self.callback_url();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("relay client: {e}"))?;
        let started = tokio::time::Instant::now();
        let polling = async {
            loop {
                progress(deadline.saturating_sub(started.elapsed()));
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|error| format!("relay: {}", error.without_url()))?;
                match response.status().as_u16() {
                    200 => {
                        let body = response
                            .text()
                            .await
                            .map_err(|error| format!("relay body: {}", error.without_url()))?;
                        return parse_result(&body);
                    }
                    204 => {}
                    other => return Err(format!("relay answered {other}")),
                }
                tokio::time::sleep(RELAY_POLL).await;
            }
        };
        tokio::time::timeout_at(started + deadline, polling)
            .await
            .map_err(|_| "the phone did not answer in time".to_string())?
    }
}

pub fn countdown(left: Duration) -> String {
    let secs = left.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub fn passkey_frame_request(pubkey: &[u8], seq: u64, msg: &sdk::Msg) -> (Request, Vec<u8>) {
    let preimage = node::frame_preimage(keyscheme::KeyScheme::Secp256r1, pubkey, seq, msg);
    let challenge = keyscheme::webauthn_challenge(node::FRAME_NS, &preimage);
    (Request::Get { challenge }, preimage)
}

pub fn passkey_frame(mut preimage: Vec<u8>, outcome: &Outcome) -> Result<Vec<u8>, String> {
    let Outcome::Get {
        authenticator_data,
        client_data_json,
        signature,
        ..
    } = outcome
    else {
        return Err("expected a passkey assertion (op=get)".into());
    };
    preimage.extend_from_slice(&keyscheme::webauthn_proof(
        authenticator_data,
        client_data_json,
        signature,
    ));
    Ok(preimage)
}

pub fn wallet_frame_request(pubkey: &[u8], seq: u64, msg: &sdk::Msg) -> (Request, Vec<u8>) {
    let preimage = node::frame_preimage(keyscheme::KeyScheme::Secp256k1, pubkey, seq, msg);
    let message = keyscheme::personal_message(node::FRAME_NS, &preimage);
    (Request::Eth { message }, preimage)
}

pub fn wallet_frame(mut preimage: Vec<u8>, outcome: &Outcome) -> Result<Vec<u8>, String> {
    let Outcome::Eth { signature, .. } = outcome else {
        return Err("expected a wallet signature (op=eth)".into());
    };
    if signature.len() != 65 {
        return Err(format!(
            "a wallet signature is 65 bytes (r‖s‖v), got {}",
            signature.len()
        ));
    }
    preimage.extend_from_slice(signature);
    Ok(preimage)
}

pub fn wallet_pubkey(reveal: &[u8], outcome: &Outcome) -> Result<Vec<u8>, String> {
    let Outcome::Eth {
        signature, message, ..
    } = outcome
    else {
        return Err("expected a wallet signature (op=eth)".into());
    };
    if message != reveal {
        return Err("the wallet signed a different message than the key reveal".into());
    }
    keyscheme::recover_personal_sign(message, signature)
        .ok_or_else(|| "the wallet signature does not recover to a key".to_string())
}

pub fn account_request() -> Request {
    Request::Get {
        challenge: create_challenge(),
    }
}

pub fn assertion_account(chain_id: &str, outcome: &Outcome) -> Result<u64, String> {
    let Outcome::Get { user_handle, .. } = outcome else {
        return Err("expected a passkey assertion (op=get)".into());
    };
    let Some(handle) = user_handle else {
        return Err("the passkey names no account (no userHandle) — register it with ducktape account key add --passkey from a member device".into());
    };
    let expected = UserHandle::new(chain_id, handle.number);
    if handle.chain_hash != expected.chain_hash {
        return Err(format!(
            "this passkey belongs to a different chain; select a passkey for {chain_id}"
        ));
    }
    Ok(handle.number)
}

pub fn login_request(
    chain_id: &str,
    device_key: &[u8],
    generation: u64,
    account: u64,
    expires_at: u64,
) -> Request {
    let preimage = identity::add_key_preimage(
        chain_id,
        identity::KeyScheme::Ed25519,
        device_key,
        generation,
        account,
        expires_at,
    );
    let challenge = keyscheme::webauthn_challenge(identity::IDENTITY_ADD_KEY_NS, &preimage);
    Request::Get { challenge }
}

pub fn login_consent(chain_id: &str, outcome: &Outcome) -> Result<(u64, Vec<u8>), String> {
    let number = assertion_account(chain_id, outcome)?;
    let Outcome::Get {
        authenticator_data,
        client_data_json,
        signature,
        ..
    } = outcome
    else {
        return Err("expected a passkey assertion (op=get)".into());
    };
    Ok((
        number,
        keyscheme::webauthn_proof(authenticator_data, client_data_json, signature),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_url_and_handle_match_the_auth_page_contract() {
        let url = request_url(
            AUTH_PAGE,
            &Request::Create {
                challenge: [7; 32],
                user: 42,
                chain_id: "demo#a1b2c3d4".into(),
                name: "Duck & Co".into(),
            },
            "http://127.0.0.1:9/cb/state",
        );
        assert!(url.starts_with("https://auth.ducktape.industries/#op=create&challenge=Bwc"));
        assert!(url.contains("user="));
        assert!(url.contains("name=Duck%20%26%20Co%20%C2%B7%20demo%23a1b2c3d4"));
        assert!(url.contains("cb=http%3A%2F%2F127.0.0.1%3A9%2Fcb%2Fstate"));
    }

    #[test]
    fn result_decoder_pins_golden_eth_bytes() {
        let result = parse_result(
            r#"{"op":"eth","address":"0xabc","signature":"0x0102ff","message":"AQI"}"#,
        )
        .unwrap();
        assert_eq!(
            result,
            Outcome::Eth {
                address: "0xabc".into(),
                signature: vec![1, 2, 255],
                message: vec![1, 2],
            }
        );
    }
}
