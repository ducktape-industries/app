//! The auth host's relay slots, `/r/<id>`: a one-shot mailbox — POST
//! stores a `result`, GET hands it out once, 204 until then. A passkey
//! touch on a phone answers through a slot the app minted at random
//! ([`Relay`]); two devices joining pass a request and a consent through
//! two slots named from the code ([`slot`], [`post`], [`take`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use sha2::{Digest as _, Sha256};

use super::auth_page::{Outcome, parse_result};
use super::loopback::url_encode;

/// How often a slot is asked for its answer.
pub(super) const POLL: Duration = Duration::from_millis(1500);

/// The slot `id` on `page`'s origin — the page accepts only its own.
fn slot_url(page: &str, id: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(page)
        .map_err(|error| format!("The auth host address is unusable: {error}"))?;
    url.set_path(&format!("/r/{id}"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

/// One touch's slot on the auth host, `/r/<id>`: 32 random bytes the app
/// mints (an unguessable id is the slot's only lock), which the phone's
/// page POSTs to and the app polls. The body is public data every flow
/// verifies against the account's keys, so a forged post only fails there.
pub(super) struct Relay {
    pub(super) http: reqwest::Client,
    pub(super) url: String,
    pub(super) every: Duration,
}

impl Relay {
    /// A fresh slot on `page`'s origin — the page accepts only its own.
    pub(super) fn at(page: &str) -> Result<Relay, String> {
        Ok(Relay {
            http: reqwest::Client::new(),
            url: slot_url(page, &B64.encode(rand::random::<[u8; 32]>()))?,
            every: POLL,
        })
    }

    /// Polls while `chosen` (the phone was picked) until the slot answers.
    /// An unreachable host is retried; the ceremony's timeout bounds it.
    pub(super) async fn wait(self, chosen: &AtomicBool) -> Result<Outcome, String> {
        loop {
            if chosen.load(Ordering::Relaxed) {
                match self.http.get(&self.url).send().await {
                    Ok(reply) if reply.status() == reqwest::StatusCode::OK => {
                        let body = reply.text().await.map_err(|_| {
                            "Lost the phone's answer on the way. Try again.".to_string()
                        })?;
                        return parse_result(&body);
                    }
                    Ok(reply) if reply.status() == reqwest::StatusCode::NO_CONTENT => {}
                    Ok(reply) => {
                        return Err(format!(
                            "The auth host refused to relay the phone's answer ({}). Try again.",
                            reply.status()
                        ));
                    }
                    Err(error) => {
                        tracing::debug!(target: "ducktape::auth", event = "relay_unreachable", %error);
                    }
                }
            }
            tokio::time::sleep(self.every).await;
        }
    }
}

/// The relay slot for one side of a code: `request` (new → old) or
/// `consent` (old → new).
pub(super) fn slot(page: &str, code: &str, side: &str) -> Result<String, String> {
    let mut hash = Sha256::new();
    hash.update(b"ducktape:link:v1\0");
    hash.update(side.as_bytes());
    hash.update(b"\0");
    hash.update(code.as_bytes());
    slot_url(page, &B64.encode(hash.finalize()))
}

pub(super) async fn post(url: &str, json: String) -> Result<(), String> {
    let reply = reqwest::Client::new()
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("result={}", url_encode(&json)))
        .send()
        .await
        .map_err(|_| {
            "Can't reach the auth host. Check the connection and try again.".to_string()
        })?;
    match reply.status().is_success() {
        true => Ok(()),
        false => Err(format!(
            "The auth host refused the message ({}).",
            reply.status()
        )),
    }
}

/// One GET of a slot: its message, or `None` while nothing has arrived.
pub(super) async fn take(url: &str) -> Result<Option<String>, String> {
    let reply = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|_| "Can't reach the auth host.".to_string())?;
    match reply.status() {
        reqwest::StatusCode::OK => reply
            .text()
            .await
            .map(Some)
            .map_err(|_| "Lost the message on the way.".to_string()),
        reqwest::StatusCode::NO_CONTENT => Ok(None),
        status => Err(format!("The auth host refused ({status}).")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::auth_page::{Request, request_url};
    use super::*;

    #[test]
    fn the_two_sides_of_a_code_are_different_slots() {
        let page = "https://auth.ducktape.industries/";
        let request = slot(page, "KQ4M9XPT", "request").unwrap();
        let consent = slot(page, "KQ4M9XPT", "consent").unwrap();
        assert_ne!(request, consent);
        // the relay takes 43-character ids only
        let id = request.rsplit('/').next().unwrap();
        assert_eq!(id.len(), 43);
        assert!(request.starts_with("https://auth.ducktape.industries/r/"));
    }

    #[tokio::test]
    async fn a_relay_slot_is_minted_on_the_pages_origin() {
        let relay = Relay::at("https://auth.example/.duck/auth?x#y").unwrap();
        let id = relay.url.strip_prefix("https://auth.example/r/").unwrap();
        assert_eq!(B64.decode(id).unwrap().len(), 32);
        assert_ne!(
            Relay::at("https://a/").unwrap().url,
            Relay::at("https://a/").unwrap().url
        );
        assert!(Relay::at("not a url").is_err());
        // the QR carries the slot as the page's callback
        let url = request_url(
            "https://auth.example/",
            &Request::Get { challenge: [0; 32] },
            &relay.url,
        );
        assert!(url.ends_with(&format!("&cb=https%3A%2F%2Fauth.example%2Fr%2F{id}")));
    }
}
