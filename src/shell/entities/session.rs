//! The node in hand: reached, polled, switched, left. `Session` holds what
//! the shell draws of it (`SessionState`, compared: reached or on its way,
//! which network, what was typed, what went wrong) and owns the work that
//! moves it: the connect attempt and the 2 s status poll, each a task
//! dropped with the attempt it belongs to. The poll writes `Chain` (the
//! height, when it moved, the node's answer) and asks `Account` for the
//! seated key's account when a block moved. No status line: "Connected ·
//! block N" moved with every block, and the screens that say it build it
//! from `Chain`'s height.
//!
//! What other entities do for it follows through events: the network left
//! (`SessionEvent::LeftNetwork`: `Windows` clears the desks and the active
//! program, `Rail` its badges), a failure said over a network in hand
//! (`Toast`), the console brought forward as a node answers (`Connected`,
//! `Windows`).

use super::{Account, Chain, Screen, Slice, on_runtime, spawn_on_runtime};
use crate::backend;
use crate::runtime::notify;
use futures::FutureExt as _;
use futures::future::LocalBoxFuture;
use gpui_kit::{Context, Entity, EventEmitter, Task};
use std::rc::Rc;
use std::time::Duration;

/// Unanswered status polls in a row before the node counts as lost: one
/// miss is a hiccup, two (one `STATUS_EVERY` each) is a node that went away.
pub(crate) const LOST_AFTER: u32 = 2;

/// How often the node's status is asked for while connected.
pub(crate) const STATUS_EVERY: Duration = Duration::from_secs(2);

/// How long a node has to answer the first status before it is not reached.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The session's compared value.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SessionState {
    /// A node answered and is polled; true through a switch in flight.
    pub(crate) connected: bool,
    /// A connect attempt is out.
    pub(crate) connecting: bool,
    /// The node that attempt reaches for: the address it was started
    /// with, whatever is typed since. Empty when none is out.
    pub(crate) reaching: String,
    /// Connected, but the last [`LOST_AFTER`] status polls went unanswered.
    pub(crate) reconnecting: bool,
    /// The node the views are on.
    pub(crate) connected_rpc: String,
    pub(crate) network: String,
    /// The chain id `duck://` links carry, `<network>#<salt>`
    /// ([`ducklink::ChainId`]); the network's name alone when its genesis
    /// yields none.
    pub(crate) chain: String,
    /// The node address being typed or tried.
    pub(crate) endpoint: String,
    /// The typed address refused before it was tried ([`backend::ENDPOINT_REFUSAL`]).
    pub(crate) endpoint_error: String,
    pub(crate) recent_endpoints: Vec<backend::RecentEndpoint>,
    /// The network shares its name with another chain met first: its keys
    /// are its own, and the sign-in screen says so.
    pub(crate) other_chain: bool,
    /// The last connect attempt's failure, drawn under the address field;
    /// cleared by the next keystroke or try. shell/windows.rs also parks a
    /// pop-out that would not open here, so it is only seen on that screen.
    pub(crate) error: String,
}

impl SessionState {
    /// The state at launch: the nodes reached before, the last of them in
    /// the address field (or the default node).
    pub(crate) fn booted() -> Self {
        let recent = backend::recent_endpoints();
        let endpoint = recent
            .first()
            .map(|entry| entry.url.clone())
            .unwrap_or_else(|| backend::DEFAULT_ENDPOINT.to_owned());
        Self {
            endpoint,
            recent_endpoints: recent,
            ..Self::default()
        }
    }
}

/// What other entities do for the session (`Windows`, `Rail`, `Toast`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionEvent {
    /// The network in hand was left (disconnected, or another chain taken
    /// up): every desk's panes, the badges and the active program go.
    LeftNetwork,
    /// A failure said over the network in hand (a switch that did not land).
    Toast(String),
    /// A node answered: the console comes forward.
    Connected,
}

/// Where a status comes from: the node's `/v1/status`, or a test's answer.
pub(crate) type StatusSource =
    Rc<dyn Fn() -> LocalBoxFuture<'static, Result<backend::NodeStatus, String>>>;

pub(crate) struct Session {
    state: SessionState,
    chain: Entity<Chain>,
    account: Entity<Account>,
    screen: Entity<Slice<Screen>>,
    /// The notification centre: told the chain, which its links name.
    center: notify::CenterHandle,
    /// Status polls gone unanswered in a row.
    misses: u32,
    /// The connected node's status, asked by the poll.
    source: Option<StatusSource>,
    /// The connect attempt in flight; dropping it drops the request.
    connect: Option<Task<()>>,
    /// The status poll, while connected.
    poll: Option<Task<()>>,
}

impl EventEmitter<SessionEvent> for Session {}

fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.connect")
}

impl Session {
    pub(crate) fn new(
        chain: Entity<Chain>,
        account: Entity<Account>,
        screen: Entity<Slice<Screen>>,
        center: notify::CenterHandle,
    ) -> Self {
        Self {
            state: SessionState::booted(),
            chain,
            account,
            screen,
            center,
            misses: 0,
            source: None,
            connect: None,
            poll: None,
        }
    }

    /// The node to reach at launch: `DUCKTAPE_RPC`, else the last one
    /// reached, else none.
    pub(crate) fn boot_target() -> Option<String> {
        std::env::var("DUCKTAPE_RPC").ok().or_else(|| {
            backend::recent_endpoints()
                .first()
                .map(|entry| entry.url.clone())
        })
    }

    pub(crate) fn get(&self) -> &SessionState {
        &self.state
    }

    /// A field-wise edit, compared as a whole; notifies when it moved.
    fn edit(&mut self, edit: impl FnOnce(&mut SessionState), cx: &mut Context<Self>) -> bool {
        super::slice::edit_compared(&mut self.state, edit, cx)
    }

    /// The node's `/v1/status`, as the poll asks for it.
    pub(crate) fn status_source(origin: &str) -> StatusSource {
        let client = backend::RpcClient::new(origin);
        Rc::new(move || {
            let client = client.clone();
            async move { client.status().await.map_err(|error| error.to_string()) }.boxed_local()
        })
    }

    /// The address as typed. A new address makes the last failure's
    /// sentence stale: it named a node this attempt is not about.
    pub(crate) fn set_endpoint(&mut self, text: String, cx: &mut Context<Self>) {
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
    pub(crate) fn submit(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        match backend::endpoint_origin(&self.state.endpoint) {
            Some(origin) => self.connect(origin, cx),
            None => {
                self.edit(
                    |state| {
                        state.error.clear();
                        state.endpoint_error = backend::ENDPOINT_REFUSAL.into();
                    },
                    cx,
                );
            }
        }
    }

    /// Reaches the node at `origin`: its first status answers, and the
    /// network it serves becomes the one in hand.
    pub(crate) fn connect(&mut self, origin: String, cx: &mut Context<Self>) {
        let source = Self::status_source(&origin);
        self.connect_through(origin, source, cx);
    }

    /// As `connect`, with the status coming from `source` (the node's, or a
    /// test's).
    pub(crate) fn connect_through(
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
        let ask = source.clone();
        let work = async move {
            match tokio::time::timeout(CONNECT_TIMEOUT, ask()).await {
                Ok(answer) => answer,
                Err(_) => Err("the node did not answer in time".into()),
            }
        };
        // a new attempt drops the last one: its answer never lands
        self.connect = Some(spawn_on_runtime(cx, work, move |this, answer, cx| {
            this.connect_answered(origin, source, answer, cx)
        }));
    }

    /// The first status answered, or not.
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
            Err(error) => return self.connect_failed(origin, error, cx),
        };
        crate::perf::mark("connected");
        // refused like a node that never answered: a switch keeps the
        // network in hand
        if status.contract != backend::noded::NODE_CONTRACT {
            let error = format!(
                "this node speaks contract {}; this app speaks {}",
                status.contract,
                backend::noded::NODE_CONTRACT
            );
            return self.connect_failed(origin, error, cx);
        }
        let keyring = match backend::bind_keyring(&status.network, status.time) {
            Ok(keyring) => keyring,
            Err(error) => return self.connect_failed(origin, error, cx),
        };
        let other_chain = keyring.other_chain;
        // the chain id `duck://` links name: the network and its genesis
        // salt; the name alone when the genesis yields none
        let chain = ducklink::ChainId::of(&status.network, &status.genesis)
            .map_or_else(|| status.network.clone(), |chain| chain.to_string());
        let network = status.network.clone();
        let left = self.account.update(cx, |account, cx| {
            account.take_up(keyring, origin.clone(), network.clone(), cx)
        });
        backend::note_endpoint(backend::RecentEndpoint {
            url: origin.clone(),
            network: network.clone(),
            founded: status.time,
            other_chain,
        });
        self.center.lock().set_network(&chain);
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
        self.misses = 0;
        self.apply_status(&status, cx);
        drop(crate::runtime::connected(
            &backend::RpcClient::new(origin),
            &network,
            &chain,
        ));
        if left {
            cx.emit(SessionEvent::LeftNetwork);
        }
        cx.emit(SessionEvent::Connected);
        self.account.update(cx, |account, cx| {
            account.open_device_key(cx);
            account.resolve(cx);
        });
        self.source = Some(source);
        self.start_poll(STATUS_EVERY, cx);
    }

    /// The node at `origin` was not reached. A switch that did not land:
    /// the network in hand stays as it was, and the failure is said over it.
    fn connect_failed(&mut self, origin: String, error: String, cx: &mut Context<Self>) {
        // a node this device reached before is named by its network, the
        // way the Recent list names it
        let who = self
            .state
            .recent_endpoints
            .iter()
            .find(|entry| entry.url == origin && !entry.network.is_empty())
            .map(|entry| format!("{} ({})", entry.network, entry.url))
            .unwrap_or(origin);
        let error = backend::connect_error(&who, error);
        if self.state.connected {
            self.edit(|state| state.endpoint = state.connected_rpc.clone(), cx);
            cx.emit(SessionEvent::Toast(error));
            return;
        }
        self.edit(|state| state.error = error, cx);
    }

    /// The poll's answer: a status, or none. A moved height asks the node
    /// which account the seated key holds now (a block may have created
    /// it, or renamed it).
    pub(crate) fn status_answered(
        &mut self,
        answer: Result<backend::NodeStatus, String>,
        cx: &mut Context<Self>,
    ) {
        let _timed = timed();
        match answer {
            Ok(status) => {
                self.misses = 0;
                self.edit(|state| state.reconnecting = false, cx);
                let moved =
                    i64::try_from(status.height).unwrap_or(-1) != self.chain.read(cx).height;
                self.apply_status(&status, cx);
                if moved {
                    drop(crate::runtime::deployments_checked());
                    self.account.update(cx, |account, cx| account.resolve(cx));
                }
            }
            Err(error) => {
                tracing::debug!(target: "ducktape::app", %error, "status not answered");
                self.misses = self.misses.saturating_add(1);
                let lost = self.misses >= LOST_AFTER;
                self.edit(|state| state.reconnecting = state.connected && lost, cx);
            }
        }
    }

    /// The node's answer onto `Chain`: the height, the wall second it
    /// moved, the second it answered (uncompared), the answer itself.
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

    /// The status poll: every `STATUS_EVERY` (the first after `first`), a
    /// node that hangs as gone as one that refuses.
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

    /// "Retry now": the status asked for at once, and the poll counted
    /// from it.
    pub(crate) fn poll_now(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.state.connected {
            self.start_poll(Duration::ZERO, cx);
        }
    }

    /// Off every network: the Connect screen, nothing of the last network
    /// kept.
    pub(crate) fn disconnect(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.connect = None;
        self.poll = None;
        self.source = None;
        self.misses = 0;
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
        cx.emit(SessionEvent::LeftNetwork);
    }

    /// Another node from the switcher: reached first, and only once it
    /// answers does the console leave the network in hand (`connect_failed`
    /// leaves everything as it was).
    pub(crate) fn switch(&mut self, origin: String, cx: &mut Context<Self>) {
        if origin == self.state.connected_rpc && !self.state.connecting {
            return;
        }
        self.connect(origin, cx);
    }

    /// A node taken off the Recent list.
    pub(crate) fn forget(&mut self, url: &str, cx: &mut Context<Self>) {
        let _timed = timed();
        backend::forget_endpoint(url);
        self.edit(
            |state| state.recent_endpoints = backend::recent_endpoints(),
            cx,
        );
    }

    /// A failure said under the address field (a window that would not
    /// open, `shell/windows.rs`).
    pub(crate) fn note_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.edit(|state| state.error = error, cx);
    }

    /// The chain, account and screen the session writes, for a test to read.
    #[cfg(test)]
    pub(crate) fn parts(&self) -> (Entity<Chain>, Entity<Account>, Entity<Slice<Screen>>) {
        (
            self.chain.clone(),
            self.account.clone(),
            self.screen.clone(),
        )
    }

    /// The state a test starts from.
    #[cfg(test)]
    pub(crate) fn seed(&mut self, state: SessionState, cx: &mut Context<Self>) {
        self.misses = if state.reconnecting { LOST_AFTER } else { 0 };
        self.edit(|current| *current = state, cx);
    }

    /// A test's session on a node already reached, polling `source`: what
    /// `connect_answered` leaves, without the network taken up on this
    /// device (its keyring, its recent-nodes entry, the runtime's roster).
    #[cfg(test)]
    pub(crate) fn seed_connected(
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
