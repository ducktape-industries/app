//! Reaching a node, keeping up with it, and leaving it.

use super::update::STATUS_EVERY;
use super::{AppMessage as Message, Ducktape, Overlay, Stage, Unlock};
use crate::backend;
use view_wire::Task;

impl Ducktape {
    /// The node: typed, reached, polled, switched, left.
    pub(super) fn on_connect(&mut self, message: Message) -> Task<Message> {
        match message {
            // A new address or a new try makes the last failure's sentence
            // stale: it named a node this attempt is not about.
            Message::EndpointTyped(text) => {
                self.endpoint = text;
                self.endpoint_error.clear();
                self.error.clear();
                Task::none()
            }
            Message::ConnectSubmit => match backend::endpoint_origin(&self.endpoint) {
                Some(origin) => self.update(Message::ConnectTo(origin)),
                None => {
                    self.error.clear();
                    self.endpoint_error = backend::ENDPOINT_REFUSAL.into();
                    Task::none()
                }
            },
            Message::ConnectTo(origin) => {
                self.endpoint = origin.clone();
                self.endpoint_error.clear();
                self.error.clear();
                self.connecting = true;
                self.status = format!("Reaching {origin}…");
                self.connect_generation += 1;
                let generation = self.connect_generation;
                let (task, handle) = Task::future(async move {
                    let client = backend::RpcClient::new(origin.clone());
                    let status =
                        tokio::time::timeout(std::time::Duration::from_secs(10), client.status())
                            .await;
                    match status {
                        Ok(Ok(status)) => Message::Connected {
                            generation,
                            origin,
                            status,
                        },
                        Ok(Err(error)) => Message::ConnectFailed {
                            generation,
                            error: error.to_string(),
                        },
                        Err(_) => Message::ConnectFailed {
                            generation,
                            error: "the node did not answer in time".into(),
                        },
                    }
                })
                .abortable();
                self.connect_task = Some(handle.abort_on_drop());
                task
            }
            Message::Connected {
                generation,
                origin,
                status,
            } => {
                if generation != self.connect_generation {
                    return Task::none();
                }
                self.connect_task = None;
                self.connecting = false;
                // refused like a node that never answered — a switch keeps
                // the network in hand
                if status.contract != backend::noded::NODE_CONTRACT {
                    let error = format!(
                        "this node speaks contract {}; this app speaks {}",
                        status.contract,
                        backend::noded::NODE_CONTRACT
                    );
                    return self.update(Message::ConnectFailed { generation, error });
                }
                let keyring = match backend::bind_keyring(&status.network, status.time) {
                    Ok(keyring) => keyring,
                    Err(error) => return self.update(Message::ConnectFailed { generation, error }),
                };
                let left = self.take_up(keyring.clone());
                let client = backend::RpcClient::new(origin.clone());
                backend::note_endpoint(backend::RecentEndpoint {
                    url: origin.clone(),
                    network: status.network.clone(),
                    founded: status.time,
                    other_chain: keyring.other_chain,
                });
                self.recent_endpoints = backend::recent_endpoints();
                self.connected_rpc = origin;
                self.network = status.network.clone();
                // the chain id links name: the network and its genesis salt
                self.chain = ducklink::ChainId::of(&status.network, &status.genesis)
                    .map_or_else(|| status.network.clone(), |chain| chain.to_string());
                crate::runtime::notify::center().set_network(&self.chain);
                self.connected = true;
                self.status_misses = 0;
                self.apply_status(&status);
                drop(crate::runtime::connected(
                    &client,
                    &self.network,
                    &self.chain,
                ));
                let window = self.raise_console();
                Task::batch([left, window, self.open_device_key(), self.resolve_account()])
            }
            Message::ConnectFailed { generation, error } => {
                if generation != self.connect_generation {
                    return Task::none();
                }
                self.connect_task = None;
                self.connecting = false;
                // A node this device reached before is named by its network,
                // the way the Recent list names it.
                let who = self
                    .recent_endpoints
                    .iter()
                    .find(|entry| entry.url == self.endpoint && !entry.network.is_empty())
                    .map(|entry| format!("{} ({})", entry.network, entry.url))
                    .unwrap_or_else(|| self.endpoint.clone());
                let error = backend::connect_error(&who, error);
                // A switch that did not land: the network in hand stays, as
                // it was, and the failure is said over it.
                if self.connected {
                    self.endpoint = self.connected_rpc.clone();
                    self.status = format!("Connected · block {}", self.height);
                    return self.update(Message::ShowToast(error));
                }
                self.status = "Not connected".into();
                self.error = error;
                Task::none()
            }
            Message::StatusPushed(status) => {
                let moved = i64::try_from(status.height).unwrap_or(-1) != self.height;
                self.status_misses = 0;
                self.apply_status(&status);
                if !moved {
                    return Task::none();
                }
                drop(crate::runtime::deployments_checked());
                // a block may have created the account, or renamed it
                self.resolve_account()
            }
            Message::StatusMissed => {
                self.status_misses = self.status_misses.saturating_add(1);
                if self.reconnecting() {
                    self.status = "Reconnecting…".into();
                }
                Task::none()
            }
            // After a sign-in the key screen stays up (`Unlock::awaiting`)
            // until the node answers: a key with no account goes on to the
            // account step, one with an account to the console, and neither
            // shows the other first. Later answers move no screen.
            Message::AccountResolved { node, key, account } => {
                if node != self.connected_rpc || key != self.signer_key {
                    return Task::none();
                }
                if let Stage::Unlock(Unlock { awaiting: true, .. }) = self.stage {
                    self.stage = match account {
                        None => Stage::Account(Default::default()),
                        Some(_) => Stage::Desk,
                    };
                }
                self.account = Some(account);
                Task::none()
            }
            Message::Disconnect => {
                self.connect_generation += 1;
                self.connect_task = None;
                self.connected = false;
                self.connecting = false;
                self.status_misses = 0;
                self.connected_rpc.clear();
                self.network.clear();
                self.chain.clear();
                self.status = "Not connected".into();
                let left = self.leave_network();
                self.stage = Stage::Connect;
                left
            }
            // The console stays on the network in hand while the other is
            // reached: the status reads "Reaching …", and a node that does not
            // answer leaves everything as it was (`ConnectFailed`).
            Message::SwitchNetwork(origin) => {
                self.close(Overlay::Network);
                if origin == self.connected_rpc && !self.connecting {
                    return Task::none();
                }
                self.update(Message::ConnectTo(origin))
            }
            Message::ForgetEndpoint(url) => {
                backend::forget_endpoint(&url);
                self.recent_endpoints = backend::recent_endpoints();
                Task::none()
            }
            Message::Tick => {
                if !self.connected {
                    return Task::none();
                }
                let client = backend::RpcClient::new(self.connected_rpc.clone());
                Task::future(async move {
                    // a node that hangs is as gone as one that refuses
                    match tokio::time::timeout(STATUS_EVERY, client.status()).await {
                        Ok(Ok(status)) => Message::StatusPushed(status),
                        Ok(Err(error)) => {
                            tracing::debug!(target: "ducktape::app", %error, "status not answered");
                            Message::StatusMissed
                        }
                        Err(_) => Message::StatusMissed,
                    }
                })
            }
            _ => unreachable!("routed by `update`"),
        }
    }

    fn apply_status(&mut self, status: &backend::NodeStatus) {
        let height = i64::try_from(status.height).unwrap_or(-1);
        if height != self.height {
            self.block_seen = self.wall_now;
        }
        self.height = height;
        self.node = Some(status.clone());
        // mid-switch the line reads "Reaching …" until the other node answers
        if !self.connecting {
            self.status = format!("Connected · block {}", status.height);
        }
    }

    /// The network `keyring` names becomes the one in hand. Another chain
    /// than the last (a switch, not a second node of the same network):
    /// nothing of the last one — its seated key, account, open view —
    /// carries over, and the key step comes first. A second node of the
    /// same network keeps the screen it was on.
    pub(super) fn take_up(&mut self, keyring: backend::Keyring) -> Task<Message> {
        let left = match keyring.dir != self.keyring {
            true => self.leave_network(),
            false => Task::none(),
        };
        if matches!(self.stage, Stage::Connect) {
            self.stage = Stage::Unlock(Unlock::default());
        }
        self.keyring = keyring.dir;
        self.other_chain = keyring.other_chain;
        self.key_exists = backend::key_exists(&self.keyring);
        left
    }

    /// Everything that belonged to the network being left: its seated key
    /// (the seat is one for the whole app), the account it resolved to, the
    /// open view and its badges, and any sign-in half done.
    fn leave_network(&mut self) -> Task<Message> {
        // dropping the old ones wipes their secrets and cancels their tasks
        self.sign_in = Default::default();
        self.stage = Stage::Unlock(Unlock::default());
        self.account = None;
        self.active = None;
        // every window's panes go, each desk keeping its measure
        for layout in self.layouts.values_mut() {
            layout.clear();
        }
        self.badges.clear();
        self.overlay = None;
        self.node = None;
        self.keyring.clear();
        self.other_chain = false;
        self.key_exists = false;
        self.signer_key.clear();
        Task::future(async {
            backend::lock_signer().await;
        })
        .discard()
    }

    /// Asks the node which account the seated key belongs to; the answer
    /// lands as [`Message::AccountResolved`]. A failed ask keeps what the
    /// menu bar already shows — the next block asks again.
    pub(super) fn resolve_account(&self) -> Task<Message> {
        let Ok(key) = backend::hex_decode(&self.signer_key) else {
            return Task::none();
        };
        if key.is_empty() || !self.connected {
            return Task::none();
        }
        let node = self.connected_rpc.clone();
        let client = backend::RpcClient::new(node.clone());
        let network = self.network.clone();
        let signer = self.signer_key.clone();
        Task::future(async move {
            match backend::passkey::account_of_key(&client, &network, key).await {
                Ok(account) => Some(Message::AccountResolved {
                    node,
                    key: signer,
                    account,
                }),
                Err(error) => {
                    tracing::debug!(target: "ducktape::app", %error, "account not resolved");
                    None
                }
            }
        })
        .and_then(Task::done)
    }
}
