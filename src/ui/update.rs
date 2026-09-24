//! The reducer: one message in, the state moved, a task out.

use super::{AppMessage as Message, Appearance, Ducktape, Screen, Spot, SpotRow};
use crate::backend;
use crate::runtime::Intent;
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
                // the chain id links name: the network and its genesis salt
                self.chain = ducklink::ChainId::of(&status.network, &status.genesis)
                    .map_or_else(|| status.network.clone(), |chain| chain.to_string());
                crate::runtime::notify::center().set_network(&self.chain);
                self.connected = true;
                self.status_misses = 0;
                self.screen = Screen::Console;
                self.apply_status(&status);
                drop(crate::runtime::connected(&client, &self.network));
                let window = match self.console_win {
                    Some(key) => crate::shell::raise(key),
                    None => {
                        let (key, opened) = crate::shell::open(crate::shell::WindowKind::Console);
                        self.console_win = Some(key);
                        opened.map(Message::ConsoleOpened)
                    }
                };
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
            // The console opens at once after a sign-in, and the account
            // step follows when the node says the key holds no account: a
            // slow node never holds the console back, and a key that has an
            // account (the usual Unlock) never sees a "checking…" screen.
            // After a new key the answer landed during the phrase check, so
            // the step follows the check with no console in between.
            Message::AccountResolved { node, key, account } => {
                if node == self.connected_rpc && key == self.signer_key {
                    self.sign_in.account_step |= offers_account_step(
                        std::mem::take(&mut self.sign_in.account_offer),
                        &account,
                    );
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
                self.chain.clear();
                self.status = "Not connected".into();
                self.screen = Screen::Connect;
                self.leave_network()
            }
            Message::ToggleNetworkMenu => {
                self.popover = None;
                self.network_menu = !self.network_menu;
                Task::none()
            }
            Message::CloseNetworkMenu => {
                self.network_menu = false;
                Task::none()
            }
            // The console stays on the network in hand while the other is
            // reached: the status reads "Reaching …", and a node that does not
            // answer leaves everything as it was (`ConnectFailed`).
            Message::SwitchNetwork(origin) => {
                self.network_menu = false;
                if origin == self.connected_rpc && !self.connecting {
                    return Task::none();
                }
                self.update(Message::ConnectTo(origin))
            }
            Message::TogglePopover(which) => {
                self.network_menu = false;
                self.popover = match self.popover == Some(which) {
                    true => None,
                    false => Some(which),
                };
                Task::none()
            }
            Message::ClosePopover => {
                self.popover = None;
                Task::none()
            }
            Message::OpenSpotlight => {
                self.popover = None;
                self.network_menu = false;
                self.spotlight = true;
                self.spotlight_query.clear();
                self.spotlight_pick = 0;
                Task::none()
            }
            Message::CloseSpotlight => {
                self.spotlight = false;
                self.spotlight_query.clear();
                Task::none()
            }
            Message::SpotlightTyped(text) => {
                self.spotlight_query = text;
                self.spotlight_pick = 0;
                Task::none()
            }
            Message::SpotlightMove { down, rows } => {
                self.spotlight_pick = match (down, rows) {
                    (_, 0) => 0,
                    (true, rows) => (self.spotlight_pick + 1).min(rows - 1),
                    (false, _) => self.spotlight_pick.saturating_sub(1),
                };
                Task::none()
            }
            Message::SpotlightSubmit => {
                let rows = self.spotlight_rows();
                match rows.into_iter().nth(self.spotlight_pick) {
                    Some(row) => self.update(Message::Spot(row.spot)),
                    None => Task::none(),
                }
            }
            Message::Spot(spot) => {
                self.spotlight = false;
                self.spotlight_query.clear();
                let message = match spot {
                    Spot::Open(module) => Message::SelectView(module),
                    Spot::Switch(url) => Message::SwitchNetwork(url),
                    Spot::Settings => Message::OpenSettings,
                    Spot::CreateAccount => Message::ShowCreateAccount,
                    Spot::Lock => Message::Lock,
                    Spot::Appearance(mode) => Message::SetAppearance(mode),
                    Spot::OtherNetwork => Message::Disconnect,
                };
                self.update(message)
            }
            Message::OpenSettings => {
                self.popover = None;
                self.spotlight = false;
                self.network_menu = false;
                self.settings = true;
                Task::none()
            }
            Message::CloseSettings => {
                self.settings = false;
                Task::none()
            }
            Message::ShowSettingsPage(page) => {
                self.settings_page = page;
                Task::none()
            }
            Message::SetMotion(on) => {
                self.motion = on;
                backend::save_motion(on);
                Task::none()
            }
            Message::SelectView(module) => {
                self.active = Some(module);
                self.badges.remove(module);
                Task::none()
            }
            Message::ViewEvent(module, intent) => match intent {
                Intent::Badge(count) if count > 0 => {
                    self.badges.insert(module, count);
                    Task::none()
                }
                Intent::Badge(_) => {
                    self.badges.remove(module);
                    Task::none()
                }
                Intent::OpenLink(link) => self.update(Message::OpenLink(link)),
                // the dispatch redraws
                Intent::Notified => Task::none(),
            },
            Message::NotifyOpen(id) => {
                self.popover = None;
                let entry = crate::runtime::notify::center().open(id);
                match entry {
                    Some(entry) if !entry.link.is_empty() => {
                        return self.update(Message::OpenLink(entry.link));
                    }
                    Some(entry) if crate::runtime::listed_view(&entry.module) => {
                        self.open_seat(crate::runtime::intern(&entry.module), None);
                    }
                    _ => {}
                }
                Task::none()
            }
            Message::NotifyMarkAllRead => {
                crate::runtime::notify::center().mark_all_read();
                Task::none()
            }
            Message::NotifyClearRead => {
                crate::runtime::notify::center().clear_read();
                Task::none()
            }
            Message::NotifySettings => {
                self.settings_page = super::SettingsPage::Notifications;
                self.update(Message::OpenSettings)
            }
            Message::NotifyPermission(module, permission) => {
                crate::runtime::notify::set_permission(module, permission);
                Task::none()
            }
            Message::NotifyNotNow(module) => {
                crate::runtime::notify::not_now(module);
                Task::none()
            }
            Message::SetNotifyBanners(on) => {
                crate::runtime::notify::save_banners(on);
                Task::none()
            }
            Message::SetNotifyInFront(show) => {
                crate::runtime::notify::save_in_front(show);
                Task::none()
            }
            Message::SetNotifyBurst(burst) => {
                crate::runtime::notify::save_burst(burst);
                Task::none()
            }
            Message::OpenLink(link) => {
                // `duck://<chain>/<program>/<tail>` on this chain, or the short
                // `duck://<view>/<route>`: the seat opens and its view is
                // handed the route. The window coming forward is the answer;
                // a notice only says what there is nothing to see of.
                use crate::runtime::Link;
                match crate::runtime::parse_link(&link) {
                    Link::View { module, route } => self.open_seat(module, route),
                    Link::Chain(parsed) if !crate::runtime::listed_view(&parsed.program) => {
                        self.notice(format!("No view here opens {} links.", parsed.program));
                    }
                    Link::Chain(parsed) => {
                        let module = crate::runtime::intern(&parsed.program);
                        let route = parsed.tail.join("/");
                        if parsed.chain.to_string() != self.chain {
                            // another chain's page is not this chain's to show
                            self.open_seat(module, None);
                            self.notice(format!(
                                "That link is to {}, not this network; opened {module}.",
                                parsed.chain.label
                            ));
                        } else if route.is_empty() || crate::runtime::valid_route(&route) {
                            self.open_seat(module, (!route.is_empty()).then_some(route));
                        } else {
                            self.open_seat(module, None);
                            self.notice(format!("{module} can't open that part of the link."));
                        }
                    }
                    Link::Web(url) => return crate::shell::open_url(url),
                    Link::Unknown => self.notice("This link is not one this app opens.".into()),
                }
                Task::none()
            }
            Message::PasswordTyped(text) => {
                self.sign_in.password = text;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            // With no password typed, Unlock reopens this device's OS-kept
            // key (after a Lock). With one, it opens a password-locked key
            // from before keys moved into the OS, and moves it there: the
            // password is asked this once. The typed password is copied, not
            // taken: a failed try leaves model and field in agreement.
            Message::UnlockSubmit => {
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                self.sign_in.locked = false;
                self.sign_in.unlock_error.clear();
                if !self.key_exists {
                    return self.open_device_key();
                }
                if self.sign_in.password.is_empty() {
                    self.sign_in.unlock_error = "Type this key's password first.".into();
                    return Task::none();
                }
                let password = zeroize::Zeroizing::new(self.sign_in.password.clone());
                let keyring = self.keyring.clone();
                self.sign_in.unlock_busy = true;
                Task::future(async move {
                    let opened = tokio::task::spawn_blocking(move || {
                        let path = backend::session_key_path(&keyring)?;
                        let key = keystore::userkey::open_user_key_at(&path, &password)?;
                        // kept by the OS from now on; a refusal only means
                        // the password is asked again next time
                        if let Err(error) = backend::device_key::save(&keyring, &key) {
                            tracing::info!(target: "ducktape::keys", %error, "password-locked key not moved into the OS");
                        }
                        Ok::<_, String>(key)
                    })
                    .await
                    .unwrap_or_else(|_| Err("opening this device's key did not finish".into()));
                    match opened {
                        Ok(key) => Message::Unlocked(backend::seat_key(key).await),
                        Err(error) => Message::UnlockFailed(backend::user_error(error)),
                    }
                })
            }
            Message::DeviceKey(found) => {
                self.sign_in.seating = false;
                match found {
                    Ok(Some(pubkey)) => {
                        self.key_exists = false;
                        self.update(Message::Unlocked(pubkey))
                    }
                    // only a password-locked key here: the key screen asks
                    Ok(None) => Task::none(),
                    Err(error) => {
                        self.sign_in.unlock_error = error;
                        Task::none()
                    }
                }
            }
            Message::RecoveryKeyStart => {
                self.popover = None;
                self.forget_phrase();
                self.sign_in.phrase = backend::join::new_recovery_phrase();
                Task::none()
            }
            Message::PhraseCancel => {
                self.forget_phrase();
                Task::none()
            }
            Message::PhraseWrittenDown => {
                self.sign_in.phrase_quiz = Some(quiz_positions(
                    self.sign_in.phrase.split_whitespace().count(),
                ));
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::PhraseWordTyped(nth, text) => {
                if let Some(answer) = self.sign_in.quiz_answers.get_mut(nth) {
                    answer.zeroize();
                    *answer = text;
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            // The words checked: the key they make goes onto the account.
            Message::PhraseCheckSubmit => {
                let Some(asked) = self.sign_in.phrase_quiz else {
                    return Task::none();
                };
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                if !quiz_matches(&self.sign_in.phrase, asked, &self.sign_in.quiz_answers) {
                    self.sign_in.unlock_error =
                        "Those words don't match your phrase. Check them, or show the phrase again."
                            .into();
                    return Task::none();
                }
                let phrase = zeroize::Zeroizing::new(self.sign_in.phrase.clone());
                let client = backend::RpcClient::new(self.connected_rpc.clone());
                let network = self.network.clone();
                self.sign_in.unlock_busy = true;
                Task::future(async move {
                    Message::RecoveryKeyAdded(
                        backend::join::add_recovery_key(&client, &network, &phrase).await,
                    )
                })
            }
            Message::RecoveryKeyAdded(added) => {
                self.sign_in.unlock_busy = false;
                match added {
                    Ok(()) => {
                        self.forget_phrase();
                        self.update(Message::ShowToast(
                            "Recovery key added. Keep the paper somewhere safe.".into(),
                        ))
                    }
                    Err(error) => {
                        self.sign_in.unlock_error = error;
                        Task::none()
                    }
                }
            }
            Message::PhraseShowAgain => {
                self.sign_in.phrase_quiz = None;
                self.sign_in
                    .quiz_answers
                    .iter_mut()
                    .for_each(Zeroize::zeroize);
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::BrowseWithoutKey => {
                self.browsing = true;
                self.forget_secrets();
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::SignIn => {
                self.browsing = false;
                self.sign_in.locked = false;
                self.open_device_key()
            }
            Message::RestorePhraseTyped(text) => {
                self.sign_in.restore_phrase = text;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::RecoverShow => {
                self.sign_in.recovering = true;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::RecoverCancel => {
                self.sign_in.recovering = false;
                self.forget_secrets();
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::RecoverSubmit => {
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                let phrase =
                    zeroize::Zeroizing::new(normalize_phrase(&self.sign_in.restore_phrase));
                if keystore::userkey::seed_of_mnemonic(&phrase).is_err() {
                    self.sign_in.unlock_error =
                        "Those words aren't a recovery key — check each one.".into();
                    return Task::none();
                }
                let client = backend::RpcClient::new(self.connected_rpc.clone());
                let network = self.network.clone();
                self.sign_in.unlock_busy = true;
                self.sign_in.unlock_error.clear();
                Task::future(async move {
                    Message::Joined(
                        backend::join::join_with_recovery_key(&client, &network, &phrase).await,
                    )
                })
            }
            Message::LinkStart => {
                if self.sign_in.link_task.is_some() {
                    return Task::none();
                }
                let code = backend::join::new_code();
                self.sign_in.link_code = code.clone();
                self.sign_in.unlock_error.clear();
                let client = backend::RpcClient::new(self.connected_rpc.clone());
                let network = self.network.clone();
                let (task, handle) = Task::future(async move {
                    Message::Joined(backend::join::join_from_device(&client, &network, &code).await)
                })
                .abortable();
                self.sign_in.link_task = Some(handle.abort_on_drop());
                task
            }
            Message::LinkCancel => {
                self.sign_in.link_task = None;
                self.sign_in.link_code.clear();
                Task::none()
            }
            Message::Joined(joined) => {
                self.sign_in.unlock_busy = false;
                self.sign_in.link_task = None;
                self.sign_in.link_code.clear();
                match joined {
                    Ok(()) => {
                        self.sign_in.recovering = false;
                        self.forget_secrets();
                        self.sign_in.account_offer = false;
                        self.sign_in.account_step = false;
                        self.resolve_account()
                    }
                    Err(error) => {
                        self.sign_in.unlock_error = error;
                        Task::none()
                    }
                }
            }
            Message::ApproveOpen => {
                self.popover = None;
                self.approving = true;
                self.sign_in.approve_code.clear();
                self.sign_in.approve_found = None;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::ApproveClose => {
                self.approving = false;
                self.sign_in.approve_found = None;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::ApproveCodeTyped(text) => {
                self.sign_in.approve_code = text;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::ApproveFind => {
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                let code = self.sign_in.approve_code.clone();
                self.sign_in.unlock_busy = true;
                self.sign_in.unlock_error.clear();
                Task::future(async move {
                    Message::ApproveFound(backend::join::find_request(&code).await)
                })
            }
            Message::ApproveFound(found) => {
                self.sign_in.unlock_busy = false;
                match found {
                    Ok(request) => self.sign_in.approve_found = Some(request),
                    Err(error) => self.sign_in.unlock_error = error,
                }
                Task::none()
            }
            Message::ApproveConfirm => {
                let (Some(request), Some(Some((account, _)))) =
                    (self.sign_in.approve_found.clone(), self.account.clone())
                else {
                    return Task::none();
                };
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                let code = self.sign_in.approve_code.clone();
                let client = backend::RpcClient::new(self.connected_rpc.clone());
                let network = self.network.clone();
                self.sign_in.unlock_busy = true;
                Task::future(async move {
                    Message::ApproveDone(
                        backend::join::approve(&client, &network, account, &code, &request).await,
                    )
                })
            }
            Message::ApproveDone(done) => {
                self.sign_in.unlock_busy = false;
                match done {
                    Ok(()) => {
                        self.approving = false;
                        self.sign_in.approve_found = None;
                        self.update(Message::ShowToast(
                            "Approved. The new device finishes on its own.".into(),
                        ))
                    }
                    Err(error) => {
                        self.sign_in.unlock_error = error;
                        Task::none()
                    }
                }
            }
            Message::ForgetEndpoint(url) => {
                backend::forget_endpoint(&url);
                self.recent_endpoints = backend::recent_endpoints();
                Task::none()
            }
            Message::Unlocked(pubkey) => {
                self.sign_in.unlock_busy = false;
                self.key_exists = false;
                self.sign_in.seating = false;
                self.sign_in.locked = false;
                self.signer_key = pubkey;
                self.sign_in.account_offer = true;
                self.forget_secrets();
                self.resolve_account()
            }
            Message::AccountNameTyped(text) => {
                self.sign_in.account_name = text;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::PasskeyCreateSubmit => self.passkey(true),
            Message::PasskeySignInSubmit => self.passkey(false),
            Message::PasskeyUsePhone => {
                self.sign_in
                    .passkey_phone
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                Task::none()
            }
            Message::PasskeyQr(url) => {
                self.sign_in.passkey_qr = url;
                Task::none()
            }
            Message::PasskeyCancel => {
                self.sign_in.passkey_task = None;
                self.sign_in.unlock_busy = false;
                Task::done(Message::ShowToast("Passkey step cancelled".into()))
            }
            Message::PasskeyFailed(error) => {
                self.sign_in.passkey_task = None;
                self.sign_in.unlock_busy = false;
                self.sign_in.unlock_error = error;
                Task::none()
            }
            Message::PasskeyDone(pubkey) => {
                self.sign_in.passkey_task = None;
                self.sign_in.unlock_busy = false;
                if pubkey != self.signer_key {
                    return Task::none();
                }
                self.sign_in.account_name.clear();
                // the passkey made (or joined) the account already
                self.sign_in.account_offer = false;
                self.sign_in.account_step = false;
                self.resolve_account()
            }
            Message::ShowCreateAccount => {
                self.sign_in.account_step = true;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::CreateAccountLater => {
                self.sign_in.account_step = false;
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::CreateAccountSubmit => {
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                let name = self.sign_in.account_name.trim().to_owned();
                if name.is_empty() {
                    self.sign_in.unlock_error = "Type the name others will see first.".into();
                    return Task::none();
                }
                let client = backend::RpcClient::new(self.connected_rpc.clone());
                let network = self.network.clone();
                self.sign_in.unlock_busy = true;
                self.sign_in.unlock_error.clear();
                Task::future(async move {
                    Message::AccountCreated(
                        backend::passkey::create_plain_account(&client, &network, &name).await,
                    )
                })
            }
            Message::AccountCreated(created) => {
                self.sign_in.unlock_busy = false;
                match created {
                    Ok(account) => {
                        self.account = Some(Some(account));
                        self.sign_in.account_step = false;
                        self.sign_in.account_name.clear();
                    }
                    Err(error) => self.sign_in.unlock_error = account_error(&self.network, error),
                }
                Task::none()
            }
            Message::UnlockFailed(error) => {
                self.sign_in.passkey_task = None;
                self.key_exists = backend::key_exists(&self.keyring);
                self.sign_in.unlock_busy = false;
                self.sign_in.unlock_error = error;
                Task::none()
            }
            Message::Lock => {
                self.popover = None;
                self.sign_in.locked = true;
                self.signer_key.clear();
                self.account = None;
                self.sign_in.account_offer = false;
                self.sign_in.account_step = false;
                self.browsing = false;
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
                self.cmd_held = crate::runtime::command_held(modifiers);
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

    fn open_seat(&mut self, module: &'static str, route: Option<String>) {
        if let Some(route) = route {
            crate::runtime::route_to(module, route);
        }
        self.active = Some(module);
        self.reveal = true;
    }

    fn notice(&mut self, said: String) {
        self.toast = said;
        self.toast_age = 0;
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
        // dropping the old one wipes its secrets and cancels its tasks
        self.sign_in = Default::default();
        self.account = None;
        self.active = None;
        self.badges.clear();
        self.network_menu = false;
        self.popover = None;
        self.spotlight = false;
        self.node = None;
        self.browsing = false;
        self.keyring.clear();
        self.other_chain = false;
        self.key_exists = false;
        self.approving = false;
        self.signer_key.clear();
        Task::future(async {
            backend::lock_signer().await;
        })
        .discard()
    }

    /// Asks the node which account the seated key belongs to; the answer
    /// lands as [`Message::AccountResolved`]. A failed ask keeps what the
    /// menu bar already shows — the next block asks again.
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

    /// What ⌘K offers for the text typed, in the order shown: the
    /// network's programs, the other networks this device reached, then
    /// things to do. A row matches when its title or detail holds the text,
    /// ignoring case.
    pub(crate) fn spotlight_rows(&self) -> Vec<SpotRow> {
        let row = |group, title: String, meta: String, spot| SpotRow {
            group,
            title,
            meta,
            spot,
        };
        let mut rows: Vec<SpotRow> = crate::runtime::rail()
            .into_iter()
            .filter(|program| !program.empty)
            .map(|program| {
                row(
                    "Programs",
                    program.label.clone(),
                    "Open".into(),
                    Spot::Open(program.module),
                )
            })
            .collect();
        rows.extend(
            self.recent_endpoints
                .iter()
                .filter(|entry| entry.url != self.connected_rpc)
                .map(|entry| {
                    row(
                        "Networks",
                        entry.name(),
                        entry.host().to_owned(),
                        Spot::Switch(entry.url.clone()),
                    )
                }),
        );
        rows.push(row(
            "Actions",
            "Ducktape settings".into(),
            "theme, networks".into(),
            Spot::Settings,
        ));
        if matches!(self.account, Some(None)) && !self.signer_key.is_empty() {
            rows.push(row(
                "Actions",
                "Create account".into(),
                self.network.clone(),
                Spot::CreateAccount,
            ));
        }
        if !self.signer_key.is_empty() {
            rows.push(row(
                "Actions",
                "Lock".into(),
                "this device's key".into(),
                Spot::Lock,
            ));
        }
        for (title, mode) in [
            ("Light appearance", Appearance::Light),
            ("Dark appearance", Appearance::Dark),
            ("Match the system's appearance", Appearance::System),
        ] {
            rows.push(row(
                "Actions",
                title.into(),
                String::new(),
                Spot::Appearance(mode),
            ));
        }
        rows.push(row(
            "Actions",
            "Add a network…".into(),
            String::new(),
            Spot::OtherNetwork,
        ));
        let query = self.spotlight_query.trim().to_lowercase();
        rows.retain(|row| {
            query.is_empty()
                || row.title.to_lowercase().contains(&query)
                || row.meta.to_lowercase().contains(&query)
        });
        rows
    }

    /// Wipes every password and typed phrase the sign-in screens hold. The
    /// fields on screen mirror these (`DesktopWindow::input`), so they empty
    /// on the next draw too.
    fn forget_secrets(&mut self) {
        for secret in [&mut self.sign_in.password, &mut self.sign_in.restore_phrase] {
            secret.zeroize();
        }
    }

    /// Opens this device's key for the network in hand and seats it — made
    /// here the first time this device meets the network. Nothing to do when
    /// one is seated, or the person locked it; a password-locked key from
    /// before answers `Ok(None)` and waits for its password once.
    fn open_device_key(&mut self) -> Task<Message> {
        if self.sign_in.seating
            || self.sign_in.locked
            || !self.signer_key.is_empty()
            || self.keyring.is_empty()
        {
            return Task::none();
        }
        self.sign_in.seating = true;
        let keyring = self.keyring.clone();
        let legacy = self.key_exists;
        Task::future(async move {
            let opened =
                tokio::task::spawn_blocking(move || match backend::device_key::load(&keyring)? {
                    Some(key) => Ok(Some(key)),
                    None if legacy => Ok(None),
                    None => backend::device_key::mint(&keyring).map(Some),
                })
                .await
                .unwrap_or_else(|_| Err("opening this device's key did not finish".into()));
            Message::DeviceKey(match opened {
                Ok(Some(key)) => Ok(Some(backend::seat_key(key).await)),
                Ok(None) => Ok(None),
                Err(error) => Err(error),
            })
        })
    }

    /// The recovery phrase and its check, gone once passed (or abandoned).
    fn forget_phrase(&mut self) {
        self.sign_in.phrase.zeroize();
        self.sign_in.phrase_quiz = None;
        self.sign_in
            .quiz_answers
            .iter_mut()
            .for_each(Zeroize::zeroize);
        self.sign_in.unlock_error.clear();
    }

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
    /// A passkey creates an account (`create`) or admits this device's key
    /// into the account it holds. Both are about the ACCOUNT: the device
    /// key is already set up and seated (unlocked, or minted with its
    /// phrase) before either is offered, and it signs the writes.
    fn passkey(&mut self, create: bool) -> Task<Message> {
        if self.sign_in.unlock_busy {
            return Task::none();
        }
        if self.signer_key.is_empty() {
            self.sign_in.unlock_error = "Unlock this device's key first.".into();
            return Task::none();
        }
        let name = self.sign_in.account_name.trim().to_owned();
        if create && name.is_empty() {
            self.sign_in.unlock_error = "Name your account first.".into();
            return Task::none();
        }
        let network = self.network.clone();
        let client = backend::RpcClient::new(self.connected_rpc.clone());
        let pubkey = self.signer_key.clone();
        self.sign_in.unlock_busy = true;
        self.sign_in.unlock_error.clear();
        self.sign_in.passkey_phone = Default::default();
        self.sign_in.passkey_qr.clear();
        let (phone, urls) = backend::passkey::Phone::new(self.sign_in.passkey_phone.clone());
        let flow = async move {
            let joined = match create {
                true => backend::passkey::create_account(&client, &network, &name, &phone).await,
                false => backend::passkey::sign_in(&client, &network, &phone).await,
            };
            match joined {
                Ok(()) => Message::PasskeyDone(pubkey),
                Err(error) => Message::PasskeyFailed(error),
            }
        };
        // the flow owns the only URL sender: the QR updates end with it
        let (task, handle) = Task::stream(futures::stream::select(
            urls.map(Message::PasskeyQr),
            futures::stream::once(flow),
        ))
        .abortable();
        self.sign_in.passkey_task = Some(handle.abort_on_drop());
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
    fn a_link_on_this_chain_hands_its_view_the_route() {
        let (mut state, _) = Ducktape::boot();
        state.chain = "testkit#0a1b2c3d".into();
        crate::runtime::list_for_test("link-test-here");
        crate::runtime::list_for_test("link-test-away");
        let _ = state.update(Message::OpenLink(
            "duck://testkit-0a1b2c3d/link-test-here/tx/00ff".into(),
        ));
        assert_eq!(state.active, Some("link-test-here"));
        assert!(state.toast.is_empty(), "the view coming forward says it");
        assert_eq!(
            crate::runtime::take_route("link-test-here").as_deref(),
            Some("tx/00ff")
        );
        // another chain's link opens the seat, routes nothing, and says why
        let _ = state.update(Message::OpenLink(
            "duck://othernet-0a1b2c3d/link-test-away/tx/00ff".into(),
        ));
        assert_eq!(state.active, Some("link-test-away"));
        assert_eq!(crate::runtime::take_route("link-test-away"), None);
        assert!(state.toast.contains("othernet"));
        // a view nobody lists opens nothing
        state.toast.clear();
        let _ = state.update(Message::OpenLink(
            "duck://testkit-0a1b2c3d/link-test-nowhere/x".into(),
        ));
        assert_eq!(state.active, Some("link-test-away"));
        assert!(state.toast.contains("link-test-nowhere"));
    }

    #[test]
    fn a_view_event_sets_its_badge_and_opens_its_link() {
        let (mut state, _) = Ducktape::boot();
        let _ = state.update(Message::ViewEvent("chat", Intent::Badge(3)));
        assert_eq!(state.badges.get("chat"), Some(&3));
        let _ = state.update(Message::ViewEvent("chat", Intent::Badge(0)));
        assert!(state.badges.is_empty(), "a zero count clears the badge");
        let _ = state.update(Message::ViewEvent("chat", Intent::Badge(-2)));
        assert!(state.badges.is_empty());
        let _ = state.update(Message::ViewEvent("chat", Intent::Notified));
        assert!(state.badges.is_empty() && state.active.is_none());

        crate::runtime::list_for_test("view-event-link");
        let _ = state.update(Message::ViewEvent(
            "chat",
            Intent::OpenLink("duck://view-event-link/room/7".into()),
        ));
        assert_eq!(state.active, Some("view-event-link"));
        assert!(state.reveal, "a link brings its seat forward");
        assert_eq!(
            crate::runtime::take_route("view-event-link").as_deref(),
            Some("room/7")
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
            genesis: [0; 32],
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
        assert!(!state.sign_in.unlock_busy, "an empty password is not sent");
        let _ = state.update(Message::PasswordTyped("hunter22".into()));
        let _ = state.update(Message::UnlockSubmit);
        assert_eq!(state.sign_in.password, "hunter22");
        let _ = state.update(Message::UnlockFailed("wrong".into()));
        assert_eq!(
            state.sign_in.password, "hunter22",
            "a retry must send what the field shows"
        );
        let _ = state.update(Message::Unlocked("ab".into()));
        assert!(state.sign_in.password.is_empty());
    }

    #[test]
    fn a_lock_stays_locked_until_unlock_and_approving_needs_a_found_request() {
        let mut state = signing_in();
        let _ = state.update(Message::Unlocked("ab".into()));
        let _ = state.update(Message::Lock);
        assert!(state.sign_in.locked && state.signer_key.is_empty());
        assert!(!state.sign_in.seating, "a lock reopened the key on its own");
        let _ = state.update(Message::ApproveOpen);
        let _ = state.update(Message::ApproveConfirm);
        assert!(!state.sign_in.unlock_busy, "approved with nothing found");
    }

    #[test]
    fn switching_node_leaves_no_key_seated() {
        let mut state = signing_in();
        state.signer_key = "ab".into();
        state.sign_in.password = "hunter22".into();
        state.sign_in.phrase = "canoe pond forest".into();
        state.sign_in.restore_phrase = "canoe pond".into();
        state.sign_in.account_step = true;
        state.sign_in.link_code = "ABCD-EFGH".into();
        state.sign_in.unlock_error = "wrong".into();
        let _ = state.update(Message::Disconnect);
        assert!(state.signer_key.is_empty() && state.sign_in.password.is_empty());
        let left = &state.sign_in;
        assert!(left.phrase.is_empty() && left.restore_phrase.is_empty());
        assert!(!left.account_step && left.link_code.is_empty() && left.unlock_error.is_empty());
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
            Message::DeviceKey(Ok(Some("ab".into()))),
        ] {
            let mut state = signing_in();
            let _ = state.update(signed_in);
            assert!(
                !state.sign_in.account_step,
                "shown before the node answered"
            );
            resolved(&mut state, None);
            assert!(state.sign_in.account_step);
            let _ = state.update(Message::CreateAccountLater);
            resolved(&mut state, None);
            assert!(!state.sign_in.account_step, "a later block reopened it");
        }
    }

    #[test]
    fn the_account_step_is_skipped_for_a_key_with_an_account_or_a_passkey() {
        let mut state = signing_in();
        let _ = state.update(Message::Unlocked("ab".into()));
        resolved(&mut state, Some((7, "ada".into())));
        assert!(!state.sign_in.account_step);
        let mut state = signing_in();
        let _ = state.update(Message::PasskeyDone("ab".into()));
        resolved(&mut state, None);
        assert!(!state.sign_in.account_step, "the passkey made the account");
        // another key's answer neither opens nor disarms it
        let mut state = signing_in();
        let _ = state.update(Message::Unlocked("ab".into()));
        let _ = state.update(Message::AccountResolved {
            node: state.connected_rpc.clone(),
            key: "cd".into(),
            account: None,
        });
        resolved(&mut state, None);
        assert!(state.sign_in.account_step);
    }

    #[test]
    fn the_rail_reopens_the_step_and_a_created_account_closes_it() {
        let mut state = signing_in();
        state.signer_key = "ab".into();
        let _ = state.update(Message::ShowCreateAccount);
        assert!(state.sign_in.account_step);
        let _ = state.update(Message::CreateAccountSubmit);
        assert!(!state.sign_in.unlock_busy, "an empty name is not sent");
        assert!(!state.sign_in.unlock_error.is_empty());
        let _ = state.update(Message::AccountCreated(Err("refused".into())));
        assert!(state.sign_in.account_step && state.sign_in.unlock_error.contains("refused"));
        let _ = state.update(Message::AccountCreated(Err(
            "error sending request for url (http://127.0.0.1:1/)".into(),
        )));
        assert!(
            !state.sign_in.unlock_error.contains("url"),
            "a transport string"
        );
        let _ = state.update(Message::AccountCreated(Ok((9, "ada".into()))));
        assert!(!state.sign_in.account_step);
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
        // A's poll lands mid-switch: the status keeps saying where it is going
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
            genesis: [0; 32],
        }
    }
}
