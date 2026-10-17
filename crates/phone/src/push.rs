//! This phone registered with the push server (`crates/pushd`), so the
//! server carries the account's notices to it: for each push token the OS
//! handed over ([`crate::os::push_tokens`]) a [`Registration`] on the chain
//! in hand, for the account the seated key holds, signed by that key
//! (`backend::seated_sign`, the same namespaced signature a frame carries).
//! Sent again when any of that moves (another token, another account, the
//! network left and another reached), a token the OS withdrew sent as a
//! leave (an empty token), a failed send tried again after [`RETRY`]. What
//! becomes a notice is the program's; this only says where this phone is.
//!
//! The server is the phone's `push_server` pref, or the production server
//! (`ducktape_host::push::server`).

use ducktape_host::push::{self, Registration};
use ducktape_host::session::{Shared, on_runtime};
use gpui_kit::{App, AppContext as _, Context, Entity, Subscription, Task};
use std::collections::BTreeMap;
use std::time::Duration;

/// How long after a send the server refused or did not answer before the
/// next try.
const RETRY: Duration = Duration::from_secs(30);

/// One registration as this phone means it, before it is timed and signed:
/// a [`Registration`]'s fields but `at` and `sig`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    /// The chain as links spell it.
    pub chain: String,
    /// The account the seated key holds there.
    pub account: u64,
    /// The seated key, hex.
    pub key: String,
    /// `voip`, `apns` or `fcm`.
    pub kind: String,
    /// Empty: the device is leaving this kind.
    pub token: String,
    /// `production` or `sandbox` for an APNs kind, empty for `fcm`.
    pub env: String,
}

/// What the registrar knows the server holds for this phone, by chain and
/// kind, as far as its own sends went.
pub type Sent = BTreeMap<(String, String), Wanted>;

/// The registrations due now: each token not registered as it stands
/// (`chain`, `account`, `key`, `tokens`, `env` for APNs kinds), and a leave
/// for each kind sent on this chain and key that the OS withdrew since.
/// Nothing while the phone is off a network, locked or on no account: no
/// key signs then.
pub fn plan(
    chain: &str,
    account: Option<u64>,
    key: &str,
    tokens: &BTreeMap<String, String>,
    env: &str,
    sent: &Sent,
) -> Vec<Wanted> {
    let Some(account) = account else {
        return Vec::new();
    };
    if chain.is_empty() || key.is_empty() {
        return Vec::new();
    }
    let wanted = |kind: &str, token: &str| Wanted {
        chain: chain.to_owned(),
        account,
        key: key.to_owned(),
        kind: kind.to_owned(),
        token: token.to_owned(),
        env: match kind {
            "fcm" => String::new(),
            _ => env.to_owned(),
        },
    };
    let mut due: Vec<Wanted> = tokens
        .iter()
        .map(|(kind, token)| wanted(kind, token))
        .filter(|want| sent.get(&(want.chain.clone(), want.kind.clone())) != Some(want))
        .collect();
    due.extend(
        sent.values()
            .filter(|was| was.chain == chain && was.key == key && !was.token.is_empty())
            .filter(|was| !tokens.contains_key(&was.kind))
            .map(|was| wanted(&was.kind, "")),
    );
    due
}

/// The registrar: watches the session, the account and the tokens, and
/// sends what [`plan`] says, one at a time.
pub struct Registrar {
    shared: Shared,
    sent: Sent,
    /// The send out now, or the wait before the next try.
    busy: Option<Task<()>>,
    http: reqwest::Client,
    _observing: Vec<Subscription>,
}

impl Registrar {
    /// Started over `shared`, synced on every move of the session, the
    /// account or the tokens.
    pub fn start(shared: Shared, cx: &mut App) -> Entity<Self> {
        let registrar = cx.new(|cx| {
            let observing = vec![
                cx.observe(&shared.session, |this: &mut Self, _, cx| this.sync(cx)),
                cx.observe(&shared.account, |this: &mut Self, _, cx| this.sync(cx)),
            ];
            Self {
                shared,
                sent: Sent::new(),
                busy: None,
                http: reqwest::Client::new(),
                _observing: observing,
            }
        });
        let moved = crate::os::push_tokens_moved();
        let woken = registrar.downgrade();
        cx.spawn(async move |cx| {
            while moved.recv().await.is_ok() {
                let Some(registrar) = woken.upgrade() else {
                    return;
                };
                let _ = cx.update(|cx| registrar.update(cx, |this, cx| this.sync(cx)));
            }
        })
        .detach();
        registrar.update(cx, |this, cx| this.sync(cx));
        registrar
    }

    /// What is due, the first of it sent; the rest follows as each lands.
    fn sync(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let session = self.shared.session.read(cx).get();
        let account = self.shared.account.read(cx).get();
        let due = plan(
            &session.chain,
            account.account.as_ref().and_then(|found| found.as_ref().map(|(number, _)| *number)),
            &account.signer_key,
            &crate::os::push_tokens(),
            crate::os::push_environment(),
            &self.sent,
        );
        let Some(want) = due.into_iter().next() else {
            return;
        };
        let (http, server) = (self.http.clone(), push::server());
        let work = async move {
            let sent = send(&http, &server, &want).await;
            (want, sent)
        };
        self.busy = Some(cx.spawn(async move |this, cx| {
            let (want, sent) = on_runtime(work).await;
            let _ = this.update(cx, |this, cx| this.landed(want, sent, cx));
        }));
    }

    /// A send came back: taken, it is what the server holds now and the
    /// next due one goes; refused or unanswered, the next try waits.
    fn landed(&mut self, want: Wanted, sent: Result<(), String>, cx: &mut Context<Self>) {
        self.busy = None;
        match sent {
            Ok(()) => {
                let at = (want.chain.clone(), want.kind.clone());
                match want.token.is_empty() {
                    true => self.sent.remove(&at),
                    false => self.sent.insert(at, want),
                };
                self.sync(cx);
            }
            Err(error) => {
                tracing::warn!(target: "ducktape::phone", kind = want.kind, %error, "push registration not taken");
                self.busy = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(RETRY).await;
                    let _ = this.update(cx, |this, cx| {
                        this.busy = None;
                        this.sync(cx);
                    });
                }));
            }
        }
    }
}

/// `want` timed, signed by the seated key and posted to `server`; `Ok`
/// when the server took it.
async fn send(http: &reqwest::Client, server: &str, want: &Wanted) -> Result<(), String> {
    let mut registration = Registration {
        chain: want.chain.clone(),
        account: want.account,
        key: want.key.clone(),
        kind: want.kind.clone(),
        token: want.token.clone(),
        env: want.env.clone(),
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64),
        sig: String::new(),
    };
    let (public, signature) =
        ducktape_host::backend::seated_sign(push::NAMESPACE, &registration.preimage())
            .await
            .map_err(|error| error.to_string())?;
    if noded_client::hex::hex_encode(&public) != want.key {
        return Err("another key is seated now".into());
    }
    registration.sig = noded_client::hex::hex_encode(&signature);
    let body = serde_json::to_string(&registration).map_err(|error| error.to_string())?;
    let answer = http
        .post(format!("{server}{}", push::ROUTE))
        .header("content-type", "application/json")
        .timeout(Duration::from_secs(15))
        .body(body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = answer.status();
    if status.is_success() {
        return Ok(());
    }
    let reason = answer.text().await.unwrap_or_default();
    Err(format!("{server} answered {status}: {}", reason.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(kind, token)| ((*kind).to_owned(), (*token).to_owned()))
            .collect()
    }

    /// Every token is due on a chain with an account and a seated key, an
    /// APNs one with the build's env and an fcm one with none; nothing is
    /// due off a network, locked, or before the account is known.
    #[test]
    fn every_token_is_due_once_the_phone_is_on_an_account() {
        let have = tokens(&[("voip", "v1"), ("apns", "a1"), ("fcm", "f1")]);
        let due = plan("testkit-0a1b2c3d", Some(15), "ab", &have, "sandbox", &Sent::new());
        assert_eq!(due.len(), 3);
        let by_kind = |kind: &str| due.iter().find(|want| want.kind == kind).unwrap().clone();
        assert_eq!(
            by_kind("voip"),
            Wanted {
                chain: "testkit-0a1b2c3d".into(),
                account: 15,
                key: "ab".into(),
                kind: "voip".into(),
                token: "v1".into(),
                env: "sandbox".into(),
            }
        );
        assert_eq!(by_kind("fcm").env, "");
        assert!(plan("", Some(15), "ab", &have, "sandbox", &Sent::new()).is_empty());
        assert!(plan("testkit-0a1b2c3d", Some(15), "", &have, "sandbox", &Sent::new()).is_empty());
        assert!(plan("testkit-0a1b2c3d", None, "ab", &have, "sandbox", &Sent::new()).is_empty());
    }

    /// What the server holds as sent is not sent again; a token that moved
    /// is, and a kind the OS withdrew goes as a leave once, on the chain
    /// and key it was sent for.
    fn sent_of(due: &[Wanted]) -> Sent {
        due.iter()
            .map(|want| ((want.chain.clone(), want.kind.clone()), want.clone()))
            .collect()
    }

    #[test]
    fn only_what_moved_is_sent_again_and_a_withdrawn_token_leaves() {
        let chain = "testkit-0a1b2c3d";
        let have = tokens(&[("voip", "v1"), ("fcm", "f1")]);
        let sent = sent_of(&plan(chain, Some(15), "ab", &have, "production", &Sent::new()));
        assert!(plan(chain, Some(15), "ab", &have, "production", &sent).is_empty());
        let rotated = tokens(&[("voip", "v2"), ("fcm", "f1")]);
        let due = plan(chain, Some(15), "ab", &rotated, "production", &sent);
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].kind.as_str(), due[0].token.as_str()), ("voip", "v2"));
        let withdrawn = tokens(&[("fcm", "f1")]);
        let due = plan(chain, Some(15), "ab", &withdrawn, "production", &sent);
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].kind.as_str(), due[0].token.as_str()), ("voip", ""));
        // another chain's sends say nothing about this one
        let due = plan("other-11111111", Some(3), "ab", &withdrawn, "production", &sent);
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].kind.as_str(), due[0].token.as_str()), ("fcm", "f1"));
        // the account moved under the same key: sent again
        let due = plan(chain, Some(16), "ab", &have, "production", &sent);
        assert_eq!(due.len(), 2);
    }
}
