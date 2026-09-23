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
            Message::EndpointTyped(text) => {
                self.endpoint = text;
                self.endpoint_error.clear();
                Task::none()
            }
            Message::ConnectSubmit => match backend::endpoint_origin(&self.endpoint) {
                Some(origin) => self.update(Message::ConnectTo(origin)),
                None => {
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
                if status.contract != backend::noded::NODE_CONTRACT {
                    self.status = "Not connected".into();
                    self.error = format!(
                        "this node speaks contract {}; this app speaks {}",
                        status.contract,
                        backend::noded::NODE_CONTRACT
                    );
                    return Task::none();
                }
                let client = backend::RpcClient::new(origin.clone());
                backend::note_endpoint(&origin, &status.network);
                self.recent_endpoints = backend::recent_endpoints();
                self.connected_rpc = origin;
                self.network = status.network.clone();
                self.connected = true;
                self.browsing = false;
                self.key_exists = backend::key_exists(&self.network);
                self.screen = Screen::Console;
                self.apply_status(&status);
                drop(crate::runtime::connected(&client, &self.network));
                self.push_props();
                match self.console_win {
                    Some(key) => crate::shell::raise(key),
                    None => {
                        let (key, opened) = crate::shell::open(crate::shell::WindowKind::Console);
                        self.console_win = Some(key);
                        opened.map(Message::ConsoleOpened)
                    }
                }
            }
            Message::ConnectFailed { generation, error } => {
                if generation != self.connect_generation {
                    return Task::none();
                }
                self.connect_task = None;
                self.connecting = false;
                self.connected = false;
                self.status = "Not connected".into();
                self.error = backend::connect_error(&self.endpoint, error);
                Task::none()
            }
            Message::StatusPushed(status) => {
                let moved = i64::try_from(status.height).unwrap_or(-1) != self.height;
                self.apply_status(&status);
                if moved {
                    drop(crate::runtime::deployments_checked());
                }
                Task::none()
            }
            Message::Disconnect => {
                self.connect_generation += 1;
                self.connect_task = None;
                self.connected = false;
                self.connecting = false;
                self.connected_rpc.clear();
                self.network.clear();
                self.active = None;
                self.status = "Not connected".into();
                self.screen = Screen::Connect;
                self.browsing = false;
                self.password.clear();
                self.confirm_password.clear();
                self.phrase.clear();
                self.unlock_error.clear();
                self.key_exists = false;
                self.restoring = false;
                self.restore_phrase.clear();
                self.restore_password.clear();
                self.restore_confirm_password.clear();
                self.account_name.clear();
                self.passkey_task = None;
                self.unlock_busy = false;
                self.push_props();
                Task::none()
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
            Message::UnlockSubmit => {
                if self.unlock_busy {
                    return Task::none();
                }
                let password = zeroize::Zeroizing::new(std::mem::take(&mut self.password));
                let network = self.network.clone();
                self.unlock_busy = true;
                self.unlock_error.clear();
                Task::future(async move {
                    let path = match backend::session_key_path(&network) {
                        Ok(path) => path,
                        Err(error) => return Message::UnlockFailed(backend::user_error(error)),
                    };
                    match backend::seat_signer(path, password).await {
                        Ok(pubkey) => Message::Unlocked(pubkey),
                        Err(error) => Message::UnlockFailed(backend::user_error(error)),
                    }
                })
            }
            Message::CreateWalletSubmit => {
                if self.unlock_busy {
                    return Task::none();
                }
                if let Some(error) = weak_password(&self.password, &self.confirm_password) {
                    self.unlock_error = error;
                    return Task::none();
                }
                let password = zeroize::Zeroizing::new(std::mem::take(&mut self.password));
                self.confirm_password.clear();
                let network = self.network.clone();
                self.unlock_busy = true;
                self.unlock_error.clear();
                Task::future(async move {
                    let created = tokio::task::spawn_blocking({
                        let network = network.clone();
                        let password = password.clone();
                        move || backend::create_wallet(&network, "default", &password)
                    })
                    .await
                    .unwrap_or_else(|_| Err("creating the wallet did not finish".into()));
                    let phrase = match created {
                        Ok(phrase) => phrase,
                        Err(error) => return Message::UnlockFailed(backend::user_error(error)),
                    };
                    let path = match backend::session_key_path(&network) {
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
                self.push_props();
                Task::none()
            }
            Message::PhraseWrittenDown => {
                self.phrase.clear();
                Task::none()
            }
            Message::BrowseWithoutKey => {
                self.browsing = true;
                self.password.clear();
                self.unlock_error.clear();
                Task::none()
            }
            Message::SignIn => {
                self.browsing = false;
                Task::none()
            }
            Message::ShowRestore => {
                self.restoring = true;
                self.unlock_error.clear();
                Task::none()
            }
            Message::RestoreCancel => {
                self.restoring = false;
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
                let password = zeroize::Zeroizing::new(std::mem::take(&mut self.restore_password));
                self.restore_confirm_password.clear();
                let network = self.network.clone();
                self.unlock_busy = true;
                self.unlock_error.clear();
                Task::future(async move {
                    let restored = tokio::task::spawn_blocking({
                        let network = network.clone();
                        let password = password.clone();
                        move || backend::restore_wallet(&network, &phrase, &password)
                    })
                    .await
                    .unwrap_or_else(|_| Err("restoring the key did not finish".into()));
                    if let Err(error) = restored {
                        return Message::UnlockFailed(backend::user_error(error));
                    }
                    let path = match backend::session_key_path(&network) {
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
                self.restore_phrase.clear();
                self.push_props();
                Task::none()
            }
            Message::ForgetEndpoint(url) => {
                backend::forget_endpoint(&url);
                self.recent_endpoints = backend::recent_endpoints();
                Task::none()
            }
            Message::Unlocked(pubkey) => {
                self.unlock_busy = false;
                self.signer_key = pubkey;
                self.push_props();
                Task::none()
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
                self.key_exists = backend::key_exists(&self.network);
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
                self.push_props();
                Task::none()
            }
            Message::UnlockFailed(error) => {
                self.passkey_task = None;
                self.key_exists = backend::key_exists(&self.network);
                self.unlock_busy = false;
                self.unlock_error = error;
                Task::none()
            }
            Message::Lock => {
                self.signer_key.clear();
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
                    match client.status().await {
                        Ok(status) => Some(Message::StatusPushed(status)),
                        Err(error) => {
                            tracing::debug!(target: "ducktape::app", %error, "status not answered");
                            None
                        }
                    }
                })
                .and_then(Task::done)
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
        self.status = format!("Connected · block {}", status.height);
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
        let password = zeroize::Zeroizing::new(std::mem::take(&mut self.password));
        self.confirm_password.clear();
        let network = self.network.clone();
        let client = backend::RpcClient::new(self.connected_rpc.clone());
        self.unlock_busy = true;
        self.unlock_error.clear();
        self.passkey_phone = Default::default();
        self.passkey_qr.clear();
        let (phone, urls) = backend::passkey::Phone::new(self.passkey_phone.clone());
        let flow = async move {
            if mint {
                let minted = tokio::task::spawn_blocking({
                    let (network, password) = (network.clone(), password.clone());
                    move || backend::create_wallet(&network, "default", &password)
                })
                .await
                .unwrap_or_else(|_| Err("creating this device's key did not finish".into()));
                if let Err(error) = minted {
                    return Message::UnlockFailed(backend::user_error(error));
                }
            }
            let seated = match backend::session_key_path(&network) {
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

fn weak_password(password: &str, confirm: &str) -> Option<String> {
    if keystore::userkey::check_password_len(password).is_err() {
        return Some(format!(
            "The password needs at least {} characters.",
            keystore::userkey::MIN_PASSWORD_LEN
        ));
    }
    (password != confirm).then(|| "The passwords don't match.".to_string())
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
    fn weak_password_catches_short_and_mismatched() {
        assert!(weak_password("short", "short").is_some());
        assert!(weak_password("longenough1", "different").is_some());
        assert!(weak_password("longenough1", "longenough1").is_none());
    }
}
