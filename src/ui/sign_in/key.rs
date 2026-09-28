//! This device's key: opened or made on reaching a network, an old
//! password-locked one moved into the OS, locked, and reading without it.

use crate::backend;
use crate::ui::task::Task;
use crate::ui::{AppMessage as Message, Ducktape, Secret, Stage, Unlock};

impl Ducktape {
    pub(super) fn on_key(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::PasswordTyped(text) => {
                if let Stage::Unlock(step) = &mut self.stage {
                    step.password = text.into();
                }
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
                let typed = match &self.stage {
                    Stage::Unlock(step) => step.password.as_str(),
                    _ => "",
                };
                if typed.is_empty() {
                    self.sign_in.unlock_error = "Type this key's password first.".into();
                    return Task::none();
                }
                let password = Secret::new(typed.to_owned());
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
            // The key step stays up, its password gone, until the node says
            // whether the key holds an account (`AccountResolved`). A key
            // that lands after the network was left seats nothing on screen.
            Message::Unlocked(pubkey) => {
                self.sign_in.unlock_busy = false;
                self.key_exists = false;
                self.sign_in.seating = false;
                self.sign_in.locked = false;
                // another key's account is not this one's: the views hear
                // none until `AccountResolved` names this key's
                if self.signer_key != pubkey {
                    self.account = None;
                }
                self.signer_key = pubkey;
                if !matches!(self.stage, Stage::Connect) {
                    self.stage = Stage::Unlock(Unlock {
                        awaiting: true,
                        ..Unlock::default()
                    });
                }
                self.resolve_account()
            }
            Message::UnlockFailed(error) => {
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
                self.stage = Stage::Unlock(Unlock::default());
                Task::future(async {
                    backend::lock_signer().await;
                    Message::ShowToast("Locked".into())
                })
            }
            // Reading opens the desk, unless the key is seated already and
            // the node's answer is on its way: that answer decides.
            Message::BrowseWithoutKey => {
                match &mut self.stage {
                    Stage::Unlock(step) if step.awaiting => step.password = Default::default(),
                    _ => self.stage = Stage::Desk,
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::SignIn => {
                self.sign_in.locked = false;
                self.stage = Stage::Unlock(Unlock::default());
                self.open_device_key()
            }
            _ => unreachable!("routed by `on_sign_in`"),
        }
    }

    /// Opens this device's key for the network in hand and seats it — made
    /// here the first time this device meets the network. Nothing to do when
    /// one is seated, or the person locked it; a password-locked key from
    /// before answers `Ok(None)` and waits for its password once.
    pub(in crate::ui) fn open_device_key(&mut self) -> Task<Message> {
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
}
