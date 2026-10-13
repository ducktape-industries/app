//! The auth host's relay slots, `/r/<id>`: a one-shot mailbox — POST
//! stores a `result`, GET hands it out once, 204 until then. Two devices
//! joining pass a request and a consent through two slots named from the
//! code ([`slot`], [`post`], [`take`]).

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use sha2::{Digest as _, Sha256};

use super::loopback::url_encode;

/// How often a slot is asked for its answer.
pub(super) const POLL: Duration = Duration::from_millis(1500);

/// The slot `id` on `page`'s origin.
fn slot_url(page: &str, id: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(page)
        .map_err(|error| format!("The auth host address is unusable: {error}"))?;
    url.set_path(&format!("/r/{id}"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
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

/// `json` into the slot at `url`, over the connection's `http`.
pub(super) async fn post(http: &reqwest::Client, url: &str, json: String) -> Result<(), String> {
    let reply = http
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

/// One GET of a slot, over the connection's `http`: its message, or `None`
/// while nothing has arrived.
pub(super) async fn take(http: &reqwest::Client, url: &str) -> Result<Option<String>, String> {
    let reply = http
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
}
