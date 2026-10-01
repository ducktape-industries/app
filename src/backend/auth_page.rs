//! The WebAuthn ceremony, run for the app by the auth page in the system
//! browser or on a phone: the request rides the page URL's fragment, the
//! answer comes back as a form POST to the loopback listener, or through
//! the relay slot the phone's page posts to. One [`ceremony`] is one touch;
//! the flows in `passkey` chain them. The page's contract is core's
//! `ops/auth-page/README.md` (`git -C core show b690a31bf^:ops/auth-page/README.md`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use keyscheme::KeyScheme;

use super::loopback::{Listener, url_encode};
use super::relay::Relay;

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
    /// `navigator.credentials.get()` with `allowCredentials: []`.
    Get { challenge: [u8; 32] },
}

pub(super) fn request_url(page: &str, request: &Request, callback: &str) -> String {
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

pub(super) async fn created(request: Request, phone: &Phone) -> Result<Vec<u8>, String> {
    match ceremony(request, phone).await? {
        Outcome::Created(key) => Ok(key),
        Outcome::Asserted(_) => Err("The browser answered a different step. Try again.".into()),
    }
}

pub(super) async fn asserted(challenge: [u8; 32], phone: &Phone) -> Result<Assertion, String> {
    match ceremony(Request::Get { challenge }, phone).await? {
        Outcome::Asserted(assertion) => Ok(assertion),
        Outcome::Created(_) => Err("The browser answered a different step. Try again.".into()),
    }
}

/// One touch: offer `request` as a QR for a phone, open it in this
/// device's browser unless the phone was picked, and take whichever answer
/// comes first.
async fn ceremony(request: Request, phone: &Phone) -> Result<Outcome, String> {
    let page = auth_page();
    let listener = Listener::bind()
        .await
        .map_err(|error| format!("Couldn't listen for the browser: {error}"))?;
    let relay = Relay::at(&page)?;
    phone.show(request_url(&page, &request, &relay.url));
    if !phone.chosen() {
        open_browser(&request_url(&page, &request, &listener.callback_url()))?;
    }
    answer(listener, relay, phone, CEREMONY_TIMEOUT).await
}

/// The first answer, from this device's browser or the phone's relay slot.
async fn answer(
    listener: Listener,
    relay: Relay,
    phone: &Phone,
    within: Duration,
) -> Result<Outcome, String> {
    let either = async {
        tokio::select! {
            outcome = listener.wait() => outcome,
            outcome = relay.wait(&phone.chosen) => outcome,
        }
    };
    tokio::time::timeout(within, either)
        .await
        .map_err(|_| "Nothing came back from the passkey. Try again.".to_string())?
}

/// The phone path of one flow's touches: each touch's QR URL goes out on
/// `shown`; the screen sets `chosen` once the person picks "Use a phone
/// instead". The flow holds the only sender, so the URLs end with it.
pub(crate) struct Phone {
    chosen: Arc<AtomicBool>,
    shown: futures::channel::mpsc::UnboundedSender<String>,
}

impl Phone {
    /// A phone path the screen picks through `chosen`, and the stream of QR
    /// URLs its touches show.
    pub(crate) fn new(
        chosen: Arc<AtomicBool>,
    ) -> (Phone, futures::channel::mpsc::UnboundedReceiver<String>) {
        let (shown, urls) = futures::channel::mpsc::unbounded();
        (Phone { chosen, shown }, urls)
    }

    fn chosen(&self) -> bool {
        self.chosen.load(Ordering::Relaxed)
    }

    fn show(&self, url: String) {
        let _ = self.shown.unbounded_send(url);
    }
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
    use super::super::loopback::read_request;
    use super::super::noded::FRAME_NAMESPACE;
    use super::super::passkey::{passkey_body, passkey_frame};
    use super::*;
    use keyscheme::testkit;
    use tokio::io::AsyncWriteExt as _;
    use tokio::net::TcpListener;

    const RP: &str = "auth.ducktape.industries";

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
        for (name, sentence) in [
            (
                "NotAllowedError",
                "The passkey step was cancelled or timed out. Try again.",
            ),
            (
                "InvalidStateError",
                "That authenticator already holds a passkey for this account.",
            ),
            (
                "SomethingElse",
                "The passkey step failed (SomethingElse). Try again.",
            ),
        ] {
            let page = format!(r#"{{"op":"get","error":"{name}","message":"x"}}"#);
            assert_eq!(parse_result(&page), Err(sentence.into()), "{name}");
        }
    }

    /// A fake auth host: each GET of `/r/<id>` takes the next of `answers`
    /// (then 204s); `polls` counts them. It answers one request at a time,
    /// in the order they connected, so [`barrier`] is served only after
    /// every poll already on the wire has been counted.
    async fn fake_relay(
        answers: Vec<(&'static str, String)>,
    ) -> (String, tokio::sync::watch::Receiver<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let page = format!("http://{}/.duck/auth", listener.local_addr().unwrap());
        let (count, polls) = tokio::sync::watch::channel(0);
        tokio::spawn(async move {
            let mut answers = answers.into_iter();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                // a poll cut off by a cancel sends nothing whole
                let Ok((method, path, _)) = read_request(&mut stream).await else {
                    continue;
                };
                assert_eq!(method, "GET");
                let (status, body) = match path.as_str() {
                    "/barrier" => ("204 No Content", String::new()),
                    _ => {
                        assert!(path.starts_with("/r/") && path.len() == 46, "{path}");
                        count.send_modify(|polls| *polls += 1);
                        answers.next().unwrap_or(("204 No Content", String::new()))
                    }
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        (page, polls)
    }

    /// Returns once the fake host has answered every request that reached
    /// it before this one.
    async fn barrier(page: &str) {
        let origin = page.trim_end_matches("/.duck/auth");
        reqwest::get(format!("{origin}/barrier")).await.unwrap();
    }

    /// A relay polling every 10 ms, and a phone already picked.
    fn phone_relay(page: &str) -> (Relay, Phone) {
        let relay = Relay {
            every: Duration::from_millis(10),
            ..Relay::at(page).unwrap()
        };
        (relay, Phone::new(Arc::new(AtomicBool::new(true))).0)
    }

    #[tokio::test]
    async fn the_relay_polls_through_204s_to_the_phones_answer() {
        let key = testkit::passkey_pubkey(&testkit::passkey(2));
        let created = format!(r#"{{"op":"create","publicKey":"{}"}}"#, B64.encode(&key));
        let (page, polls) = fake_relay(vec![
            ("204 No Content", String::new()),
            ("204 No Content", String::new()),
            ("200 OK", created),
        ])
        .await;
        let (relay, phone) = phone_relay(&page);
        let listener = Listener::bind().await.unwrap();
        let outcome = answer(listener, relay, &phone, Duration::from_secs(10)).await;
        assert_eq!(outcome, Ok(Outcome::Created(key)));
        assert_eq!(*polls.borrow(), 3);
    }

    #[tokio::test]
    async fn the_relay_is_not_polled_until_the_phone_is_picked_and_then_times_out() {
        let (page, mut polls) = fake_relay(vec![]).await;
        let chosen = Arc::new(AtomicBool::new(false));
        let phone = Phone::new(chosen.clone()).0;
        // never picked: the ceremony times out, the slot never asked
        let (relay, _) = phone_relay(&page);
        let listener = Listener::bind().await.unwrap();
        let outcome = answer(listener, relay, &phone, Duration::from_millis(100)).await;
        assert_eq!(
            outcome,
            Err("Nothing came back from the passkey. Try again.".into())
        );
        barrier(&page).await;
        assert_eq!(*polls.borrow(), 0);
        // picked: the slot is asked
        chosen.store(true, Ordering::Relaxed);
        let (relay, _) = phone_relay(&page);
        let listener = Listener::bind().await.unwrap();
        tokio::select! {
            outcome = answer(listener, relay, &phone, Duration::from_secs(60)) => {
                panic!("answered with nothing to answer: {outcome:?}")
            }
            polled = polls.wait_for(|polls| *polls > 0) => {
                polled.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn cancelling_stops_the_polling() {
        let (page, mut polls) = fake_relay(vec![]).await;
        let (relay, phone) = phone_relay(&page);
        let listener = Listener::bind().await.unwrap();
        let waiting =
            tokio::spawn(
                async move { answer(listener, relay, &phone, Duration::from_secs(60)).await },
            );
        polls.wait_for(|polls| *polls > 0).await.unwrap();
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        barrier(&page).await;
        let stopped = *polls.borrow();
        // ten of the relay's periods: a poll here would be one after the
        // cancel (a pause can only hide one, never invent one)
        tokio::time::sleep(Duration::from_millis(100)).await;
        barrier(&page).await;
        assert_eq!(*polls.borrow(), stopped);
    }

    #[tokio::test]
    async fn a_relayed_answer_that_fails_verification_is_refused() {
        // a key no P-256 verifier accepts
        let bad_key = format!(r#"{{"op":"create","publicKey":"{}"}}"#, B64.encode([2; 32]));
        // an assertion by another passkey than the frame's signer
        let body = passkey_body(
            &testkit::passkey_pubkey(&testkit::passkey(3)),
            "testkit",
            0,
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
        let (page, _) = fake_relay(vec![("200 OK", bad_key), ("200 OK", stranger)]).await;

        let (relay, phone) = phone_relay(&page);
        let outcome = answer(
            Listener::bind().await.unwrap(),
            relay,
            &phone,
            Duration::from_secs(10),
        )
        .await;
        assert_eq!(
            outcome,
            Err("The browser returned a key Ducktape can't use.".into())
        );

        let (relay, phone) = phone_relay(&page);
        let Ok(Outcome::Asserted(assertion)) = answer(
            Listener::bind().await.unwrap(),
            relay,
            &phone,
            Duration::from_secs(10),
        )
        .await
        else {
            panic!("an assertion");
        };
        assert!(passkey_frame(body, &assertion).is_none());
    }

    #[tokio::test]
    async fn a_relay_refusal_reads_as_a_sentence() {
        let (page, _) = fake_relay(vec![("404 Not Found", String::new())]).await;
        let (relay, phone) = phone_relay(&page);
        let outcome = answer(
            Listener::bind().await.unwrap(),
            relay,
            &phone,
            Duration::from_secs(10),
        )
        .await;
        assert!(outcome.unwrap_err().starts_with("The auth host refused"));
    }
}
