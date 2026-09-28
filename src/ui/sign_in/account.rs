//! The account step: a name for a new account, a passkey that makes or
//! joins one, and "From another device", which waits to be approved.

use crate::backend;
use crate::ui::task::Task;
use crate::ui::{Account, AppMessage as Message, Ducktape, Stage};
use futures::StreamExt as _;

impl Ducktape {
    pub(super) fn on_account(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::LinkStart => {
                let Stage::Account(step) = &mut self.stage else {
                    return Task::none();
                };
                if step.link_task.is_some() {
                    return Task::none();
                }
                let code = backend::join::new_code();
                step.link_code = code.clone();
                self.sign_in.unlock_error.clear();
                let client = backend::RpcClient::new(self.connected_rpc.clone());
                let network = self.network.clone();
                let (task, handle) = Task::future(async move {
                    Message::Joined(backend::join::join_from_device(&client, &network, &code).await)
                })
                .abortable();
                step.link_task = Some(handle.abort_on_drop());
                task
            }
            Message::LinkCancel => {
                if let Stage::Account(step) = &mut self.stage {
                    step.link_task = None;
                    step.link_code.clear();
                }
                Task::none()
            }
            // Another device or the recovery key added this one: the desk.
            Message::Joined(joined) => {
                self.sign_in.unlock_busy = false;
                if let Stage::Account(step) = &mut self.stage {
                    step.link_task = None;
                    step.link_code.clear();
                }
                match joined {
                    Ok(()) => {
                        if matches!(self.stage, Stage::Account(_) | Stage::Recover(_)) {
                            self.stage = Stage::Desk;
                        }
                        self.resolve_account()
                    }
                    Err(error) => {
                        self.sign_in.unlock_error = error;
                        Task::none()
                    }
                }
            }
            Message::AccountNameTyped(text) => {
                if let Stage::Account(step) = &mut self.stage {
                    step.name = text;
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::PasskeyCreateSubmit => self.passkey(true),
            Message::PasskeySignInSubmit => self.passkey(false),
            Message::PasskeyUsePhone => {
                if let Stage::Account(step) = &self.stage {
                    step.passkey_phone
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                Task::none()
            }
            Message::PasskeyQr(url) => {
                if let Stage::Account(step) = &mut self.stage {
                    step.passkey_qr = url;
                }
                Task::none()
            }
            Message::PasskeyCancel => {
                self.end_passkey();
                Task::done(Message::ShowToast("Passkey step cancelled".into()))
            }
            Message::PasskeyFailed(error) => {
                self.end_passkey();
                self.sign_in.unlock_error = error;
                Task::none()
            }
            // The passkey made (or joined) the account: the desk.
            Message::PasskeyDone(pubkey) => {
                self.end_passkey();
                if pubkey != self.signer_key {
                    return Task::none();
                }
                if matches!(self.stage, Stage::Account(_)) {
                    self.stage = Stage::Desk;
                    self.open_help(true);
                }
                self.resolve_account()
            }
            Message::ShowCreateAccount => {
                if !self.signer_key.is_empty() {
                    self.stage = Stage::Account(Account::default());
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::CreateAccountLater => {
                if matches!(self.stage, Stage::Account(_)) {
                    self.stage = Stage::Desk;
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::CreateAccountSubmit => {
                let Stage::Account(step) = &self.stage else {
                    return Task::none();
                };
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                let name = step.name.trim().to_owned();
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
                        backend::identity::create_plain_account(&client, &network, &name).await,
                    )
                })
            }
            Message::AccountCreated(created) => {
                self.sign_in.unlock_busy = false;
                match created {
                    Ok(account) => {
                        self.account = Some(Some(account));
                        if matches!(self.stage, Stage::Account(_)) {
                            self.stage = Stage::Desk;
                            self.open_help(true);
                        }
                    }
                    Err(error) => self.sign_in.unlock_error = account_error(&self.network, error),
                }
                Task::none()
            }
            _ => unreachable!("routed by `on_sign_in`"),
        }
    }

    /// The passkey ceremony over, however it ended.
    fn end_passkey(&mut self) {
        if let Stage::Account(step) = &mut self.stage {
            step.passkey_task = None;
        }
        self.sign_in.unlock_busy = false;
    }

    /// A passkey creates an account (`create`) or admits this device's key
    /// into the account it holds. Both are about the ACCOUNT: the device
    /// key is already seated before either is offered (opened, or minted
    /// on first contact; it has no phrase), and it signs the writes.
    fn passkey(&mut self, create: bool) -> Task<Message> {
        let Stage::Account(step) = &mut self.stage else {
            return Task::none();
        };
        if self.sign_in.unlock_busy {
            return Task::none();
        }
        if self.signer_key.is_empty() {
            self.sign_in.unlock_error = "Unlock this device's key first.".into();
            return Task::none();
        }
        let name = step.name.trim().to_owned();
        if create && name.is_empty() {
            self.sign_in.unlock_error = "Name your account first.".into();
            return Task::none();
        }
        let network = self.network.clone();
        let client = backend::RpcClient::new(self.connected_rpc.clone());
        let pubkey = self.signer_key.clone();
        self.sign_in.unlock_busy = true;
        self.sign_in.unlock_error.clear();
        step.passkey_phone = Default::default();
        step.passkey_qr.clear();
        let (phone, urls) = backend::auth_page::Phone::new(step.passkey_phone.clone());
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
        step.passkey_task = Some(handle.abort_on_drop());
        task
    }
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
