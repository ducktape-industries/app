//! The WebAuthn ceremony, run for the app by the auth page in the system
//! browser: the request rides the page URL's fragment, the answer comes
//! back as a form POST to the loopback listener (the only place the page
//! sends it). One [`ceremony`] is one touch; the flows in `passkey` chain
//! them. The page and its contract live in `ducktape-industries/auth` (its
//! README).

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use keyscheme::KeyScheme;

use super::loopback::{Listener, url_encode};

/// The live page. Its host IS the RP ID every passkey is scoped to.
const AUTH_PAGE: &str = "https://auth.ducktape.industries/";

/// How long one touch may take before the app stops waiting.
const CEREMONY_TIMEOUT: Duration = Duration::from_secs(300);

pub(super) fn auth_page() -> String {
    std::env::var("DUCKTAPE_AUTH_PAGE").unwrap_or_else(|_| AUTH_PAGE.to_owned())
}

pub(super) enum Request {
    /// `navigator.credentials.create()`; the challenge is pass-through.
    Create { user: [u8; 40], name: String },
    /// `navigator.credentials.get()` with `allowCredentials: []`; `step`
    /// is its place in the flow (`"1/2"`), for the page's words.
    Get {
        challenge: [u8; 32],
        step: &'static str,
    },
}

pub(super) fn request_url(page: &str, request: &Request, callback: &str) -> String {
    let fields = match request {
        // a passkey is always made first of two touches (`passkey`)
        Request::Create { user, name } => format!(
            "op=create&challenge={}&user={}&name={}&step=1/2",
            B64.encode(rand::random::<[u8; 32]>()),
            B64.encode(user),
            url_encode(name)
        ),
        Request::Get { challenge, step } => {
            format!("op=get&challenge={}&step={step}", B64.encode(challenge))
        }
    };
    format!("{page}#{fields}&cb={}", url_encode(callback))
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// the 33-byte compressed SEC1 point
    Created(Vec<u8>),
    Asserted(Assertion),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Assertion {
    authenticator_data: Vec<u8>,
    client_data_json: Vec<u8>,
    /// raw `R‖S`
    signature: Vec<u8>,
    pub(super) user_handle: Option<Vec<u8>>,
}

impl Assertion {
    /// `keyscheme`'s `Secp256r1` proof bytes.
    pub(super) fn proof(&self) -> Vec<u8> {
        keyscheme::webauthn_proof(
            &self.authenticator_data,
            &self.client_data_json,
            &self.signature,
        )
    }
}

pub(super) fn parse_result(json: &str) -> Result<Outcome, String> {
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

pub(super) async fn created(request: Request) -> Result<Vec<u8>, String> {
    match ceremony(request).await? {
        Outcome::Created(key) => Ok(key),
        Outcome::Asserted(_) => Err("The browser answered a different step. Try again.".into()),
    }
}

pub(super) async fn asserted(challenge: [u8; 32], step: &'static str) -> Result<Assertion, String> {
    match ceremony(Request::Get { challenge, step }).await? {
        Outcome::Asserted(assertion) => Ok(assertion),
        Outcome::Created(_) => Err("The browser answered a different step. Try again.".into()),
    }
}

/// One touch: open `request` in this device's browser and take the page's
/// answer on loopback.
async fn ceremony(request: Request) -> Result<Outcome, String> {
    let page = auth_page();
    let listener = Listener::bind(&page)
        .await
        .map_err(|error| format!("Couldn't listen for the browser: {error}"))?;
    open_browser(&request_url(&page, &request, &listener.callback_url()))?;
    tokio::time::timeout(CEREMONY_TIMEOUT, listener.wait())
        .await
        .map_err(|_| "Nothing came back from the passkey. Try again.".to_string())?
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
        None => match OPEN_URL.get() {
            Some(open) => {
                open(url.to_owned());
                Ok(())
            }
            None => {
                tracing::error!(target: "ducktape::auth", reason = "no_url_opener", "the browser page could not be opened");
                Ok(())
            }
        },
    }
}

/// What opens a page in the system browser: the shell's, handed over at
/// startup ([`on_open_url`]) so this layer never calls up into it.
static OPEN_URL: std::sync::OnceLock<fn(String)> = std::sync::OnceLock::new();

/// The shell says how a page reaches the system browser.
pub(crate) fn on_open_url(open: fn(String)) {
    let _ = OPEN_URL.set(open);
}

#[cfg(test)]
mod tests {
    use super::super::passkey::{passkey_body, passkey_frame};
    use super::*;
    use keyscheme::testkit;
    use noded_client::wire::{FRAME_NAMESPACE, Tip};

    const RP: &str = "auth.ducktape.industries";

    #[test]
    fn the_request_rides_the_fragment() {
        let url = request_url(
            AUTH_PAGE,
            &Request::Get {
                challenge: [0xff; 32],
                step: "2/2",
            },
            "http://127.0.0.1:1/cb/x",
        );
        assert_eq!(
            url,
            format!(
                "{AUTH_PAGE}#op=get&challenge={}&step=2/2&cb=http%3A%2F%2F127.0.0.1%3A1%2Fcb%2Fx",
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
        assert!(create.contains("&name=ada%20%C2%B7%20testkit&step=1/2&cb=cb"));
        assert!(create.contains(&format!("&user={}&", B64.encode([1; 40]))));
    }

    #[test]
    fn a_page_error_reads_as_a_sentence() {
        let error = parse_result(r#"{"op":"get","error":"NotAllowedError","message":"x"}"#);
        assert_eq!(error, Err(ceremony_error("NotAllowedError")));
        let short_key = format!(r#"{{"op":"create","publicKey":"{}"}}"#, B64.encode([2; 32]));
        assert!(parse_result(&short_key).is_err());
    }

    #[test]
    fn an_answer_that_fails_verification_is_refused() {
        // a key no P-256 verifier accepts
        let bad_key = format!(r#"{{"op":"create","publicKey":"{}"}}"#, B64.encode([2; 32]));
        // an assertion by another passkey than the frame's signer
        let body = passkey_body(
            &testkit::passkey_pubkey(&testkit::passkey(3)),
            "testkit",
            Tip {
                height: 0,
                id: [0; 32],
            },
            vec![1],
        );
        let (authenticator_data, client_data_json, signature) = testkit::passkey_assertion_parts(
            &testkit::passkey(4),
            RP,
            FRAME_NAMESPACE,
            &body.preimage(),
        );
        let stranger = serde_json::json!({
            "op": "get",
            "authenticatorData": B64.encode(authenticator_data),
            "clientDataJSON": B64.encode(client_data_json),
            "signature": B64.encode(signature),
        })
        .to_string();
        assert_eq!(
            parse_result(&bad_key),
            Err("The browser returned a key Ducktape can't use.".into())
        );
        let Ok(Outcome::Asserted(assertion)) = parse_result(&stranger) else {
            panic!("an assertion");
        };
        assert!(passkey_frame(body, &assertion).is_none());
    }
}
