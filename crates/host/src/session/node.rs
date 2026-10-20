//! The node in hand: reached, polled, switched, left. `Session` holds what
//! the shell draws of it (`SessionState`, compared: reached or on its way,
//! which network, what was typed, what went wrong) and owns the work that
//! moves it: the connect attempt and the 2 s status poll, each a task
//! dropped with the attempt it belongs to. The poll writes `Chain` (the
//! height, when it moved, the node's answer). No status line: "Connected ·
//! block N" moved with every block, and the screens that say it build it
//! from `Chain`'s height.
//!
//! Leaving a network (a disconnect, or another chain taken up) empties the
//! roster at once, before anything of the next network is read, so neither
//! the rail nor a link is ever read against the left network's programs.
//! What other entities do for it follows through events: the network left
//! (`SessionEvent::LeftNetwork`: `Windows` clears the desks and the active
//! program, `Rail` its badges and rows), a failure said over a network in hand
//! (`Toast`), the console brought forward as a node answers (`Connected`,
//! `Windows`).

use super::{Account, Chain, Screen, Slice, on_runtime, spawn_on_runtime};
use crate::backend;
use crate::runtime::{Roster, notify};
use futures::FutureExt as _;
use futures::future::LocalBoxFuture;
use gpui_kit::{Context, Entity, EventEmitter, Task};
use std::rc::Rc;
use std::time::Duration;

/// How often the node's status is asked for while connected; each other
/// listed network's too (`runtime::follow`).
pub use crate::runtime::STATUS_EVERY;

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
pub type StatusSource =
    Rc<dyn Fn() -> LocalBoxFuture<'static, Result<backend::NodeStatus, String>>>;

pub struct Session {
    state: SessionState,
    chain: Entity<Chain>,
    account: Entity<Account>,
    screen: Entity<Slice<Screen>>,
    center: notify::CenterHandle,
    roster: Roster,
    source: Option<StatusSource>,
    connect: Option<Task<()>>,
    poll: Option<Task<()>>,
    heard: Vec<Task<()>>,
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
        roster: Roster,
    ) -> Self {
        Self {
            state: SessionState::booted(),
            chain,
            account,
            screen,
            center,
            roster,
            source: None,
            connect: None,
            poll: None,
            heard: Vec::new(),
            confirming: false,
            hands_runtime: true,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn apart_from_runtime(&mut self) {
        self.hands_runtime = false;
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
            crate::runtime::connected(client, network, chain);
            self.hear_writes(client, cx);
        }
    }

    fn hear_writes(&mut self, client: &backend::RpcClient, cx: &mut Context<Self>) {
        let programs = crate::runtime::writes(client, ::programs::PROGRAM);
        let identity = crate::runtime::writes(client, identity::PROGRAM);
        self.heard = vec![
            Self::hear(programs, cx, |_, _| crate::runtime::deployments_checked()),
            Self::hear(identity, cx, |this, cx| {
                this.account.update(cx, |account, cx| account.resolve(cx))
            }),
        ];
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
            async move { client.status().await.map_err(|error| error.to_string()) }.boxed_local()
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
        let Ok(keyring) = noded_client::keyring::bind(remotes, &saved.network, saved.founded)
        else {
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
        if self.poll.is_none() {
            self.source = Some(source);
            self.start_poll(STATUS_EVERY, cx);
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
        if self.poll.is_none() {
            self.source = Some(source);
            self.start_poll(STATUS_EVERY, cx);
        }
    }

    fn connect_answered(
        &mut self,
        origin: String,
        source: StatusSource,
        answer: Result<backend::NodeStatus, String>,
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
            Err(error) if self.confirming => return self.unconfirmed(source, error, cx),
            Err(error) => return self.connect_failed(origin, error, cx),
        };
        crate::perf::mark("connected");
        if status.contract != noded_client::wire::NODE_CONTRACT {
            let error = format!(
                "this node speaks contract {}; this app speaks {}",
                status.contract,
                noded_client::wire::NODE_CONTRACT
            );
            if self.confirming {
                self.disconnect(cx);
            }
            return self.connect_failed(origin, error, cx);
        }
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
        let keyring = match noded_client::keyring::bind(remotes, &status.network, status.time) {
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
        self.start_poll(STATUS_EVERY, cx);
    }

    fn connect_failed(&mut self, origin: String, error: String, cx: &mut Context<Self>) {
        let host = origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://"))
            .unwrap_or(&origin);
        let who = self
            .state
            .recent_endpoints
            .iter()
            .find(|entry| entry.url == origin && !entry.network.is_empty())
            .map_or_else(
                || host.to_owned(),
                |entry| format!("{} ({host})", entry.network),
            );
        let error = backend::connect_error(&who, error);
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
        answer: Result<backend::NodeStatus, String>,
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
                if moved && self.hands_runtime && crate::runtime::deployments_due(status.height) {
                    crate::runtime::deployments_checked();
                }
                if moved {
                    self.account
                        .update(cx, |account, cx| account.resolve_unanswered(cx));
                }
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

    fn start_poll(&mut self, first: Duration, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        self.poll = Some(cx.spawn(async move |this, cx| {
            let mut wait = first;
            loop {
                cx.background_executor().timer(wait).await;
                wait = STATUS_EVERY;
                let ask = source.clone();
                let answer = on_runtime(async move {
                    match tokio::time::timeout(STATUS_EVERY, ask()).await {
                        Ok(answer) => answer,
                        Err(_) => Err("the node did not answer in time".into()),
                    }
                })
                .await;
                if this
                    .update(cx, |this, cx| this.status_answered(answer, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// The app's phase moved (`Signal::Lifecycle`): in the background the
    /// status poll stops (the node is not asked while nothing is shown);
    /// coming to the front asks at once and polls on.
    pub fn lifecycle(&mut self, phase: gpui_kit::AppLifecyclePhase, cx: &mut Context<Self>) {
        use gpui_kit::AppLifecyclePhase::{Background, Foreground};
        match phase {
            Background => {
                self.poll = None;
                self.heard.clear();
            }
            Foreground if self.state.connected && self.poll.is_none() => {
                self.start_poll(Duration::ZERO, cx);
                if self.hands_runtime {
                    let client = backend::rpc_client(self.state.connected_rpc.clone());
                    self.hear_writes(&client, cx);
                    crate::runtime::deployments_checked();
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
            self.start_poll(Duration::ZERO, cx);
        }
    }

    /// Off every network: the Connect screen, nothing of the last network
    /// kept.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.connect = None;
        self.poll = None;
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
            crate::runtime::disconnected();
            crate::runtime::notifier::disconnected();
        }
        self.heard.clear();
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
        self.start_poll(STATUS_EVERY, cx);
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
        self.start_poll(STATUS_EVERY, cx);
    }
}
