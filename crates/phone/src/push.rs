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
//! Each is sent once a run, so a row the server lost comes back at the
//! next launch, and again when any of it moves (another token, another
//! account). A token the OS withdrew is sent as a leave (an empty token),
//! and so is every kind sent on a chain no listed node serves any more (the
//! network forgotten, or pushed off the list): what the server holds is
//! kept in the prefs for that, so a forget after a relaunch still leaves. A
//! switch is not a leave: a network still listed keeps its rows. A send the
//! server did not answer, or answered 408, 429 or 5xx, is tried again after
//! [`RETRY`], the rest going on meanwhile; one it refused otherwise is not
//! sent again until its inputs move (the tokens, the account, the list), and
//! a leave refused so has nothing left to leave. What becomes a notice is
//! the program's; this only says where this phone is.
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

/// Registrations by chain and kind: what the server holds for this phone as
/// far as its own sends went, or what was sent this run.
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

/// The listed nodes as networks, one per chain (the node in hand ahead of
/// the rest): what each said ([`reach`]), else only the chain it served when
/// last heard, else nothing yet. A chain's first node with a known account
/// stands for it: one only remembered hides no other that answered.
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
        match networks
            .iter_mut()
            .find(|known| known.chain == network.chain)
        {
            Some(known) if known.account.is_none() && network.account.is_some() => *known = network,
            Some(_) => {}
            None => networks.push(network),
        }
    }
    networks
}

/// The registrations due now. `tokens` holds each kind the OS handed a
/// token for this run, empty for one it took back since; a kind not handed
/// yet says nothing (a launch's tokens come after it). On each of
/// `networks` with an account and a key: each token not `sent` this run as
/// it stands, and a leave for each kind taken back that the server holds
/// (`kept`) on its chain and key. Then a leave for each kind kept on a chain
/// none of `networks` is on, signed by the key it was sent with. Nothing on
/// a network whose account is not known: no key acts there yet, or the node
/// has not said.
pub fn plan(
    networks: &[Network],
    tokens: &BTreeMap<String, String>,
    env: &str,
    kept: &Sent,
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
                .filter(|want| {
                    let at = (want.chain.clone(), want.kind.clone());
                    match want.token.is_empty() {
                        true => kept
                            .get(&at)
                            .is_some_and(|was| was.key == want.key && !was.token.is_empty()),
                        false => sent.get(&at) != Some(want),
                    }
                }),
        );
    }
    due.extend(
        kept.values()
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
    /// What the server holds for this phone, as far as its sends went; kept
    /// in the prefs, for the leaves.
    kept: Sent,
    /// What was sent and taken this run: each registration goes once a run.
    sent: Sent,
    /// The chain each listed node serves, as last heard; kept in the prefs.
    /// A tapped notice finds its network by it ([`Registrar::node_of`]).
    nodes: BTreeMap<String, String>,
    /// What each node said this run, by its URL; the node in hand's is read
    /// off the session and the account.
    reached: BTreeMap<String, Network>,
    /// The nodes not reached since the inputs last moved.
    missed: BTreeSet<String>,
    /// The kinds the OS handed a token for this run: one taken back since
    /// is a leave, one not handed yet is no word at all.
    heard: BTreeSet<String>,
    /// Sends refused for good since the inputs last moved: not sent again
    /// as they stand.
    stopped: Sent,
    /// Sends not taken for now, each with its wait: not sent again as they
    /// stand until [`RETRY`] is up, the rest going past them.
    again: BTreeMap<(String, String), (Wanted, Task<()>)>,
    inputs: Option<Inputs>,
    /// The send or the reach out now.
    busy: Option<Task<()>>,
    http: reqwest::Client,
    _observing: Vec<Subscription>,
}

impl Registrar {
    /// Started over `shared` with what the prefs kept, synced on every move
    /// of the session, the account or the tokens.
    pub fn start(shared: Shared, cx: &mut App) -> Entity<Self> {
        let (kept, nodes) = load();
        let registrar = cx.new(|cx| {
            let observing = vec![
                cx.observe(&shared.session, |this: &mut Self, _, cx| this.sync(cx)),
                cx.observe(&shared.account, |this: &mut Self, _, cx| this.sync(cx)),
            ];
            Self {
                shared,
                kept,
                sent: Sent::new(),
                nodes,
                reached: BTreeMap::new(),
                missed: BTreeSet::new(),
                heard: BTreeSet::new(),
                stopped: Sent::new(),
                again: BTreeMap::new(),
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
        let handed = crate::os::push_tokens();
        self.heard.extend(handed.keys().cloned());
        let tokens: BTreeMap<String, String> = self
            .heard
            .iter()
            .map(|kind| (kind.clone(), handed.get(kind).cloned().unwrap_or_default()))
            .collect();
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
            &self.kept,
            &self.sent,
        );
        if let Some(want) = self.next(due) {
            return self.send(want, cx);
        }
        let unheard = listed
            .into_iter()
            .find(|url| !self.reached.contains_key(url) && !self.missed.contains(url));
        if let Some(url) = unheard {
            self.reach(url, cx);
        }
    }

    /// The first of `due` neither refused for good nor waiting to be tried
    /// again, as it stands.
    fn next(&self, due: Vec<Wanted>) -> Option<Wanted> {
        due.into_iter().find(|want| {
            let at = (want.chain.clone(), want.kind.clone());
            let waits = self.again.get(&at).map(|(waits, _)| waits);
            self.stopped.get(&at) != Some(want) && waits != Some(want)
        })
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

    /// A send came back. Taken, it is what the server holds now (kept in
    /// the prefs) and the next due one goes. Refused for good, it waits on
    /// its inputs; a leave refused so has nothing left to leave (pushd holds
    /// no live row: the chain unfollowed, the key off the account), and goes
    /// from the prefs. Not taken for now, it waits [`RETRY`]. Either way the
    /// next due one goes.
    fn landed(&mut self, want: Wanted, sent: Result<(), Failed>, cx: &mut Context<Self>) {
        self.busy = None;
        let at = (want.chain.clone(), want.kind.clone());
        match sent {
            Ok(()) => {
                self.again.remove(&at);
                match want.token.is_empty() {
                    true => {
                        self.kept.remove(&at);
                        self.sent.remove(&at);
                    }
                    false => {
                        self.kept.insert(at.clone(), want.clone());
                        self.sent.insert(at, want);
                    }
                }
                self.save();
            }
            Err(Failed::Stopped(error)) => {
                tracing::warn!(target: "ducktape::phone", kind = want.kind, chain = want.chain, %error,
                    "push registration refused: not sent again until it moves");
                if want.token.is_empty() {
                    self.kept.remove(&at);
                    self.sent.remove(&at);
                    self.save();
                } else {
                    self.stopped.insert(at, want);
                }
            }
            Err(Failed::Again(error)) => {
                tracing::warn!(target: "ducktape::phone", kind = want.kind, chain = want.chain, %error,
                    "push registration not taken");
                let waited = at.clone();
                let wait = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(RETRY).await;
                    this.update(cx, |this, cx| {
                        this.again.remove(&waited);
                        this.sync(cx);
                    })
                    .ok();
                });
                self.again.insert(at, (want, wait));
            }
        }
        self.sync(cx);
    }

    /// What was sent and the nodes' chains, into the prefs.
    fn save(&self) {
        let sent: Vec<serde_json::Value> = self.kept.values().map(json_of).collect();
        let kept = serde_json::json!({ "sent": sent, "nodes": self.nodes });
        if !ducktape_host::backend::edit_prefs(|prefs| prefs[PREF] = kept) {
            tracing::warn!(target: "ducktape::phone", "push registrations not kept");
        }
    }
}

/// What the prefs kept: what the server holds, and the chain each listed
/// node served. An entry that does not read is dropped.
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
    let opened = keyring.clone();
    let public = off_window(move || match os.device_key(&opened) {
        Ok(Some(key)) => Ok(key.public_key()),
        Ok(None) => Err(NO_KEY.to_owned()),
        Err(refused) => Err(refused.sentence()),
    })
    .await??;
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

/// What a phone with no key for a network's keyring says.
const NO_KEY: &str = "this phone keeps no key for that network";

/// `work` on the runtime's blocking pool, off the window thread: a key
/// store may take its time or prompt (`OsServices::device_key`), as the
/// host's own open does (`session/account.rs`).
async fn off_window<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    ducktape_host::runtime::handle()
        .spawn_blocking(work)
        .await
        .map_err(|_| "the key store's work did not finish".to_owned())
}

/// The signature over `preimage` of the key `opened` for a registration of
/// `key`, hex. A key that would not open or sign is tried again (an iPhone
/// keeps it locked away while it is locked); only another key kept for the
/// network is final.
fn signed(
    opened: Result<
        Option<std::sync::Arc<dyn noded_client::Signer>>,
        ducktape_host::contract::KeyRefusal,
    >,
    key: &str,
    preimage: &[u8],
) -> Result<String, Failed> {
    let signer = opened
        .map_err(|refused| Failed::Again(refused.sentence()))?
        .ok_or_else(|| Failed::Again(NO_KEY.into()))?;
    if noded_client::hex::hex_encode(&signer.public_key()) != key {
        return Err(Failed::Stopped(
            "another key is kept for that network now".into(),
        ));
    }
    let signature = signer
        .sign(push::NAMESPACE, preimage)
        .map_err(|refused| Failed::Again(refused.0))?;
    Ok(noded_client::hex::hex_encode(&signature))
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
    let (keyring, key, preimage) = (
        want.keyring.clone(),
        want.key.clone(),
        registration.preimage(),
    );
    let signature = off_window(move || {
        signed(
            ducktape_host::contract::os().device_key(&keyring),
            &key,
            &preimage,
        )
    })
    .await
    .map_err(Failed::Again)??;
    registration.sig = signature;
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
        let due = plan(&here, &have, "sandbox", &Sent::new(), &Sent::new());
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
            assert!(plan(&[idle], &have, "sandbox", &Sent::new(), &Sent::new()).is_empty());
        }
    }

    /// What the server holds as sent is not sent again; a token that moved
    /// is, and a kind the OS withdrew goes as a leave once, on the chain
    /// and key it was sent for; a kind not handed yet is not a withdrawal.
    #[test]
    fn only_what_moved_is_sent_again_and_a_withdrawn_token_leaves() {
        let here = [on("testkit#0a1b2c3d", Some(15), "ab")];
        let have = tokens(&[("voip", "v1"), ("fcm", "f1")]);
        let sent = sent_of(&plan(
            &here,
            &have,
            "production",
            &Sent::new(),
            &Sent::new(),
        ));
        assert!(plan(&here, &have, "production", &sent, &sent).is_empty());
        let rotated = tokens(&[("voip", "v2"), ("fcm", "f1")]);
        let due = plan(&here, &rotated, "production", &sent, &sent);
        assert_eq!(due.len(), 1);
        assert_eq!(
            (due[0].kind.as_str(), due[0].token.as_str()),
            ("voip", "v2")
        );
        let withdrawn = tokens(&[("voip", ""), ("fcm", "f1")]);
        let due = plan(&here, &withdrawn, "production", &sent, &sent);
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].kind.as_str(), due[0].token.as_str()), ("voip", ""));
        // a relaunch before the OS handed a kind's token: no word, no leave
        let unhanded = tokens(&[("fcm", "f1")]);
        assert!(plan(&here, &unhanded, "production", &sent, &sent).is_empty());
        // the account moved under the same key: sent again
        let moved = on("testkit#0a1b2c3d", Some(16), "ab");
        assert_eq!(plan(&[moved], &have, "production", &sent, &sent).len(), 2);
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
        let due = plan(&networks, &have, "production", &Sent::new(), &Sent::new());
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
        let due = plan(&after, &have, "production", &relaunched, &relaunched);
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
        assert!(plan(&still, &have, "production", &relaunched, &relaunched).is_empty());
        // one of away's two nodes forgotten: the other still serves it
        let nodes = BTreeMap::from([
            ("http://a".to_owned(), "home#0a0a0a0a".to_owned()),
            ("http://b2".to_owned(), "away#0b0b0b0b".to_owned()),
        ]);
        let listed = ["http://a", "http://b2"].map(String::from);
        let still = networks(&listed, &reached, &nodes);
        assert!(plan(&still, &have, "production", &relaunched, &relaunched).is_empty());
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

    /// A remembered node of a chain that did not answer this run hides no
    /// node of that chain that did: the one with a known account stands for
    /// the chain, and its tokens are due there.
    #[test]
    fn a_remembered_node_hides_no_reached_one_of_its_chain() {
        let reached =
            BTreeMap::from([("http://b2".to_owned(), on("away#0b0b0b0b", Some(4), "b2"))]);
        let nodes = BTreeMap::from([("http://b1".to_owned(), "away#0b0b0b0b".to_owned())]);
        let listed = ["http://b1", "http://b2"].map(String::from);
        let networks = networks(&listed, &reached, &nodes);
        assert_eq!(networks, [on("away#0b0b0b0b", Some(4), "b2")]);
        let have = tokens(&[("apns", "a1")]);
        let due = plan(&networks, &have, "production", &Sent::new(), &Sent::new());
        assert_eq!(
            due.len(),
            1,
            "nothing due on a chain one of whose nodes answered"
        );
    }

    /// Each registration goes once a run: after a relaunch, what the server
    /// held is sent again (a row it lost comes back), and once taken it is
    /// not sent again in the run.
    #[test]
    fn a_relaunch_sends_each_registration_once_again() {
        let have = tokens(&[("apns", "a1"), ("voip", "v1")]);
        let here = [on("home#0a0a0a0a", Some(15), "aa")];
        let kept = sent_of(&plan(
            &here,
            &have,
            "production",
            &Sent::new(),
            &Sent::new(),
        ));
        let due = plan(&here, &have, "production", &kept, &Sent::new());
        assert_eq!(
            due.len(),
            2,
            "a relaunch did not send what the server held again"
        );
        assert!(plan(&here, &have, "production", &kept, &sent_of(&due)).is_empty());
    }

    /// The key store's work runs off the calling (window) thread.
    #[test]
    fn a_key_is_opened_off_the_window_thread() {
        let caller = std::thread::current().id();
        let ran = ducktape_host::runtime::handle()
            .block_on(off_window(|| std::thread::current().id()))
            .unwrap();
        assert_ne!(
            ran, caller,
            "the key store's work ran on the calling thread"
        );
    }

    /// A key that would not open, is not there, or would not sign is tried
    /// again (a locked iPhone); another key kept for the network is final;
    /// the key kept signs.
    #[test]
    fn a_key_that_will_not_open_or_sign_is_tried_again() {
        use ducktape_host::contract::KeyRefusal;
        struct Locked;
        impl noded_client::Signer for Locked {
            fn public_key(&self) -> [u8; 32] {
                [1; 32]
            }
            fn sign(&self, _: &[u8], _: &[u8]) -> Result<[u8; 64], noded_client::SignError> {
                Err(noded_client::SignError("the phone is locked".into()))
            }
        }
        let again = |said: &str| Err(Failed::Again(said.into()));
        let locked = KeyRefusal::Denied("the phone is locked".into());
        assert_eq!(
            signed(Err(locked), "ab", b"x"),
            again("the phone is locked")
        );
        assert_eq!(signed(Ok(None), "ab", b"x"), again(NO_KEY));
        let one = hex_encode(&[1; 32]);
        let refused: std::sync::Arc<dyn noded_client::Signer> = std::sync::Arc::new(Locked);
        assert_eq!(
            signed(Ok(Some(refused.clone())), &one, b"x"),
            again("the phone is locked")
        );
        assert_eq!(
            signed(Ok(Some(refused)), "ab", b"x"),
            Err(Failed::Stopped(
                "another key is kept for that network now".into()
            ))
        );
        let key = ducktape_host::contract::os()
            .device_key("testkit")
            .unwrap()
            .unwrap();
        let public = hex_encode(&key.public_key());
        assert!(signed(Ok(Some(key)), &public, b"x").is_ok());
    }

    fn hex_encode(bytes: &[u8]) -> String {
        noded_client::hex::hex_encode(bytes)
    }

    /// A registrar over a session of the test's own.
    fn registrar(cx: &mut gpui_kit::TestAppContext) -> (Shared, Entity<Registrar>) {
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let shared = cx.update(|cx| Shared::for_test(Default::default(), Default::default(), cx));
        let registrar = cx.update(|cx| Registrar::start(shared.clone(), cx));
        (shared, registrar)
    }

    fn want(chain: &str, token: &str) -> Wanted {
        Wanted {
            chain: chain.into(),
            account: 3,
            key: "bb".into(),
            kind: "apns".into(),
            token: token.into(),
            env: "production".into(),
            keyring: "bb-ring".into(),
        }
    }

    /// A send refused for good is not sent again while it stands as it
    /// was, until an input moves (here, the list).
    #[gpui_kit::test]
    fn a_refused_send_waits_on_its_inputs(cx: &mut gpui_kit::TestAppContext) {
        use ducktape_host::session::SessionState;
        let (shared, registrar) = registrar(cx);
        let refused = want("away#0b0b0b0b", "a1");
        registrar.update(cx, |this, cx| {
            this.landed(refused.clone(), Err(Failed::Stopped("404".into())), cx);
            assert_eq!(this.next(vec![refused.clone()]), None);
        });
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
        registrar.update(cx, |this, _| {
            assert_eq!(
                this.next(vec![refused.clone()]),
                Some(refused),
                "the list moved and the refusal stood"
            );
        });
    }

    /// A send not taken for now waits [`RETRY`] aside: the next pick goes
    /// past it, to the networks after it and the leaves, and nothing is held
    /// busy; once its wait is up it is due again.
    #[gpui_kit::test]
    fn a_send_waiting_to_be_tried_again_lets_the_rest_go(cx: &mut gpui_kit::TestAppContext) {
        let (_, registrar) = registrar(cx);
        let (first, then) = (want("away#0b0b0b0b", "a1"), want("far#0c0c0c0c", ""));
        registrar.update(cx, |this, cx| {
            this.landed(first.clone(), Err(Failed::Again("503".into())), cx);
            assert!(
                this.busy.is_none(),
                "a send waiting to be tried again held the registrar"
            );
            assert_eq!(
                this.next(vec![first.clone(), then.clone()]),
                Some(then.clone())
            );
        });
        cx.executor().advance_clock(RETRY);
        cx.run_until_parked();
        registrar.update(cx, |this, _| {
            assert_eq!(
                this.next(vec![first.clone(), then]),
                Some(first),
                "its wait was up and it stood aside"
            );
        });
    }

    /// A leave refused for good leaves nothing behind: pushd holds no live
    /// row to take back, so it goes from what the server holds (and the
    /// prefs), not tried again each time the inputs move.
    #[gpui_kit::test]
    fn a_leave_refused_for_good_is_forgotten(cx: &mut gpui_kit::TestAppContext) {
        let (_, registrar) = registrar(cx);
        let held = want("away#0b0b0b0b", "a1");
        let at = (held.chain.clone(), held.kind.clone());
        registrar.update(cx, |this, cx| {
            this.kept.insert(at.clone(), held.clone());
            let leave = Wanted {
                token: String::new(),
                ..held
            };
            this.landed(leave, Err(Failed::Stopped("404".into())), cx);
            assert!(
                !this.kept.contains_key(&at),
                "a leave refused for good stays to be sent again"
            );
        });
        let (kept, _) = load();
        assert!(
            !kept.contains_key(&at),
            "the prefs still hold the refused leave"
        );
    }
}
