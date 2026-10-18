//! This phone registered with the push server (`crates/pushd`), so the
//! server carries its accounts' notices to it, from every network in its
//! list: for each push token the OS handed over ([`crate::os::push_tokens`])
//! a [`Registration`] on each listed network where this phone's key acts as
//! an account, signed by that network's key (the same namespaced signature a
//! frame carries). The network in hand is read off the session and the
//! account; each other listed node is reached lightly ([`reach`]: its
//! status, this phone's key there, the account identity says the key acts
//! as; no session, no views), once a run, and one that does not answer is
//! skipped until the inputs move.
//!
//! Sent again when any of that moves (another token, another account), a
//! token the OS withdrew sent as a leave (an empty token), and so is every
//! kind sent on a chain no listed node serves any more (the network
//! forgotten, or pushed off the list): what was sent is kept in the prefs,
//! so a forget after a relaunch still leaves. A switch is not a leave: a
//! network still listed keeps its rows. A send the server did not answer,
//! or answered 408, 429 or 5xx, is tried again after [`RETRY`]; one it
//! refused otherwise is not sent again until its inputs move (the tokens,
//! the account, the list). What becomes a notice is the program's; this
//! only says where this phone is.
//!
//! The server is the phone's `push_server` pref, or the production server
//! (`ducktape_host::push::server`).

use ducktape_host::push::{self, Registration};
use ducktape_host::session::{Shared, on_runtime};
use futures::future::Either;
use gpui_kit::{App, AppContext as _, Context, Entity, Subscription, Task};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// How long after a send the server did not take for now (408, 429, 5xx,
/// no answer) before the next try.
const RETRY: Duration = Duration::from_secs(30);

/// How long a listed node has to say who this phone is there.
const REACH_WITHIN: Duration = Duration::from_secs(15);

/// Where the prefs keep what was registered and which chain each listed
/// node serves.
const PREF: &str = "push_registrar";

/// One registration as this phone means it, before it is timed and signed:
/// a [`Registration`]'s fields but `at` and `sig`, and where its key is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    /// The chain as the session names it.
    pub chain: String,
    /// The account this phone's key acts as there.
    pub account: u64,
    /// This phone's key there, hex.
    pub key: String,
    /// `voip`, `apns` or `fcm`.
    pub kind: String,
    /// Empty: the device is leaving this kind.
    pub token: String,
    /// `production` or `sandbox` for an APNs kind, empty for `fcm`.
    pub env: String,
    /// The keyring that key is kept under (`noded_client::keyring::bind`):
    /// what signs, also a leave once its network left the list.
    pub keyring: String,
}

/// What the registrar knows the server holds for this phone, by chain and
/// kind, as far as its own sends went.
pub type Sent = BTreeMap<(String, String), Wanted>;

/// A network this phone is on, as the registrar knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Network {
    /// The chain as the session names it.
    pub chain: String,
    /// The account this phone's key acts as there; `None` while it acts as
    /// none, or that is not known yet.
    pub account: Option<u64>,
    /// This phone's key there, hex.
    pub key: String,
    /// The keyring it is kept under.
    pub keyring: String,
}

/// The listed nodes as networks, one per chain (a chain's first node
/// stands for it, the node in hand ahead of the rest): what each said
/// ([`reach`]), else only the chain it served when last heard, else
/// nothing yet.
pub fn networks(
    listed: &[String],
    reached: &BTreeMap<String, Network>,
    nodes: &BTreeMap<String, String>,
) -> Vec<Network> {
    let mut networks: Vec<Network> = Vec::new();
    for url in listed {
        let network = match (reached.get(url), nodes.get(url)) {
            (Some(network), _) => network.clone(),
            (None, Some(chain)) => Network {
                chain: chain.clone(),
                ..Network::default()
            },
            (None, None) => continue,
        };
        if !networks.iter().any(|known| known.chain == network.chain) {
            networks.push(network);
        }
    }
    networks
}

/// The registrations due now. On each of `networks` with an account and a
/// key: each token not registered as it stands, and a leave for each kind
/// sent on its chain and key that the OS withdrew since. Then a leave for
/// each kind sent on a chain none of `networks` is on, signed by the key it
/// was sent with. Nothing on a network whose account is not known: no key
/// acts there yet, or the node has not said.
pub fn plan(
    networks: &[Network],
    tokens: &BTreeMap<String, String>,
    env: &str,
    sent: &Sent,
) -> Vec<Wanted> {
    let mut due = Vec::new();
    for network in networks {
        let Some(account) = network.account else {
            continue;
        };
        if network.chain.is_empty() || network.key.is_empty() {
            continue;
        }
        let wanted = |kind: &str, token: &str| Wanted {
            chain: network.chain.clone(),
            account,
            key: network.key.clone(),
            kind: kind.to_owned(),
            token: token.to_owned(),
            env: match kind {
                "fcm" => String::new(),
                _ => env.to_owned(),
            },
            keyring: network.keyring.clone(),
        };
        due.extend(
            tokens
                .iter()
                .map(|(kind, token)| wanted(kind, token))
                .filter(|want| sent.get(&(want.chain.clone(), want.kind.clone())) != Some(want)),
        );
        due.extend(
            sent.values()
                .filter(|was| was.chain == network.chain && was.key == network.key)
                .filter(|was| !was.token.is_empty() && !tokens.contains_key(&was.kind))
                .map(|was| wanted(&was.kind, "")),
        );
    }
    due.extend(
        sent.values()
            .filter(|was| !was.token.is_empty())
            .filter(|was| networks.iter().all(|network| network.chain != was.chain))
            .map(|was| Wanted {
                token: String::new(),
                ..was.clone()
            }),
    );
    due
}

/// Why a send did not land.
#[derive(Debug, PartialEq, Eq)]
pub enum Failed {
    /// Refused for good: not sent again until one of its inputs moves.
    Stopped(String),
    /// Not taken for now: tried again after [`RETRY`].
    Again(String),
}

/// The server's answer `status` as the registrar takes it: a 2xx is taken;
/// a 4xx is final (pushd's 404 for a chain it does not follow, its 400 for
/// a bad signature, its 403 for a key that does not act as the account)
/// but 408 and 429, which are tried again, as is anything else.
pub fn answered(status: u16, reason: String) -> Result<(), Failed> {
    match status {
        200..=299 => Ok(()),
        408 | 429 => Err(Failed::Again(reason)),
        400..=499 => Err(Failed::Stopped(reason)),
        _ => Err(Failed::Again(reason)),
    }
}

/// The inputs a send refused for good, or a node not reached, waits on:
/// the listed nodes, the account in hand, the tokens.
type Inputs = (Vec<String>, Option<u64>, BTreeMap<String, String>);

/// The registrar: watches the session, the account and the tokens, and
/// sends what [`plan`] says, one at a time, reaching each listed node it
/// has not heard from between.
pub struct Registrar {
    shared: Shared,
    /// What the server holds for this phone; kept in the prefs.
    sent: Sent,
    /// The chain each listed node serves, as last heard; kept in the prefs.
    /// A tapped notice finds its network by it ([`Registrar::node_of`]).
    nodes: BTreeMap<String, String>,
    /// What each node said this run, by its URL; the node in hand's is read
    /// off the session and the account.
    reached: BTreeMap<String, Network>,
    /// The nodes not reached since the inputs last moved.
    missed: BTreeSet<String>,
    /// Sends refused for good since the inputs last moved: not sent again
    /// as they stand.
    stopped: Sent,
    inputs: Option<Inputs>,
    /// The send or the reach out now, or the wait before the next try.
    busy: Option<Task<()>>,
    http: reqwest::Client,
    _observing: Vec<Subscription>,
}

impl Registrar {
    /// Started over `shared` with what the prefs kept, synced on every move
    /// of the session, the account or the tokens.
    pub fn start(shared: Shared, cx: &mut App) -> Entity<Self> {
        // ponytail: what was sent is trusted across a relaunch, so a row
        // the server lost (its file gone, a token it dropped) is not sent
        // again until its token or account moves; each sent once a run if
        // that is felt
        let (sent, nodes) = load();
        let registrar = cx.new(|cx| {
            let observing = vec![
                cx.observe(&shared.session, |this: &mut Self, _, cx| this.sync(cx)),
                cx.observe(&shared.account, |this: &mut Self, _, cx| this.sync(cx)),
            ];
            Self {
                shared,
                sent,
                nodes,
                reached: BTreeMap::new(),
                missed: BTreeSet::new(),
                stopped: Sent::new(),
                inputs: None,
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
                cx.update(|cx| registrar.update(cx, |this, cx| this.sync(cx)));
            }
        })
        .detach();
        registrar.update(cx, |this, cx| this.sync(cx));
        registrar
    }

    /// The listed node on `chain`, the most recent first: where a tapped
    /// notice of another network's switches to.
    pub fn node_of(&self, chain: &str, cx: &App) -> Option<String> {
        let session = self.shared.session.read(cx).get();
        session
            .recent_endpoints
            .iter()
            .map(|node| &node.url)
            .find(|url| self.nodes.get(*url).is_some_and(|known| known == chain))
            .cloned()
    }

    /// What is due, the first of it sent; the rest follows as each lands.
    /// Nothing due: the first listed node not heard from is reached.
    fn sync(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let tokens = crate::os::push_tokens();
        let session = self.shared.session.read(cx).get().clone();
        let account = self.shared.account.read(cx);
        let here = account
            .get()
            .account
            .as_ref()
            .and_then(|found| found.as_ref().map(|(number, _)| *number));
        let mut listed: Vec<String> = session
            .recent_endpoints
            .iter()
            .map(|node| node.url.clone())
            .collect();
        if !session.chain.is_empty() {
            // the network in hand, listed or not, as the session and the
            // account have it
            if !listed.contains(&session.connected_rpc) {
                listed.insert(0, session.connected_rpc.clone());
            }
            let network = Network {
                chain: session.chain.clone(),
                account: here,
                key: account.get().signer_key.clone(),
                keyring: account.keyring().to_owned(),
            };
            self.reached.insert(session.connected_rpc.clone(), network);
        }
        let inputs = (listed.clone(), here, tokens.clone());
        if self.inputs.as_ref() != Some(&inputs) {
            self.missed.clear();
            self.stopped.clear();
            self.inputs = Some(inputs);
        }
        self.remember(&listed);
        let networks = networks(&listed, &self.reached, &self.nodes);
        let due = plan(
            &networks,
            &tokens,
            crate::os::push_environment(),
            &self.sent,
        )
        .into_iter()
        .find(|want| self.stopped.get(&(want.chain.clone(), want.kind.clone())) != Some(want));
        if let Some(want) = due {
            return self.send(want, cx);
        }
        let unheard = listed
            .into_iter()
            .find(|url| !self.reached.contains_key(url) && !self.missed.contains(url));
        if let Some(url) = unheard {
            self.reach(url, cx);
        }
    }

    /// The chain each node reached serves, kept for the listed nodes only;
    /// saved when that moved.
    fn remember(&mut self, listed: &[String]) {
        let mut nodes = self.nodes.clone();
        for (url, network) in &self.reached {
            nodes.insert(url.clone(), network.chain.clone());
        }
        nodes.retain(|url, _| listed.contains(url));
        if nodes != self.nodes {
            self.nodes = nodes;
            self.save();
        }
    }

    /// `want` sent; [`Registrar::landed`] takes the answer.
    fn send(&mut self, want: Wanted, cx: &mut Context<Self>) {
        let (http, server) = (self.http.clone(), push::server());
        let work = async move {
            let sent = send(&http, &server, &want).await;
            (want, sent)
        };
        self.busy = Some(cx.spawn(async move |this, cx| {
            let (want, sent) = on_runtime(work).await;
            this.update(cx, |this, cx| this.landed(want, sent, cx)).ok();
        }));
    }

    /// The node at `url` asked who this phone is there, given
    /// [`REACH_WITHIN`]; heard or not, the sync goes on.
    fn reach(&mut self, url: String, cx: &mut Context<Self>) {
        let work = reach(url.clone());
        self.busy = Some(cx.spawn(async move |this, cx| {
            let within = cx.background_executor().timer(REACH_WITHIN);
            let heard = match futures::future::select(std::pin::pin!(on_runtime(work)), within).await
            {
                Either::Left((heard, _)) => heard,
                Either::Right(_) => Err("the node did not answer in time".to_owned()),
            };
            this.update(cx, |this, cx| {
                this.busy = None;
                match heard {
                    Ok(network) => {
                        this.reached.insert(url, network);
                    }
                    Err(error) => {
                        tracing::debug!(target: "ducktape::phone", url, %error, "a listed node not reached");
                        this.missed.insert(url);
                    }
                }
                this.sync(cx);
            })
            .ok();
        }));
    }

    /// A send came back: taken, it is what the server holds now (kept in
    /// the prefs) and the next due one goes; refused for good, it waits on
    /// its inputs and the next goes; not taken for now, the next try waits.
    fn landed(&mut self, want: Wanted, sent: Result<(), Failed>, cx: &mut Context<Self>) {
        self.busy = None;
        let at = (want.chain.clone(), want.kind.clone());
        match sent {
            Ok(()) => {
                match want.token.is_empty() {
                    true => self.sent.remove(&at),
                    false => self.sent.insert(at, want),
                };
                self.save();
                self.sync(cx);
            }
            Err(Failed::Stopped(error)) => {
                tracing::warn!(target: "ducktape::phone", kind = want.kind, chain = want.chain, %error,
                    "push registration refused: not sent again until it moves");
                self.stopped.insert(at, want);
                self.sync(cx);
            }
            Err(Failed::Again(error)) => {
                tracing::warn!(target: "ducktape::phone", kind = want.kind, chain = want.chain, %error,
                    "push registration not taken");
                self.busy = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(RETRY).await;
                    this.update(cx, |this, cx| {
                        this.busy = None;
                        this.sync(cx);
                    })
                    .ok();
                }));
            }
        }
    }

    /// What was sent and the nodes' chains, into the prefs.
    fn save(&self) {
        let sent: Vec<serde_json::Value> = self.sent.values().map(json_of).collect();
        let kept = serde_json::json!({ "sent": sent, "nodes": self.nodes });
        if !ducktape_host::backend::edit_prefs(|prefs| prefs[PREF] = kept) {
            tracing::warn!(target: "ducktape::phone", "push registrations not kept");
        }
    }
}

/// What the prefs kept: what was sent, and the chain each listed node
/// served. An entry that does not read is dropped.
fn load() -> (Sent, BTreeMap<String, String>) {
    let prefs = ducktape_host::backend::read_prefs().unwrap_or_default();
    let kept = &prefs[PREF];
    let sent = kept["sent"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(wanted_of)
        .map(|want| ((want.chain.clone(), want.kind.clone()), want))
        .collect();
    let nodes = kept["nodes"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(url, chain)| Some((url.clone(), chain.as_str()?.to_owned())))
        .collect();
    (sent, nodes)
}

fn json_of(want: &Wanted) -> serde_json::Value {
    serde_json::json!({
        "chain": want.chain,
        "account": want.account,
        "key": want.key,
        "kind": want.kind,
        "token": want.token,
        "env": want.env,
        "keyring": want.keyring,
    })
}

fn wanted_of(value: &serde_json::Value) -> Option<Wanted> {
    let text = |field: &str| value.get(field)?.as_str().map(str::to_owned);
    Some(Wanted {
        chain: text("chain")?,
        account: value.get("account")?.as_u64()?,
        key: text("key")?,
        kind: text("kind")?,
        token: text("token")?,
        env: text("env")?,
        keyring: text("keyring")?,
    })
}

/// Who this phone is on the network of the node at `url`: the chain it
/// serves (as the session derives it, `session/node.rs`), this phone's key
/// there, and the account identity says the key acts as, asked with the
/// shell's reader key. Light: a status and an identity query, no session,
/// no views; the key is opened, never seated.
async fn reach(url: String) -> Result<Network, String> {
    let os = ducktape_host::contract::os();
    let client = ducktape_host::backend::rpc_client(url);
    let status = client.status().await.map_err(|error| error.to_string())?;
    let chain = ducklink::ChainId::of(&status.network, &status.genesis)
        .map_or_else(|| status.network.clone(), |chain| chain.to_string());
    let keyring =
        noded_client::keyring::bind(&os.dirs().remotes, &status.network, status.time)?.dir;
    let key = os
        .device_key(&keyring)
        .map_err(|refused| refused.sentence())?
        .ok_or("this phone keeps no key for that network")?;
    let public = key.public_key();
    let account = noded_client::identity::account_of_key(
        &client,
        &status.network,
        public.to_vec(),
        &*os.reader(),
    )
    .await?;
    Ok(Network {
        chain,
        account: account.map(|(number, _)| number),
        key: noded_client::hex::hex_encode(&public),
        keyring,
    })
}

/// `want` timed, signed by its network's key (opened from its keyring:
/// the seated one's for the network in hand, another network's own, never
/// seated) and posted to `server`.
async fn send(http: &reqwest::Client, server: &str, want: &Wanted) -> Result<(), Failed> {
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
    // a key that will not open or sign does not on the next try either
    let key = ducktape_host::contract::os()
        .device_key(&want.keyring)
        .map_err(|refused| Failed::Stopped(refused.sentence()))?
        .ok_or_else(|| Failed::Stopped("this phone keeps no key for that network".into()))?;
    if noded_client::hex::hex_encode(&key.public_key()) != want.key {
        return Err(Failed::Stopped(
            "another key is kept for that network now".into(),
        ));
    }
    let signature = key
        .sign(push::NAMESPACE, &registration.preimage())
        .map_err(|refused| Failed::Stopped(refused.0))?;
    registration.sig = noded_client::hex::hex_encode(&signature);
    let body =
        serde_json::to_string(&registration).map_err(|error| Failed::Stopped(error.to_string()))?;
    let answer = http
        .post(format!("{server}{}", push::ROUTE))
        .header("content-type", "application/json")
        .timeout(Duration::from_secs(15))
        .body(body)
        .send()
        .await
        .map_err(|error| Failed::Again(error.to_string()))?;
    let status = answer.status();
    let reason = answer.text().await.unwrap_or_default();
    answered(
        status.as_u16(),
        format!("{server} answered {status}: {}", reason.trim()),
    )
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

    /// A network on `chain` where key `key` acts as `account`.
    fn on(chain: &str, account: Option<u64>, key: &str) -> Network {
        Network {
            chain: chain.into(),
            account,
            key: key.into(),
            keyring: format!("{key}-ring"),
        }
    }

    fn sent_of(due: &[Wanted]) -> Sent {
        due.iter()
            .map(|want| ((want.chain.clone(), want.kind.clone()), want.clone()))
            .collect()
    }

    /// Every token is due on a chain with an account and a key, an APNs
    /// one with the build's env and an fcm one with none; nothing is due
    /// off a network, with no key, or before the account is known.
    #[test]
    fn every_token_is_due_once_the_phone_is_on_an_account() {
        let have = tokens(&[("voip", "v1"), ("apns", "a1"), ("fcm", "f1")]);
        let here = [on("testkit#0a1b2c3d", Some(15), "ab")];
        let due = plan(&here, &have, "sandbox", &Sent::new());
        assert_eq!(due.len(), 3);
        let by_kind = |kind: &str| due.iter().find(|want| want.kind == kind).unwrap().clone();
        assert_eq!(
            by_kind("voip"),
            Wanted {
                chain: "testkit#0a1b2c3d".into(),
                account: 15,
                key: "ab".into(),
                kind: "voip".into(),
                token: "v1".into(),
                env: "sandbox".into(),
                keyring: "ab-ring".into(),
            }
        );
        assert_eq!(by_kind("fcm").env, "");
        for idle in [
            on("", Some(15), "ab"),
            on("testkit#0a1b2c3d", Some(15), ""),
            on("testkit#0a1b2c3d", None, "ab"),
        ] {
            assert!(plan(&[idle], &have, "sandbox", &Sent::new()).is_empty());
        }
    }

    /// What the server holds as sent is not sent again; a token that moved
    /// is, and a kind the OS withdrew goes as a leave once, on the chain
    /// and key it was sent for.
    #[test]
    fn only_what_moved_is_sent_again_and_a_withdrawn_token_leaves() {
        let here = [on("testkit#0a1b2c3d", Some(15), "ab")];
        let have = tokens(&[("voip", "v1"), ("fcm", "f1")]);
        let sent = sent_of(&plan(&here, &have, "production", &Sent::new()));
        assert!(plan(&here, &have, "production", &sent).is_empty());
        let rotated = tokens(&[("voip", "v2"), ("fcm", "f1")]);
        let due = plan(&here, &rotated, "production", &sent);
        assert_eq!(due.len(), 1);
        assert_eq!(
            (due[0].kind.as_str(), due[0].token.as_str()),
            ("voip", "v2")
        );
        let withdrawn = tokens(&[("fcm", "f1")]);
        let due = plan(&here, &withdrawn, "production", &sent);
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].kind.as_str(), due[0].token.as_str()), ("voip", ""));
        // the account moved under the same key: sent again
        let moved = on("testkit#0a1b2c3d", Some(16), "ab");
        assert_eq!(plan(&[moved], &have, "production", &sent).len(), 2);
    }

    /// Two listed networks, each with an account: every token is due on
    /// each, under that network's own key, account and keyring; the node in
    /// hand's first. A listed node whose account is not known (not
    /// reached, or its key acts as none there) is due nothing, and two
    /// nodes of one chain are one network, the first standing for it.
    #[test]
    fn every_listed_network_with_an_account_is_due() {
        let have = tokens(&[("apns", "a1"), ("voip", "v1")]);
        let reached = BTreeMap::from([
            ("http://a".to_owned(), on("home#0a0a0a0a", Some(15), "aa")),
            ("http://b".to_owned(), on("away#0b0b0b0b", Some(3), "bb")),
            ("http://b2".to_owned(), on("away#0b0b0b0b", Some(4), "b2")),
        ]);
        let nodes = BTreeMap::from([("http://c".to_owned(), "far#0c0c0c0c".to_owned())]);
        let listed =
            ["http://a", "http://b", "http://b2", "http://c", "http://d"].map(String::from);
        let networks = networks(&listed, &reached, &nodes);
        assert_eq!(
            networks
                .iter()
                .map(|network| network.chain.as_str())
                .collect::<Vec<_>>(),
            ["home#0a0a0a0a", "away#0b0b0b0b", "far#0c0c0c0c"]
        );
        let due = plan(&networks, &have, "production", &Sent::new());
        let seen: Vec<_> = due
            .iter()
            .map(|want| {
                (
                    want.chain.as_str(),
                    want.account,
                    want.key.as_str(),
                    want.kind.as_str(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("home#0a0a0a0a", 15, "aa", "apns"),
                ("home#0a0a0a0a", 15, "aa", "voip"),
                ("away#0b0b0b0b", 3, "bb", "apns"),
                ("away#0b0b0b0b", 3, "bb", "voip"),
            ]
        );
        assert!(
            due.iter()
                .all(|want| want.keyring == format!("{}-ring", want.key))
        );
    }

    /// A network forgotten, after a relaunch even (what was sent read back
    /// from the prefs' shape), leaves: each kind sent on its chain, signed
    /// by the key and keyring it was sent with. A switch is no leave: a
    /// chain still listed, reached or not, keeps its rows, and so does one
    /// another listed node still serves.
    #[test]
    fn a_forgotten_network_leaves_and_a_listed_one_stays() {
        let have = tokens(&[("apns", "a1")]);
        let home = on("home#0a0a0a0a", Some(15), "aa");
        let away = on("away#0b0b0b0b", Some(3), "bb");
        let sent = sent_of(&plan(
            &[home.clone(), away.clone()],
            &have,
            "production",
            &Sent::new(),
        ));
        assert_eq!(sent.len(), 2);
        let kept: Vec<_> = sent.values().map(json_of).collect();
        let relaunched: Sent = sent_of(&kept.iter().filter_map(wanted_of).collect::<Vec<_>>());
        assert_eq!(relaunched, sent, "what was sent did not survive a relaunch");

        // away's node forgotten: home in hand, away in no list
        let nodes = BTreeMap::from([("http://a".to_owned(), "home#0a0a0a0a".to_owned())]);
        let reached = BTreeMap::from([("http://a".to_owned(), home.clone())]);
        let after = networks(&["http://a".to_owned()], &reached, &nodes);
        let due = plan(&after, &have, "production", &relaunched);
        assert_eq!(
            due,
            [Wanted {
                token: String::new(),
                ..relaunched[&("away#0b0b0b0b".to_owned(), "apns".to_owned())].clone()
            }]
        );
        assert_eq!(
            (due[0].key.as_str(), due[0].keyring.as_str(), due[0].account),
            ("bb", "bb-ring", 3)
        );

        // switched to home, away still listed but not reached this run
        let nodes = BTreeMap::from([
            ("http://a".to_owned(), "home#0a0a0a0a".to_owned()),
            ("http://b".to_owned(), "away#0b0b0b0b".to_owned()),
        ]);
        let listed = ["http://a", "http://b"].map(String::from);
        let still = networks(&listed, &reached, &nodes);
        assert!(plan(&still, &have, "production", &relaunched).is_empty());
        // one of away's two nodes forgotten: the other still serves it
        let nodes = BTreeMap::from([
            ("http://a".to_owned(), "home#0a0a0a0a".to_owned()),
            ("http://b2".to_owned(), "away#0b0b0b0b".to_owned()),
        ]);
        let listed = ["http://a", "http://b2"].map(String::from);
        let still = networks(&listed, &reached, &nodes);
        assert!(plan(&still, &have, "production", &relaunched).is_empty());
    }

    /// A 4xx is final but 408 and 429; 5xx and the rest are tried again.
    #[test]
    fn a_4xx_stops_and_a_5xx_is_tried_again() {
        let said = |status| answered(status, format!("{status}"));
        assert_eq!(said(204), Ok(()));
        for status in [400, 403, 404, 413] {
            assert_eq!(said(status), Err(Failed::Stopped(format!("{status}"))));
        }
        for status in [408, 429, 500, 502, 503] {
            assert_eq!(said(status), Err(Failed::Again(format!("{status}"))));
        }
    }

    /// A send refused for good is not sent again while it stands as it
    /// was, and the next due one goes; one moved (another token) goes, and
    /// so does the refused one once an input moves. A send not taken for
    /// now waits [`RETRY`] before anything goes.
    #[gpui_kit::test]
    fn a_refused_send_waits_on_its_inputs_and_an_unanswered_one_on_the_clock(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use ducktape_host::session::SessionState;
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let shared = cx.update(|cx| Shared::for_test(Default::default(), Default::default(), cx));
        let registrar = cx.update(|cx| Registrar::start(shared.clone(), cx));
        let refused = Wanted {
            chain: "away#0b0b0b0b".into(),
            account: 3,
            key: "bb".into(),
            kind: "apns".into(),
            token: "a1".into(),
            env: "production".into(),
            keyring: "bb-ring".into(),
        };
        registrar.update(cx, |this, cx| {
            this.landed(refused.clone(), Err(Failed::Stopped("404".into())), cx);
            assert!(this.busy.is_none(), "a refusal for good waits on the clock");
            assert_eq!(this.stopped.values().collect::<Vec<_>>(), [&refused]);
        });
        // an input moved: the list
        shared.session.update(cx, |session, cx| {
            let moved = SessionState {
                recent_endpoints: vec![ducktape_host::backend::RecentEndpoint {
                    url: "http://127.0.0.1:1".into(),
                    network: "testkit".into(),
                    ..Default::default()
                }],
                ..SessionState::default()
            };
            session.seed(moved, cx)
        });
        registrar.update(cx, |this, cx| {
            assert!(
                this.stopped.is_empty(),
                "the list moved and the refusal stood"
            );
            this.busy = None;
            this.landed(refused.clone(), Err(Failed::Again("503".into())), cx);
            assert!(this.busy.is_some(), "an unanswered send did not wait");
            assert!(this.stopped.is_empty());
        });
    }
}
