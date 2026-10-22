//! The node in hand: reached, followed through its heads socket, switched, left. `Session`
//! holds what the shell draws of it and owns the work that moves it, each a task dropped with
//! the attempt it belongs to; it writes `Chain` from each head and each status answered.

use super::{Account, Chain, Screen, Slice, on_runtime, spawn_on_runtime};
use crate::backend;
use crate::runtime::{Roster, notify};
use futures::future::LocalBoxFuture;
use futures::stream::LocalBoxStream;
use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::{Context, Entity, EventEmitter, Task};
use std::rc::Rc;

/// The session's compared value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionState {
    /// A node answered and is polled; true through a switch in flight.
    pub connected: bool,
    /// A connect attempt is out.
    pub connecting: bool,
    /// The node that attempt reaches for: the address it was started
    /// with, whatever is typed since. Empty when none is out.
    pub reaching: String,
    /// Connected, but the last status poll went unanswered.
    pub reconnecting: bool,
    /// The node the views are on.
    pub connected_rpc: String,
    pub network: String,
    /// The chain id `duck://` links carry, `<network>#<salt>`
    /// ([`ducklink::ChainId`]); the network's name alone when its genesis
    /// yields none.
    pub chain: String,
    /// The node address being typed or tried.
    pub endpoint: String,
    /// The typed address refused before it was tried ([`noded_client::endpoint::ENDPOINT_REFUSAL`]).
    pub endpoint_error: String,
    pub recent_endpoints: Vec<backend::RecentEndpoint>,
    /// The network shares its name with another chain met first: its keys
    /// are its own, and the sign-in screen says so.
    pub other_chain: bool,
    /// The last connect attempt's failure, drawn under the address field;
    /// cleared by the next keystroke or try. shell/windows.rs also parks a
    /// pop-out that would not open here, so it is only seen on that screen.
    pub error: String,
}

impl SessionState {
    /// The state at launch: the nodes reached before, the last of them in
    /// the address field (or the default node).
    pub fn booted() -> Self {
        let recent = backend::recent_endpoints();
        let endpoint = recent
            .first()
            .map(|entry| entry.url.clone())
            .unwrap_or_else(|| noded_client::endpoint::DEFAULT_ENDPOINT.to_owned());
        Self {
            endpoint,
            recent_endpoints: recent,
            ..Self::default()
        }
    }
}

/// What other entities do for the session (`Windows`, `Rail`, `Toast`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// The network in hand was left (disconnected, or another chain taken
    /// up): every desk's panes, the badges and the active program go.
    LeftNetwork,
    /// A failure said over the network in hand (a switch that did not land).
    Toast(String),
    /// A node answered: the console comes forward.
    Connected,
}

/// Where a status comes from: the node's `/v1/status`, or a test's answer.
/// A `Decode` error is a node that answered in a shape this app does not
/// read (another contract: its sentence names both), not a node away.
pub type StatusSource =
    Rc<dyn Fn() -> LocalBoxFuture<'static, Result<backend::NodeStatus, noded_client::Error>>>;

/// Where heads come from: the node's heads socket, or a test's feed. An error is the node lost
/// while the source reaches for it again; its end, lost until an event starts it again.
pub type HeadsSource =
    Rc<dyn Fn() -> LocalBoxStream<'static, Result<noded_client::wire::Head, noded_client::Error>>>;

pub struct Session {
    state: SessionState,
    chain: Entity<Chain>,
    account: Entity<Account>,
    screen: Entity<Slice<Screen>>,
    center: notify::CenterHandle,
    network: std::sync::Arc<crate::runtime::Network>,
    roster: Roster,
    source: Option<StatusSource>,
    heads_of: Rc<dyn Fn(&str) -> HeadsSource>,
    connect: Option<Task<()>>,
    follow: Option<Task<()>>,
    asking: Option<Task<()>>,
    watched: bool,
    heard: Vec<Task<()>>,
    client: Option<backend::RpcClient>,
    armed: Option<abi::Roles>,
    watch: fn(&backend::RpcClient, &str) -> crate::runtime::Writes,
    confirming: bool,
    hands_runtime: bool,
}

impl EventEmitter<SessionEvent> for Session {}

fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.connect")
}

impl Session {
    pub fn new(
        chain: Entity<Chain>,
        account: Entity<Account>,
        screen: Entity<Slice<Screen>>,
        center: notify::CenterHandle,
        network: std::sync::Arc<crate::runtime::Network>,
        roster: Roster,
    ) -> Self {
        Self {
            state: SessionState::booted(),
            chain,
            account,
            screen,
            center,
            network,
            roster,
            source: None,
            heads_of: Rc::new(Self::heads_source),
            connect: None,
            follow: None,
            asking: None,
            watched: false,
            heard: Vec::new(),
            client: None,
            armed: None,
            watch: crate::runtime::writes,
            confirming: false,
            hands_runtime: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn watching_with(
        &mut self,
        client: backend::RpcClient,
        watch: fn(&backend::RpcClient, &str) -> crate::runtime::Writes,
    ) {
        self.client = Some(client);
        self.watch = watch;
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn apart_from_runtime(&mut self) {
        self.hands_runtime = false;
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn heads_through(&mut self, heads: HeadsSource) {
        self.heads_of = Rc::new(move |_: &str| heads.clone());
    }

    fn hand_runtime(
        &mut self,
        client: &backend::RpcClient,
        network: &str,
        chain: &str,
        cx: &mut Context<Self>,
    ) {
        if self.hands_runtime {
            crate::runtime::take_up(&self.center, chain);
            self.network.connected(client, network, chain);
            self.client = Some(client.clone());
            self.armed = None;
            self.heard.clear();
            let roles = self
                .chain
                .read(cx)
                .node
                .as_ref()
                .map(|node| node.roles.clone());
            if let Some(roles) = roles {
                self.follow_roles(&roles, cx);
            }
        }
    }

    fn follow_roles(&mut self, roles: &abi::Roles, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        client.heard_roles(roles);
        if self.armed.as_ref() == Some(roles) {
            return;
        }
        self.armed = Some(roles.clone());
        self.hear_writes(&client, roles, cx);
    }

    fn hear_writes(
        &mut self,
        client: &backend::RpcClient,
        roles: &abi::Roles,
        cx: &mut Context<Self>,
    ) {
        let watch = self.watch;
        let writes = |role| roles.bound(role).ok().map(|program| watch(client, program));
        let (programs, identity) = (writes(abi::role::PROGRAMS), writes(abi::role::IDENTITY));
        let programs = programs.map(|heard| {
            Self::hear(heard, cx, |this, cx| {
                this.network.deployments_checked();
                this.ask_status(cx);
            })
        });
        let identity = identity.map(|heard| {
            Self::hear(heard, cx, |this, cx| {
                this.account.update(cx, |account, cx| account.resolve(cx))
            })
        });
        self.heard = programs.into_iter().chain(identity).collect();
    }

    fn hear(
        mut writes: crate::runtime::Writes,
        cx: &mut Context<Self>,
        heard: fn(&mut Self, &mut Context<Self>),
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            while writes.next().await.is_some() {
                if this.update(cx, heard).is_err() {
                    return;
                }
            }
            tracing::warn!(
                target: "ducktape::app",
                reason = "writes_unheard",
                "the node refused a changes socket the session follows"
            );
        })
    }

    /// The node to reach at launch: `DUCKTAPE_RPC`, else the last one
    /// reached, else none.
    pub fn boot_target() -> Option<String> {
        std::env::var("DUCKTAPE_RPC").ok().or_else(|| {
            backend::recent_endpoints()
                .first()
                .map(|entry| entry.url.clone())
        })
    }

    pub fn get(&self) -> &SessionState {
        &self.state
    }

    fn edit(&mut self, edit: impl FnOnce(&mut SessionState), cx: &mut Context<Self>) -> bool {
        super::slice::edit_compared(&mut self.state, edit, cx)
    }

    /// The node's `/v1/status`, as the poll asks for it.
    pub fn status_source(origin: &str) -> StatusSource {
        let client = backend::rpc_client(origin);
        Rc::new(move || {
            let client = client.clone();
            async move { client.status().await }.boxed_local()
        })
    }

    fn heads_source(origin: &str) -> HeadsSource {
        let client = backend::rpc_client(origin);
        Rc::new(move || {
            let client = client.clone();
            let (mut tell, heard) = futures::channel::mpsc::channel(0);
            let pump = async move {
                use futures::SinkExt as _;
                let mut block_time_ms = None;
                loop {
                    if block_time_ms.is_none() {
                        block_time_ms = client
                            .status()
                            .await
                            .ok()
                            .map(|status| status.block_time_ms);
                    }
                    let lost = match client.heads_kept(block_time_ms).await {
                        Ok(mut heads) => {
                            if block_time_ms.is_none() {
                                block_time_ms = client
                                    .status()
                                    .await
                                    .ok()
                                    .map(|status| status.block_time_ms);
                            }
                            while let Some(Ok(head)) = heads.next().await {
                                if tell.send(Ok(head)).await.is_err() {
                                    return;
                                }
                            }
                            noded_client::Error::Transport("the heads socket closed".into())
                        }
                        Err(error) => error,
                    };
                    let refused = refuses(&lost);
                    let unheard = tell.send(Err(lost)).await.is_err();
                    let ends = unheard || refused;
                    if ends {
                        return;
                    }
                    match client.answering(block_time_ms).await {
                        Ok(answered) => block_time_ms = Some(answered),
                        Err(_) => return,
                    }
                }
            };
            let pumped = pump.into_stream().filter_map(|()| std::future::ready(None));
            futures::stream::select(heard, pumped).boxed_local()
        })
    }

    /// The address as typed. A new address makes the last failure's
    /// sentence stale: it named a node this attempt is not about.
    pub fn set_endpoint(&mut self, text: String, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(
            |state| {
                state.endpoint = text;
                state.endpoint_error.clear();
                state.error.clear();
            },
            cx,
        );
    }

    /// Connect, with the address typed: reached when it is an address.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        match noded_client::endpoint::endpoint_origin(&self.state.endpoint) {
            Some(origin) => self.connect(origin, cx),
            None => {
                self.edit(
                    |state| {
                        state.error.clear();
                        state.endpoint_error = noded_client::endpoint::ENDPOINT_REFUSAL.into();
                    },
                    cx,
                );
            }
        }
    }

    /// Reaches the node at `origin`: its first status answers, and the
    /// network it serves becomes the one in hand.
    pub fn connect(&mut self, origin: String, cx: &mut Context<Self>) {
        let source = Self::status_source(&origin);
        self.connect_through(origin, source, cx);
    }

    /// As `connect`, with the status coming from `source` (the node's, or a
    /// test's).
    pub fn connect_through(
        &mut self,
        origin: String,
        source: StatusSource,
        cx: &mut Context<Self>,
    ) {
        let _timed = timed();
        self.edit(
            |state| {
                state.endpoint = origin.clone();
                state.endpoint_error.clear();
                state.error.clear();
                state.connecting = true;
                state.reaching = origin.clone();
            },
            cx,
        );
        if !self.state.connected
            && let Some(saved) = backend::recent_endpoints()
                .into_iter()
                .find(|entry| entry.url == origin)
        {
            self.presume(&origin, saved, cx);
        }
        let work = source();
        self.connect = Some(spawn_on_runtime(cx, work, move |this, answer, cx| {
            this.connect_answered(origin, source, answer, cx)
        }));
    }

    fn presume(&mut self, origin: &str, saved: backend::RecentEndpoint, cx: &mut Context<Self>) {
        let Some(genesis) = saved.genesis else {
            return;
        };
        let remotes = &crate::contract::os().dirs().remotes;
        let Ok(keyring) = noded_client::keyring::bind(
            crate::contract::os().disk(),
            remotes,
            &saved.network,
            saved.founded,
        ) else {
            return;
        };
        let other_chain = keyring.other_chain;
        let chain = crate::runtime::chain_named(&saved.network, &genesis);
        let network = saved.network.clone();
        let client = backend::rpc_client(origin.to_owned());
        self.account.update(cx, |account, cx| {
            account.take_up(keyring, client.clone(), network.clone(), cx)
        });
        self.edit(
            |state| {
                state.connected_rpc = origin.to_owned();
                state.network = network.clone();
                state.chain = chain.clone();
                state.other_chain = other_chain;
                state.connected = true;
                state.reconnecting = false;
            },
            cx,
        );
        self.hand_runtime(&client, &network, &chain, cx);
        cx.emit(SessionEvent::Connected);
        self.account.update(cx, |account, cx| {
            if saved.signed_in {
                account.presume(cx);
            }
            account.open_device_key(cx);
            account.resolve(cx);
        });
        self.confirming = true;
    }

    fn unconfirmed(&mut self, source: StatusSource, error: String, cx: &mut Context<Self>) {
        tracing::debug!(target: "ducktape::app", %error, "status not answered");
        self.edit(|state| state.reconnecting = true, cx);
        if self.follow.is_none() {
            self.source = Some(source);
            self.start_follow(false, cx);
        }
    }

    fn confirmed(
        &mut self,
        origin: String,
        source: StatusSource,
        status: &backend::NodeStatus,
        cx: &mut Context<Self>,
    ) {
        self.confirming = false;
        backend::note_endpoint(backend::RecentEndpoint {
            url: origin,
            network: status.network.clone(),
            founded: status.time,
            other_chain: self.state.other_chain,
            genesis: Some(status.genesis),
            signed_in: false,
        });
        self.edit(
            |state| {
                state.recent_endpoints = backend::recent_endpoints();
                state.reconnecting = false;
            },
            cx,
        );
        self.apply_status(status, cx);
        self.follow_roles(&status.roles, cx);
        if self.follow.is_none() {
            self.source = Some(source);
            self.start_follow(false, cx);
        }
    }

    fn connect_answered(
        &mut self,
        origin: String,
        source: StatusSource,
        answer: Result<backend::NodeStatus, noded_client::Error>,
        cx: &mut Context<Self>,
    ) {
        let _timed = timed();
        self.connect = None;
        self.edit(
            |state| {
                state.connecting = false;
                state.reaching.clear();
            },
            cx,
        );
        let status = match answer {
            Ok(status) => status,
            Err(noded_client::Error::Decode(refusal)) if self.confirming => {
                return self.unreadable(origin, refusal.sentence, cx);
            }
            Err(error) if self.confirming => {
                return self.unconfirmed(source, error.to_string(), cx);
            }
            Err(error) => return self.connect_failed(origin, error.to_string(), cx),
        };
        crate::perf::mark("connected");
        let presumed = std::mem::take(&mut self.confirming);
        if presumed
            && crate::runtime::chain_of(&status) == self.state.chain
            && status.network == self.state.network
        {
            return self.confirmed(origin, source, &status, cx);
        }
        if presumed {
            self.disconnect(cx);
        }
        let remotes = &crate::contract::os().dirs().remotes;
        let keyring = match noded_client::keyring::bind(
            crate::contract::os().disk(),
            remotes,
            &status.network,
            status.time,
        ) {
            Ok(keyring) => keyring,
            Err(error) => return self.connect_failed(origin, error, cx),
        };
        let other_chain = keyring.other_chain;
        let chain = crate::runtime::chain_of(&status);
        let network = status.network.clone();
        let client = backend::rpc_client(origin.clone());
        let left = self.account.update(cx, |account, cx| {
            account.take_up(keyring, client.clone(), network.clone(), cx)
        });
        backend::note_endpoint(backend::RecentEndpoint {
            url: origin.clone(),
            network: network.clone(),
            founded: status.time,
            other_chain,
            genesis: Some(status.genesis),
            signed_in: false,
        });
        self.edit(
            |state| {
                state.recent_endpoints = backend::recent_endpoints();
                state.connected_rpc = origin.clone();
                state.network = network.clone();
                state.chain = chain.clone();
                state.other_chain = other_chain;
                state.connected = true;
                state.reconnecting = false;
            },
            cx,
        );
        self.apply_status(&status, cx);
        if left {
            self.left_network(cx);
        }
        self.hand_runtime(&client, &network, &chain, cx);
        cx.emit(SessionEvent::Connected);
        self.account.update(cx, |account, cx| {
            account.open_device_key(cx);
            account.resolve(cx);
        });
        self.source = Some(source);
        self.start_follow(false, cx);
    }

    fn unreadable(&mut self, origin: String, sentence: String, cx: &mut Context<Self>) {
        self.disconnect(cx);
        let who = self.who(&origin);
        self.connect_failed(origin, format!("{who}: {sentence}"), cx);
    }

    fn who(&self, origin: &str) -> String {
        let host = origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://"))
            .unwrap_or(origin);
        self.state
            .recent_endpoints
            .iter()
            .find(|entry| entry.url == origin && !entry.network.is_empty())
            .map_or_else(
                || host.to_owned(),
                |entry| format!("{} ({host})", entry.network),
            )
    }

    fn connect_failed(&mut self, origin: String, error: String, cx: &mut Context<Self>) {
        let error = backend::connect_error(&self.who(&origin), error);
        if self.state.connected {
            self.edit(|state| state.endpoint = state.connected_rpc.clone(), cx);
            cx.emit(SessionEvent::Toast(error));
            return;
        }
        self.edit(|state| state.error = error, cx);
    }

    /// The poll's answer: a status, or none.
    pub fn status_answered(
        &mut self,
        answer: Result<backend::NodeStatus, noded_client::Error>,
        cx: &mut Context<Self>,
    ) {
        let _timed = timed();
        match answer {
            Ok(status) if self.confirming => {
                let origin = self.state.connected_rpc.clone();
                if let Some(source) = self.source.clone() {
                    self.connect_answered(origin, source, Ok(status), cx);
                }
            }
            Ok(status) => {
                self.edit(|state| state.reconnecting = false, cx);
                let moved =
                    i64::try_from(status.height).unwrap_or(-1) != self.chain.read(cx).height;
                self.apply_status(&status, cx);
                self.follow_roles(&status.roles, cx);
                let due =
                    moved && self.hands_runtime && self.network.deployments_due(status.height);
                if due {
                    self.network.deployments_checked();
                }
                if moved {
                    self.account
                        .update(cx, |account, cx| account.resolve_unanswered(cx));
                }
            }
            Err(noded_client::Error::Decode(refusal)) => {
                let origin = self.state.connected_rpc.clone();
                self.unreadable(origin, refusal.sentence, cx);
            }
            Err(error) => {
                tracing::debug!(target: "ducktape::app", %error, "status not answered");
                self.edit(|state| state.reconnecting = state.connected, cx);
            }
        }
    }

    fn apply_status(&mut self, status: &backend::NodeStatus, cx: &mut Context<Self>) {
        let height = i64::try_from(status.height).unwrap_or(-1);
        let now = notify::wall();
        let status = status.clone();
        self.chain.update(cx, |chain, cx| {
            let block_seen = match height != chain.height {
                true => now,
                false => chain.block_seen,
            };
            chain.set(
                Chain {
                    height,
                    block_seen,
                    node: Some(status),
                    heard: now,
                },
                cx,
            );
        });
    }

    fn start_follow(&mut self, ask_first: bool, cx: &mut Context<Self>) {
        if self.source.is_none() {
            return;
        }
        if ask_first {
            self.ask_status(cx);
        }
        let heads = (self.heads_of)(&self.state.connected_rpc);
        self.follow = Some(cx.spawn(async move |this, cx| {
            let mut heard = heads();
            while let Some(item) = on_runtime(heard.next()).await {
                let told = this.update(cx, |this, cx| match item {
                    Ok(head) => this.head_heard(head, cx),
                    Err(error) => this.heads_lost(error.to_string(), cx),
                });
                if told.is_err() {
                    return;
                }
            }
            let ended = "the heads socket ended".to_owned();
            let _ = this.update(cx, |this, cx| this.heads_lost(ended, cx));
        }));
    }

    fn head_heard(&mut self, head: noded_client::wire::Head, cx: &mut Context<Self>) {
        let was_lost = self.state.reconnecting;
        self.edit(|state| state.reconnecting = false, cx);
        let height = i64::try_from(head.height).unwrap_or(-1);
        let now = notify::wall();
        let moved = self.chain.update(cx, |chain, cx| {
            let moved = height != chain.height;
            let node = chain.node.clone().map(|node| backend::NodeStatus {
                height: head.height,
                tip: head.id,
                ..node
            });
            let block_seen = match moved {
                true => now,
                false => chain.block_seen,
            };
            let next = Chain {
                height,
                block_seen,
                node,
                heard: now,
            };
            chain.set(next, cx);
            moved
        });
        let due = moved && self.hands_runtime && self.network.deployments_due(head.height);
        if due {
            self.network.deployments_checked();
        }
        if moved {
            self.account
                .update(cx, |account, cx| account.resolve_unanswered(cx));
        }
        let asks = was_lost || self.confirming || self.watched;
        if asks {
            self.ask_status(cx);
        }
    }

    fn heads_lost(&mut self, error: String, cx: &mut Context<Self>) {
        tracing::debug!(target: "ducktape::app", %error, "heads not heard");
        self.edit(|state| state.reconnecting = state.connected, cx);
    }

    pub fn node_watched(&mut self, open: bool, cx: &mut Context<Self>) {
        self.watched = open;
        if open {
            self.ask_status(cx);
        }
    }

    fn ask_status(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        if self.asking.is_some() {
            return;
        }
        self.asking = Some(spawn_on_runtime(cx, source(), |this, answer, cx| {
            this.asking = None;
            this.status_read(answer, cx);
        }));
    }

    fn newest(&self, status: backend::NodeStatus, cx: &mut Context<Self>) -> backend::NodeStatus {
        let chain = self.chain.read(cx);
        let headed = chain
            .node
            .as_ref()
            .filter(|node| node.height > status.height);
        match headed {
            Some(node) => backend::NodeStatus {
                height: node.height,
                tip: node.tip,
                ..status
            },
            None => status,
        }
    }

    fn status_read(
        &mut self,
        answer: Result<backend::NodeStatus, noded_client::Error>,
        cx: &mut Context<Self>,
    ) {
        match answer {
            Ok(status) if self.confirming => self.status_answered(Ok(status), cx),
            Ok(status) => {
                let status = self.newest(status, cx);
                self.apply_status(&status, cx);
                self.follow_roles(&status.roles, cx);
            }
            Err(noded_client::Error::Decode(refusal)) => {
                let origin = self.state.connected_rpc.clone();
                self.unreadable(origin, refusal.sentence, cx);
            }
            Err(error) => {
                tracing::debug!(target: "ducktape::app", %error, "status not answered");
            }
        }
    }

    /// The app's phase moved (`Signal::Lifecycle`): in the background the
    /// status poll stops (the node is not asked while nothing is shown);
    /// coming to the front asks at once and polls on.
    pub fn lifecycle(&mut self, phase: gpui_kit::AppLifecyclePhase, cx: &mut Context<Self>) {
        use gpui_kit::AppLifecyclePhase::{Background, Foreground};
        let resumes = self.state.connected && self.follow.is_none();
        match phase {
            Background => {
                self.follow = None;
                self.asking = None;
                self.heard.clear();
            }
            Foreground if resumes => {
                self.start_follow(true, cx);
                if let Some(roles) = self.armed.clone() {
                    let client = backend::rpc_client(self.state.connected_rpc.clone());
                    client.heard_roles(&roles);
                    self.hear_writes(&client, &roles, cx);
                }
                if self.hands_runtime {
                    self.network.deployments_checked();
                    self.account.update(cx, |account, cx| account.resolve(cx));
                }
            }
            _ => {}
        }
    }

    /// "Retry now": the status asked for at once, and the poll counted
    /// from it.
    pub fn poll_now(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.state.connected {
            self.start_follow(true, cx);
        }
    }

    /// Off every network: the Connect screen, nothing of the last network
    /// kept.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.connect = None;
        self.follow = None;
        self.asking = None;
        self.source = None;
        self.confirming = false;
        self.edit(
            |state| {
                state.connected = false;
                state.connecting = false;
                state.reaching.clear();
                state.reconnecting = false;
                state.connected_rpc.clear();
                state.network.clear();
                state.chain.clear();
                state.other_chain = false;
            },
            cx,
        );
        self.account
            .update(cx, |account, cx| account.leave_network(cx));
        self.chain.update(cx, |chain, cx| {
            let gone = Chain {
                node: None,
                ..chain.clone()
            };
            chain.set(gone, cx);
        });
        self.screen
            .update(cx, |screen, cx| screen.set(Screen::Connect, cx));
        if self.hands_runtime {
            crate::runtime::take_up(&self.center, "");
        }
        self.left_network(cx);
    }

    fn left_network(&mut self, cx: &mut Context<Self>) {
        if self.hands_runtime {
            self.network.disconnected();
            crate::runtime::notifier::disconnected();
        }
        self.heard.clear();
        self.client = None;
        self.armed = None;
        self.roster.clear();
        cx.emit(SessionEvent::LeftNetwork);
    }

    /// Another node from the switcher: reached first, and only once it
    /// answers does the console leave the network in hand (`connect_failed`
    /// leaves everything as it was).
    pub fn switch(&mut self, origin: String, cx: &mut Context<Self>) {
        if origin == self.state.connected_rpc && !self.state.connecting {
            return;
        }
        self.connect(origin, cx);
    }

    /// A node taken off the Recent list.
    pub fn forget(&mut self, url: &str, cx: &mut Context<Self>) {
        let _timed = timed();
        backend::forget_endpoint(url);
        self.edit(
            |state| state.recent_endpoints = backend::recent_endpoints(),
            cx,
        );
    }

    /// A failure said under the address field (a window that would not
    /// open, `shell/windows.rs`).
    pub fn note_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.edit(|state| state.error = error, cx);
    }

    /// The chain, account and screen the session writes, for a test to read.
    #[cfg(any(test, feature = "test-support"))]
    pub fn parts(&self) -> (Entity<Chain>, Entity<Account>, Entity<Slice<Screen>>) {
        (
            self.chain.clone(),
            self.account.clone(),
            self.screen.clone(),
        )
    }

    /// `--live`: a node already reached at `origin`, whose `status` is in
    /// hand: what `connect_answered` leaves, less the network taken up on
    /// this device (no keyring, no recent-nodes entry; the runtime's
    /// roster is the caller's), and the poll started.
    #[cfg(debug_assertions)]
    pub fn live(
        &mut self,
        origin: String,
        status: &backend::NodeStatus,
        chain: String,
        cx: &mut Context<Self>,
    ) {
        crate::runtime::take_up(&self.center, &chain);
        self.edit(
            |state| {
                state.endpoint = origin.clone();
                state.connected_rpc = origin.clone();
                state.network = status.network.clone();
                state.chain = chain.clone();
                state.connected = true;
            },
            cx,
        );
        self.apply_status(status, cx);
        self.source = Some(Self::status_source(&origin));
        self.start_follow(false, cx);
    }

    /// The state a test starts from.
    #[cfg(any(test, feature = "test-support"))]
    pub fn seed(&mut self, state: SessionState, cx: &mut Context<Self>) {
        self.edit(|current| *current = state, cx);
    }

    /// A test's session on a node already reached, polling `source`: what
    /// `connect_answered` leaves, without the network taken up on this
    /// device (its keyring, its recent-nodes entry, the runtime's roster).
    #[cfg(any(test, feature = "test-support"))]
    pub fn seed_connected(
        &mut self,
        state: SessionState,
        source: StatusSource,
        cx: &mut Context<Self>,
    ) {
        self.seed(state, cx);
        self.source = Some(source);
        self.start_follow(false, cx);
    }
}

fn refuses(error: &noded_client::Error) -> bool {
    use noded_client::Error;
    match error {
        Error::Refused(_) | Error::Decode(_) => true,
        Error::Failed { status, .. } => (400..500).contains(status),
        Error::Unreachable(_) | Error::Transport(_) => false,
    }
}
