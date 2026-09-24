//! Signing in: this device's key, a recovery phrase and its check, the
//! account step, passkeys, and approving another device.

use super::{AppMessage as Message, Ducktape, Overlay};
use crate::backend;
use futures::StreamExt as _;
use view_wire::Task;
use zeroize::Zeroize;

impl Ducktape {
    /// The key, phrase, account and device-approval steps.
    pub(super) fn on_sign_in(&mut self, message: Message) -> Task<Message> {
        match message {
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
            Message::UnlockFailed(error) => {
                self.sign_in.passkey_task = None;
                self.key_exists = backend::key_exists(&self.keyring);
                self.sign_in.unlock_busy = false;
                self.sign_in.unlock_error = error;
                Task::none()
            }
            Message::Lock => {
                self.close_menu();
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
            Message::RecoveryKeyStart => {
                self.close_menu();
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
                self.overlay = Some(Overlay::Approve);
                self.sign_in.approve_code.clear();
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
                        self.close(Overlay::Approve);
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
            _ => unreachable!("routed by `update`"),
        }
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
    pub(super) fn open_device_key(&mut self) -> Task<Message> {
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
pub(super) fn offers_account_step(armed: bool, account: &Option<(u64, String)>) -> bool {
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
pub(super) fn quiz_positions(words: usize) -> [usize; 3] {
    let mut picked = rand::seq::index::sample(&mut rand::thread_rng(), words.max(3), 3).into_vec();
    picked.sort_unstable();
    [picked[0], picked[1], picked[2]]
}

/// Whether each answer is the phrase's word at the position asked,
/// ignoring case and surrounding space.
pub(super) fn quiz_matches(phrase: &str, asked: [usize; 3], answers: &[String; 3]) -> bool {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    asked.iter().zip(answers).all(|(&nth, answer)| {
        words
            .get(nth)
            .is_some_and(|word| word.eq_ignore_ascii_case(answer.trim()))
    })
}

/// Whatever a person pastes for a recovery phrase — any run of whitespace
/// between words, any case — folded to what BIP39 checks.
pub(super) fn normalize_phrase(raw: &str) -> String {
    raw.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}
