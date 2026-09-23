//! The reducer: one message in, the state moved, a task out.

use super::{AppMessage as Message, Ducktape, Screen};
use crate::backend;
use view_wire::Subscription;
use view_wire::Task;

/// How often the node's status is asked for while connected.
const STATUS_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

impl Ducktape {
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SetAppearance(mode) => {
                self.appearance = mode;
                backend::save_appearance(mode);
                self.push_props();
                Task::none()
            }
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
                self.connected = true;
                self.status_misses = 0;
                self.screen = Screen::Console;
                self.apply_status(&status);
                drop(crate::runtime::connected(&client, &self.network));
                self.push_props();
                let window = match self.console_win {
                    Some(key) => crate::shell::raise(key),
                    None => {
                        let (key, opened) = crate::shell::open(crate::shell::WindowKind::Console);
                        self.console_win = Some(key);
                        opened.map(Message::ConsoleOpened)
                    }
                };
                Task::batch([left, window, self.resolve_account()])
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
            // The console opens at once after a sign-in, and the account
            // step follows when the node says the key holds no account: a
            // slow node never holds the console back, and a key that has an
            // account (the usual Unlock) never sees a "checking…" screen.
            // After a new key the answer landed during the phrase check, so
            // the step follows the check with no console in between.
            Message::AccountResolved { node, key, account } => {
                if node == self.connected_rpc && key == self.signer_key {
                    self.account_step |=
                        offers_account_step(std::mem::take(&mut self.account_offer), &account);
                    self.account = Some(account);
                }
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
                self.status = "Not connected".into();
                self.screen = Screen::Connect;
                self.push_props();
                self.leave_network()
            }
            Message::ToggleNetworkMenu => {
                self.network_menu = !self.network_menu;
                Task::none()
            }
            Message::CloseNetworkMenu => {
                self.network_menu = false;
                Task::none()
            }
            // The console stays on the network in hand while the other is
            // reached: the rail reads "Reaching …", and a node that does not
            // answer leaves everything as it was (`ConnectFailed`).
            Message::SwitchNetwork(origin) => {
                self.network_menu = false;
                if origin == self.connected_rpc && !self.connecting {
                    return Task::none();
                }
                self.update(Message::ConnectTo(origin))
            }
            Message::SplitView(_)
            | Message::ClosePane(_)
            | Message::FocusPane(_)
            | Message::PopOut(_)
            | Message::PopIn(_) => Task::none(),
            Message::SelectView(module) => {
                self.active = Some(module);
                self.badges.remove(module);
                Task::none()
            }
            Message::ViewEvent(module, event) => {
                match event.kind.as_str() {
                    "badge" => {
                        let count = crate::runtime::event_int(&event, "count");
                        match count > 0 {
                            true => {
                                self.badges.insert(module, count);
                            }
                            false => {
                                self.badges.remove(module);
                            }
                        }
                    }
                    "open_link" => {
                        let link = crate::runtime::event_text(&event, "link");
                        return self.update(Message::OpenLink(link));
                    }
                    _ => {}
                }
                Task::none()
            }
            Message::OpenLink(link) => {
                // `duck://<chain>/<program>/<tail>`: the program's view is the
                // one that reads the tail. The app opens the seat; the rest
                // is the view's, once it asks (a link door is not built yet).
                if let Some(module) = crate::runtime::local_link(&link) {
                    self.active = Some(module);
                    self.toast = format!("Opened {module}");
                    self.toast_age = 0;
                    return Task::none();
                }
                match ducklink::Link::parse(&link) {
                    Ok(parsed) => {
                        let module = crate::runtime::intern(&parsed.program);
                        self.active = Some(module);
                        self.toast = format!("Opened {}", parsed.program);
                        self.toast_age = 0;
                    }
                    Err(_) if link.starts_with("http://") || link.starts_with("https://") => {
                        crate::shell::open_link(link);
                    }
                    Err(_) => {
                        self.toast = "This link is not one this app opens.".into();
                        self.toast_age = 0;
                    }
                }
                Task::none()
            }
            Message::PasswordTyped(text) => {
                self.password = text;
                self.unlock_error.clear();
                Task::none()
            }
            Message::ConfirmPasswordTyped(text) => {
                self.confirm_password = text;
                self.unlock_error.clear();
                Task::none()
            }
            // Every submit below copies the typed secret instead of taking
            // it: the field on screen still shows what was typed, so a
            // failed try leaves model and field in agreement and a retry
            // sends what the person sees. Success, or leaving the screen,
            // wipes both (`forget_secrets`).
            Message::UnlockSubmit => {
                if self.unlock_busy {
                    return Task::none();
                }
                if self.password.is_empty() {
                    self.unlock_error = "Type this key's password first.".into();
                    return Task::none();
                }
                let password = zeroize::Zeroizing::new(self.password.clone());
                let keyring = self.keyring.clone();
                self.unlock_busy = true;
                self.unlock_error.clear();
                Task::future(async move {
                    let path = match backend::session_key_path(&keyring) {
                        Ok(path) => path,
                        Err(error) => return Message::UnlockFailed(backend::user_error(error)),
                    };
                    match backend::seat_signer(path, password).await {
                        Ok(pubkey) => Message::Unlocked(pubkey),
                        Err(error) => Message::UnlockFailed(backend::user_error(error)),
                    }
                })
            }
            // Minting only when there is no key yet, or the person has seen
            // what replacing it costs (`ShowNewKey`): this message never
            // doubles as "New key" on the Unlock screen.
            Message::CreateWalletSubmit => {
                if self.unlock_busy || (self.key_exists && !self.replacing) {
                    return Task::none();
                }
                if let Some(error) = weak_password(&self.password, &self.confirm_password) {
                    self.unlock_error = error;
                    return Task::none();
                }
                let password = zeroize::Zeroizing::new(self.password.clone());
                let keyring = self.keyring.clone();
                self.unlock_busy = true;
                self.unlock_error.clear();
                Task::future(async move {
                    let created = tokio::task::spawn_blocking({
                        let keyring = keyring.clone();
                        let password = password.clone();
                        move || backend::create_wallet(&keyring, &password)
                    })
                    .await
                    .unwrap_or_else(|_| Err("creating the wallet did not finish".into()));
                    let phrase = match created {
                        Ok(phrase) => phrase,
                        Err(error) => return Message::UnlockFailed(backend::user_error(error)),
                    };
                    let path = match backend::session_key_path(&keyring) {
                        Ok(path) => path,
                        Err(error) => return Message::UnlockFailed(backend::user_error(error)),
                    };
                    match backend::seat_signer(path, password).await {
                        Ok(pubkey) => Message::WalletCreated { pubkey, phrase },
                        Err(error) => Message::UnlockFailed(backend::user_error(error)),
                    }
                })
            }
            Message::WalletCreated { pubkey, phrase } => {
                self.unlock_busy = false;
                self.signer_key = pubkey;
                self.phrase = phrase;
                self.key_exists = true;
                self.replacing = false;
                self.account_offer = true;
                self.forget_secrets();
                self.push_props();
                self.resolve_account()
            }
            Message::PhraseWrittenDown => {
                self.phrase_quiz = Some(quiz_positions(self.phrase.split_whitespace().count()));
                self.unlock_error.clear();
                Task::none()
            }
            Message::PhraseWordTyped(nth, text) => {
                if let Some(answer) = self.quiz_answers.get_mut(nth) {
                    answer.zeroize();
                    *answer = text;
                }
                self.unlock_error.clear();
                Task::none()
            }
            Message::PhraseCheckSubmit => {
                let Some(asked) = self.phrase_quiz else {
                    return Task::none();
                };
                match quiz_matches(&self.phrase, asked, &self.quiz_answers) {
                    true => self.forget_phrase(),
                    false => {
                        self.unlock_error =
                            "Those words don't match your phrase. Check them, or show the phrase again."
                                .into();
                    }
                }
                Task::none()
            }
            Message::PhraseShowAgain => {
                self.phrase_quiz = None;
                self.quiz_answers.iter_mut().for_each(Zeroize::zeroize);
                self.unlock_error.clear();
                Task::none()
            }
            Message::ShowNewKey => {
                self.replacing = true;
                self.forget_secrets();
                self.unlock_error.clear();
                Task::none()
            }
            Message::NewKeyCancel => {
                self.replacing = false;
                self.forget_secrets();
                self.unlock_error.clear();
                Task::none()
            }
            Message::BrowseWithoutKey => {
                self.browsing = true;
                self.forget_secrets();
                self.unlock_error.clear();
                Task::none()
            }
            Message::SignIn => {
                self.browsing = false;
                Task::none()
            }
            Message::ShowRestore => {
                self.restoring = true;
                self.forget_secrets();
                self.unlock_error.clear();
                Task::none()
            }
            Message::RestoreCancel => {
                self.restoring = false;
                self.forget_secrets();
                self.unlock_error.clear();
                Task::none()
            }
            Message::RestorePhraseTyped(text) => {
                self.restore_phrase = text;
                self.unlock_error.clear();
                Task::none()
            }
            Message::RestorePasswordTyped(text) => {
                self.restore_password = text;
                self.unlock_error.clear();
                Task::none()
            }
            Message::RestoreConfirmPasswordTyped(text) => {
                self.restore_confirm_password = text;
                self.unlock_error.clear();
                Task::none()
            }
            Message::RestoreSubmit => {
                if self.unlock_busy {
                    return Task::none();
                }
                let phrase = normalize_phrase(&self.restore_phrase);
                if keystore::userkey::seed_of_mnemonic(&phrase).is_err() {
                    self.unlock_error = "That phrase isn't valid — check each word.".into();
                    return Task::none();
                }
                if let Some(error) =
                    weak_password(&self.restore_password, &self.restore_confirm_password)
                {
                    self.unlock_error = error;
                    return Task::none();
                }
                let password = zeroize::Zeroizing::new(self.restore_password.clone());
                let keyring = self.keyring.clone();
                self.unlock_busy = true;
                self.unlock_error.clear();
                Task::future(async move {
                    let restored = tokio::task::spawn_blocking({
                        let keyring = keyring.clone();
                        let password = password.clone();
                        move || backend::restore_wallet(&keyring, &phrase, &password)
                    })
                    .await
                    .unwrap_or_else(|_| Err("restoring the key did not finish".into()));
                    if let Err(error) = restored {
                        return Message::UnlockFailed(backend::user_error(error));
                    }
                    let path = match backend::session_key_path(&keyring) {
                        Ok(path) => path,
                        Err(error) => return Message::UnlockFailed(backend::user_error(error)),
                    };
                    match backend::seat_signer(path, password).await {
                        Ok(pubkey) => Message::Restored(pubkey),
                        Err(error) => Message::UnlockFailed(backend::user_error(error)),
                    }
                })
            }
            Message::Restored(pubkey) => {
                self.unlock_busy = false;
                self.signer_key = pubkey;
                self.key_exists = true;
                self.restoring = false;
                self.account_offer = true;
                self.forget_secrets();
                self.push_props();
                self.resolve_account()
            }
            Message::ForgetEndpoint(url) => {
                backend::forget_endpoint(&url);
                self.recent_endpoints = backend::recent_endpoints();
                Task::none()
            }
            Message::Unlocked(pubkey) => {
                self.unlock_busy = false;
                self.signer_key = pubkey;
                self.account_offer = true;
                self.forget_secrets();
                self.push_props();
                self.resolve_account()
            }
            Message::AccountNameTyped(text) => {
                self.account_name = text;
                self.unlock_error.clear();
                Task::none()
            }
            Message::PasskeyCreateSubmit => self.passkey(true),
            Message::PasskeySignInSubmit => self.passkey(false),
            Message::PasskeyUsePhone => {
                self.passkey_phone
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                Task::none()
            }
            Message::PasskeyQr(url) => {
                self.passkey_qr = url;
                Task::none()
            }
            Message::PasskeyCancel => {
                self.passkey_task = None;
                self.unlock_busy = false;
                self.key_exists = backend::key_exists(&self.keyring);
                Task::future(async {
                    backend::lock_signer().await;
                    Message::ShowToast("Passkey step cancelled".into())
                })
            }
            Message::PasskeyDone(pubkey) => {
                self.passkey_task = None;
                self.unlock_busy = false;
                self.signer_key = pubkey;
                self.key_exists = true;
                self.account_name.clear();
                // the passkey made (or joined) the account already
                self.account_offer = false;
                self.forget_secrets();
                self.push_props();
                self.resolve_account()
            }
            Message::ShowCreateAccount => {
                self.account_step = true;
                self.unlock_error.clear();
                Task::none()
            }
            Message::CreateAccountLater => {
                self.account_step = false;
                self.unlock_error.clear();
                Task::none()
            }
            Message::CreateAccountSubmit => {
                if self.unlock_busy {
                    return Task::none();
                }
                let name = self.account_name.trim().to_owned();
                if name.is_empty() {
                    self.unlock_error = "Type the name others will see first.".into();
                    return Task::none();
                }
                let client = backend::RpcClient::new(self.connected_rpc.clone());
                let network = self.network.clone();
                self.unlock_busy = true;
                self.unlock_error.clear();
                Task::future(async move {
                    Message::AccountCreated(
                        backend::passkey::create_plain_account(&client, &network, &name).await,
                    )
                })
            }
            Message::AccountCreated(created) => {
                self.unlock_busy = false;
                match created {
                    Ok(account) => {
                        self.account = Some(Some(account));
                        self.account_step = false;
                        self.account_name.clear();
                    }
                    Err(error) => self.unlock_error = account_error(&self.network, error),
                }
                Task::none()
            }
            Message::UnlockFailed(error) => {
                self.passkey_task = None;
                self.key_exists = backend::key_exists(&self.keyring);
                self.unlock_busy = false;
                self.unlock_error = error;
                Task::none()
            }
            Message::Lock => {
                self.signer_key.clear();
                self.account = None;
                self.account_offer = false;
                self.account_step = false;
                self.browsing = false;
                self.push_props();
                Task::future(async {
                    backend::lock_signer().await;
                    Message::ShowToast("Locked".into())
                })
            }
            Message::ShowToast(said) => {
                self.toast = said;
                self.toast_age = 0;
                Task::none()
            }
            Message::DismissToast => {
                self.toast.clear();
                Task::none()
            }
            Message::ToastTick => {
                self.toast_age += 1;
                if self.toast_age > 12 {
                    self.toast.clear();
                }
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
            Message::WallTick => {
                self.wall_now += 1;
                Task::none()
            }
            Message::ConsoleOpened(key) => {
                self.console_win = Some(key);
                Task::none()
            }
            Message::WindowWasClosed(key) => {
                if self.console_win == Some(key) {
                    self.console_win = None;
                }
                if self.focused_win == Some(key) {
                    self.focused_win = None;
                }
                Task::none()
            }
            Message::WindowFocused(key) => {
                self.focused_win = Some(key);
                Task::none()
            }
            Message::WindowUnfocused(key) => {
                if self.focused_win == Some(key) {
                    self.focused_win = None;
                }
                Task::none()
            }
            Message::ModifierStateChanged(modifiers) => {
                self.cmd_held = backend::command_held(modifiers);
                Task::none()
            }
            Message::TrayOpen => match self.console_win {
                Some(key) => crate::shell::raise(key),
                None => {
                    let (key, opened) = crate::shell::open(crate::shell::WindowKind::Console);
                    self.console_win = Some(key);
                    opened.map(Message::ConsoleOpened)
                }
            },
            Message::TrayQuit => crate::shell::quit(),
        }
    }

    fn apply_status(&mut self, status: &backend::NodeStatus) {
        self.height = i64::try_from(status.height).unwrap_or(-1);
        // mid-switch the line reads "Reaching …" until the other node answers
        if !self.connecting {
            self.status = format!("Connected · block {}", status.height);
        }
    }

    /// The network `keyring` names becomes the one in hand. Another chain
    /// than the last (a switch, not a second node of the same network):
    /// nothing of the last one — its seated key, account, open view —
    /// carries over.
    fn take_up(&mut self, keyring: backend::Keyring) -> Task<Message> {
        let left = match keyring.dir != self.keyring {
            true => self.leave_network(),
            false => Task::none(),
        };
        self.keyring = keyring.dir;
        self.other_chain = keyring.other_chain;
        self.key_exists = backend::key_exists(&self.keyring);
        left
    }

    /// Everything that belonged to the network being left: its seated key
    /// (the seat is one for the whole app), the account it resolved to, the
    /// open view and its badges, and any sign-in half done.
    fn leave_network(&mut self) -> Task<Message> {
        self.account = None;
        self.account_offer = false;
        self.account_step = false;
        self.active = None;
        self.badges.clear();
        self.network_menu = false;
        self.browsing = false;
        self.forget_secrets();
        self.forget_phrase();
        self.unlock_error.clear();
        self.keyring.clear();
        self.other_chain = false;
        self.key_exists = false;
        self.restoring = false;
        self.replacing = false;
        self.account_name.clear();
        self.passkey_task = None;
        self.unlock_busy = false;
        self.signer_key.clear();
        self.push_props();
        Task::future(async {
            backend::lock_signer().await;
        })
        .discard()
    }

    /// Asks the node which account the seated key belongs to; the answer
    /// lands as [`Message::AccountResolved`]. A failed ask keeps what the
    /// rail already shows — the next block asks again.
    fn resolve_account(&self) -> Task<Message> {
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

    /// Wipes every password and typed phrase the sign-in screens hold. The
    /// fields on screen mirror these (`DesktopWindow::input`), so they empty
    /// on the next draw too.
    fn forget_secrets(&mut self) {
        for secret in [
            &mut self.password,
            &mut self.confirm_password,
            &mut self.restore_phrase,
            &mut self.restore_password,
            &mut self.restore_confirm_password,
        ] {
            secret.zeroize();
        }
    }

    /// The recovery phrase and its check, gone once passed (or abandoned).
    fn forget_phrase(&mut self) {
        self.phrase.zeroize();
        self.phrase_quiz = None;
        self.quiz_answers.iter_mut().for_each(Zeroize::zeroize);
        self.unlock_error.clear();
    }

    /// Every seated view is handed the props again; the shell reads them
    /// off the state on its next draw.
    fn push_props(&mut self) {}

    /// What runs while the app does: the clocks, and nothing else.
    pub(crate) fn subscriptions(&self) -> Subscription<Message> {
        let mut recipes = vec![
            Subscription::run(wall_ticks),
            Subscription::run(toast_ticks),
        ];
        if self.connected {
            recipes.push(Subscription::run(status_ticks));
        }
        Subscription::batch(recipes)
    }
}

fn wall_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(std::time::Duration::from_secs(1)).map(|()| Message::WallTick)
}

fn toast_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(std::time::Duration::from_millis(300)).map(|()| Message::ToastTick)
}

fn status_ticks() -> impl futures::Stream<Item = Message> {
    crate::shell::every(STATUS_EVERY).map(|()| Message::Tick)
}

use futures::StreamExt as _;
use zeroize::Zeroize;

/// The one password rule enforced before minting or restoring a key: long
/// enough (the keystore's own floor), and its confirmation matches.
impl Ducktape {
    /// A passkey creates an account (`create`) or admits this device into
    /// one. Either way this device's key signs the writes: an existing key
    /// is unlocked with the password, else one is minted under it. The
    /// minted key's recovery phrase is not shown — the passkey admits a
    /// fresh device key whenever this one is lost.
    fn passkey(&mut self, create: bool) -> Task<Message> {
        if self.unlock_busy {
            return Task::none();
        }
        let name = self.account_name.trim().to_owned();
        if create && name.is_empty() {
            self.unlock_error = "Name your account first.".into();
            return Task::none();
        }
        let mint = !self.key_exists;
        if mint && let Some(error) = weak_password(&self.password, &self.confirm_password) {
            self.unlock_error = error;
            return Task::none();
        }
        let password = zeroize::Zeroizing::new(self.password.clone());
        let network = self.network.clone();
        let keyring = self.keyring.clone();
        let client = backend::RpcClient::new(self.connected_rpc.clone());
        self.unlock_busy = true;
        self.unlock_error.clear();
        self.passkey_phone = Default::default();
        self.passkey_qr.clear();
        let (phone, urls) = backend::passkey::Phone::new(self.passkey_phone.clone());
        let flow = async move {
            if mint {
                let minted = tokio::task::spawn_blocking({
                    let (keyring, password) = (keyring.clone(), password.clone());
                    move || backend::create_wallet(&keyring, &password)
                })
                .await
                .unwrap_or_else(|_| Err("creating this device's key did not finish".into()));
                if let Err(error) = minted {
                    return Message::UnlockFailed(backend::user_error(error));
                }
            }
            let seated = match backend::session_key_path(&keyring) {
                Ok(path) => backend::seat_signer(path, password).await,
                Err(error) => Err(error),
            };
            let pubkey = match seated {
                Ok(pubkey) => pubkey,
                Err(error) => return Message::UnlockFailed(backend::user_error(error)),
            };
            let joined = match create {
                true => backend::passkey::create_account(&client, &network, &name, &phone).await,
                false => backend::passkey::sign_in(&client, &network, &phone).await,
            };
            match joined {
                Ok(()) => Message::PasskeyDone(pubkey),
                Err(error) => {
                    backend::lock_signer().await;
                    Message::UnlockFailed(error)
                }
            }
        };
        // the flow owns the only URL sender: the QR updates end with it
        let (task, handle) = Task::stream(futures::stream::select(
            urls.map(Message::PasskeyQr),
            futures::stream::once(flow),
        ))
        .abortable();
        self.passkey_task = Some(handle.abort_on_drop());
        task
    }
}

/// Whether an answer about the seated key opens the account step: only the
/// first one after a sign-in that armed the offer, and only when the key
/// holds no account on this network.
fn offers_account_step(armed: bool, account: &Option<(u64, String)>) -> bool {
    armed && account.is_none()
}

/// A failed `Create` in plain words: a node that could not be reached
/// reads as that, not as a transport string; a refusal names the network
/// and says why.
fn account_error(network: &str, error: String) -> String {
    if error.contains("error sending request") {
        return format!("Can't reach {network}'s node right now. Try again in a moment.");
    }
    format!("{network} didn't create the account: {error}")
}

fn weak_password(password: &str, confirm: &str) -> Option<String> {
    if keystore::userkey::check_password_len(password).is_err() {
        return Some(format!(
            "The password needs at least {} characters.",
            keystore::userkey::MIN_PASSWORD_LEN
        ));
    }
    (password != confirm).then(|| "The passwords don't match.".to_string())
}

/// Three distinct word positions (0-based, ascending) out of `words` to ask
/// back — the old app's "Words 5, 12 and 20" check.
fn quiz_positions(words: usize) -> [usize; 3] {
    let mut picked = rand::seq::index::sample(&mut rand::thread_rng(), words.max(3), 3).into_vec();
    picked.sort_unstable();
    [picked[0], picked[1], picked[2]]
}

/// Whether each answer is the phrase's word at the position asked,
/// ignoring case and surrounding space.
fn quiz_matches(phrase: &str, asked: [usize; 3], answers: &[String; 3]) -> bool {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    asked.iter().zip(answers).all(|(&nth, answer)| {
        words
            .get(nth)
            .is_some_and(|word| word.eq_ignore_ascii_case(answer.trim()))
    })
}

/// Whatever a person pastes for a recovery phrase — any run of whitespace
/// between words, any case — folded to what BIP39 checks.
fn normalize_phrase(raw: &str) -> String {
    raw.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_phrase_folds_whitespace_and_case() {
        assert_eq!(
            normalize_phrase("  Canoe\n Pond\tFOREST  "),
            "canoe pond forest"
        );
    }

    #[test]
    fn two_missed_polls_read_reconnecting_and_one_answer_recovers() {
        let (mut state, _) = Ducktape::boot();
        state.connected = true;
        state.status = "Connected · block 7".into();
        let _ = state.update(Message::StatusMissed);
        assert!(!state.reconnecting(), "one miss is a hiccup");
        assert_eq!(state.status, "Connected · block 7");
        let _ = state.update(Message::StatusMissed);
        assert!(state.reconnecting());
        assert_eq!(state.status, "Reconnecting…");
        assert!(state.connected, "the poll keeps running");
        // same height: no block moved, so nothing else is asked for
        state.height = 9;
        let _ = state.update(Message::StatusPushed(backend::NodeStatus {
            network: "testkit".into(),
            time: 0,
            block_time_ms: 0,
            epoch_length: 0,
            height: 9,
            tip: [0; 32],
            root: abi::Root([0; 32]),
            epoch: 0,
            identity: Vec::new(),
            contract: backend::noded::NODE_CONTRACT,
        }));
        assert!(!state.reconnecting());
        assert_eq!(state.status, "Connected · block 9");
    }

    #[test]
    fn the_quiz_takes_the_asked_words_in_any_case() {
        let phrase = (1..=24)
            .map(|n| format!("w{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let answers = |a: &str, b: &str, c: &str| [a.to_string(), b.to_string(), c.to_string()];
        assert!(quiz_matches(
            &phrase,
            [4, 11, 19],
            &answers("w5", " W12 ", "w20")
        ));
        assert!(!quiz_matches(
            &phrase,
            [4, 11, 19],
            &answers("w5", "w12", "w21")
        ));
        assert!(!quiz_matches(&phrase, [4, 11, 19], &answers("", "", "")));
        let asked = quiz_positions(24);
        assert!(asked[0] < asked[1] && asked[1] < asked[2] && asked[2] < 24);
    }

    fn signing_in() -> Ducktape {
        let (mut state, _) = Ducktape::boot();
        state.screen = Screen::Console;
        state
    }

    #[test]
    fn a_new_try_clears_the_last_connect_failure() {
        let mut state = signing_in();
        state.error = "Can't reach http://a.".into();
        let _ = state.update(Message::EndpointTyped("not a url at all".into()));
        assert!(state.error.is_empty());
        state.error = "Can't reach http://a.".into();
        let _ = state.update(Message::ConnectSubmit);
        assert!(state.error.is_empty(), "the stale failure hid the refusal");
        assert_eq!(state.endpoint_error, backend::ENDPOINT_REFUSAL);
    }

    #[test]
    fn a_failed_unlock_keeps_the_typed_password_and_success_wipes_it() {
        let mut state = signing_in();
        state.key_exists = true;
        let _ = state.update(Message::UnlockSubmit);
        assert!(!state.unlock_busy, "an empty password is not sent");
        let _ = state.update(Message::PasswordTyped("hunter22".into()));
        let _ = state.update(Message::UnlockSubmit);
        assert_eq!(state.password, "hunter22");
        let _ = state.update(Message::UnlockFailed("wrong".into()));
        assert_eq!(
            state.password, "hunter22",
            "a retry must send what the field shows"
        );
        let _ = state.update(Message::Unlocked("ab".into()));
        assert!(state.password.is_empty());
    }

    #[test]
    fn create_never_stands_in_for_new_key_without_its_confirm_step() {
        let mut state = signing_in();
        state.key_exists = true;
        state.password = "longenough1".into();
        state.confirm_password = "longenough1".into();
        let _ = state.update(Message::CreateWalletSubmit);
        assert!(
            !state.unlock_busy,
            "minted over an existing key without asking"
        );
        let _ = state.update(Message::ShowNewKey);
        assert!(state.replacing && state.password.is_empty());
        let _ = state.update(Message::NewKeyCancel);
        assert!(!state.replacing);
    }

    #[test]
    fn switching_node_leaves_no_key_seated() {
        let mut state = signing_in();
        state.signer_key = "ab".into();
        state.password = "hunter22".into();
        let _ = state.update(Message::Disconnect);
        assert!(state.signer_key.is_empty() && state.password.is_empty());
    }

    fn resolved(state: &mut Ducktape, account: Option<(u64, String)>) {
        let key = state.signer_key.clone();
        let node = state.connected_rpc.clone();
        let _ = state.update(Message::AccountResolved { node, key, account });
    }

    #[test]
    fn a_sign_in_without_an_account_opens_the_account_step_once() {
        for signed_in in [
            Message::Unlocked("ab".into()),
            Message::Restored("ab".into()),
            Message::WalletCreated {
                pubkey: "ab".into(),
                phrase: "w1 w2 w3".into(),
            },
        ] {
            let mut state = signing_in();
            let _ = state.update(signed_in);
            assert!(!state.account_step, "shown before the node answered");
            resolved(&mut state, None);
            assert!(state.account_step);
            let _ = state.update(Message::CreateAccountLater);
            resolved(&mut state, None);
            assert!(!state.account_step, "a later block reopened it");
        }
    }

    #[test]
    fn the_account_step_is_skipped_for_a_key_with_an_account_or_a_passkey() {
        let mut state = signing_in();
        let _ = state.update(Message::Unlocked("ab".into()));
        resolved(&mut state, Some((7, "ada".into())));
        assert!(!state.account_step);
        let mut state = signing_in();
        let _ = state.update(Message::PasskeyDone("ab".into()));
        resolved(&mut state, None);
        assert!(!state.account_step, "the passkey made the account");
        // another key's answer neither opens nor disarms it
        let mut state = signing_in();
        let _ = state.update(Message::Unlocked("ab".into()));
        let _ = state.update(Message::AccountResolved {
            node: state.connected_rpc.clone(),
            key: "cd".into(),
            account: None,
        });
        resolved(&mut state, None);
        assert!(state.account_step);
    }

    #[test]
    fn the_rail_reopens_the_step_and_a_created_account_closes_it() {
        let mut state = signing_in();
        state.signer_key = "ab".into();
        let _ = state.update(Message::ShowCreateAccount);
        assert!(state.account_step);
        let _ = state.update(Message::CreateAccountSubmit);
        assert!(!state.unlock_busy, "an empty name is not sent");
        assert!(!state.unlock_error.is_empty());
        let _ = state.update(Message::AccountCreated(Err("refused".into())));
        assert!(state.account_step && state.unlock_error.contains("refused"));
        let _ = state.update(Message::AccountCreated(Err(
            "error sending request for url (http://127.0.0.1:1/)".into(),
        )));
        assert!(!state.unlock_error.contains("url"), "a transport string");
        let _ = state.update(Message::AccountCreated(Ok((9, "ada".into()))));
        assert!(!state.account_step);
        assert_eq!(state.account, Some(Some((9, "ada".into()))));
    }

    fn on_testkit() -> Ducktape {
        let mut state = signing_in();
        state.connected = true;
        state.connected_rpc = "http://a".into();
        state.network = "testkit".into();
        state.keyring = "testkit".into();
        state.height = 7;
        state.status = "Connected · block 7".into();
        state.signer_key = "ab".into();
        state.account = Some(Some((7, "Grace Hopper".into())));
        state.active = Some("chat");
        state.badges.insert("chat", 3);
        state
    }

    #[test]
    fn a_switch_keeps_the_network_in_hand_until_the_other_answers() {
        let mut state = on_testkit();
        let _ = state.update(Message::ToggleNetworkMenu);
        assert!(state.network_menu);
        let _ = state.update(Message::SwitchNetwork("http://b".into()));
        assert!(!state.network_menu && state.connecting);
        assert_eq!(state.status, "Reaching http://b…");
        assert_eq!(state.signer_key, "ab", "still signed in while reaching");
        // A's poll lands mid-switch: the rail keeps saying where it is going
        let _ = state.update(Message::StatusPushed(status(8)));
        assert_eq!(state.status, "Reaching http://b…");
        let _ = state.update(Message::ConnectFailed {
            generation: state.connect_generation,
            error: "error sending request for url (http://b/v1/status)".into(),
        });
        assert!(state.connected && !state.connecting);
        assert_eq!(state.status, "Connected · block 8");
        assert_eq!(state.endpoint, "http://a");
        assert!(state.error.is_empty() && state.toast.contains("http://b"));
        assert_eq!(state.account, Some(Some((7, "Grace Hopper".into()))));
    }

    #[test]
    fn taking_up_another_chain_resets_the_last_ones_state() {
        let mut state = on_testkit();
        let _ = state.take_up(backend::Keyring {
            dir: "testkit".into(),
            other_chain: false,
        });
        assert_eq!(
            state.signer_key, "ab",
            "a second node of one network keeps the key"
        );
        assert_eq!(state.active, Some("chat"));
        let _ = state.take_up(backend::Keyring {
            dir: "testkit+200".into(),
            other_chain: true,
        });
        assert!(
            state.signer_key.is_empty(),
            "the seat is locked on a switch"
        );
        assert_eq!(state.account, None, "the account is asked again");
        assert_eq!(state.active, None);
        assert!(state.badges.is_empty());
        assert!(state.other_chain);
        assert_eq!(state.keyring, "testkit+200");
    }

    #[test]
    fn an_account_answer_from_before_a_switch_is_dropped() {
        let mut state = on_testkit();
        state.account = None;
        let _ = state.update(Message::AccountResolved {
            node: "http://old".into(),
            key: "ab".into(),
            account: Some((1, "Stale".into())),
        });
        assert_eq!(state.account, None);
        let _ = state.update(Message::AccountResolved {
            node: "http://a".into(),
            key: "ab".into(),
            account: None,
        });
        assert_eq!(state.account, Some(None));
    }

    fn status(height: u64) -> backend::NodeStatus {
        backend::NodeStatus {
            network: "testkit".into(),
            time: 0,
            block_time_ms: 0,
            epoch_length: 0,
            height,
            tip: [0; 32],
            root: abi::Root([0; 32]),
            epoch: 0,
            identity: Vec::new(),
            contract: backend::noded::NODE_CONTRACT,
        }
    }

    #[test]
    fn weak_password_catches_short_and_mismatched() {
        assert!(weak_password("short", "short").is_some());
        assert!(weak_password("longenough1", "different").is_some());
        assert!(weak_password("longenough1", "longenough1").is_none());
    }
}
