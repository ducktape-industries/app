//! The recovery key: a new one's words and their quiz, off the account
//! menu; an existing one typed to add this device, off the account step.

use super::{normalize_phrase, quiz_matches, quiz_positions};
use crate::backend;
use crate::ui::task::Task;
use crate::ui::{Account, AppMessage as Message, Ducktape, Phrase, Recover, Stage};

impl Ducktape {
    pub(super) fn on_recovery(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::RecoveryKeyStart => {
                self.close_menu();
                self.sign_in.unlock_error.clear();
                self.stage = Stage::Phrase(Phrase {
                    words: backend::join::new_recovery_phrase().into(),
                    ..Phrase::default()
                });
                Task::none()
            }
            Message::PhraseCancel => {
                self.leave_phrase();
                Task::none()
            }
            Message::PhraseWrittenDown => {
                if let Stage::Phrase(step) = &mut self.stage {
                    step.quiz = Some(quiz_positions(step.words.split_whitespace().count()));
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::PhraseWordTyped(nth, text) => {
                if let Stage::Phrase(step) = &mut self.stage
                    && let Some(answer) = step.answers.get_mut(nth)
                {
                    *answer = text.into();
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            // The words checked: the key they make goes onto the account.
            Message::PhraseCheckSubmit => {
                let Stage::Phrase(step) = &self.stage else {
                    return Task::none();
                };
                let Some(asked) = step.quiz else {
                    return Task::none();
                };
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                if !quiz_matches(&step.words, asked, &step.answers) {
                    self.sign_in.unlock_error =
                        "Those words don't match your phrase. Check them, or show the phrase again."
                            .into();
                    return Task::none();
                }
                let phrase = step.words.clone();
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
                        self.leave_phrase();
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
                if let Stage::Phrase(step) = &mut self.stage {
                    step.quiz = None;
                    step.answers = Default::default();
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::RestorePhraseTyped(text) => {
                if let Stage::Recover(step) = &mut self.stage {
                    step.phrase = text.into();
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::RecoverShow => {
                if let Stage::Account(step) = &mut self.stage {
                    let name = std::mem::take(&mut step.name);
                    self.stage = Stage::Recover(Recover {
                        name,
                        ..Recover::default()
                    });
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::RecoverCancel => {
                if let Stage::Recover(step) = &mut self.stage {
                    let name = std::mem::take(&mut step.name);
                    self.stage = Stage::Account(Account {
                        name,
                        ..Account::default()
                    });
                }
                self.sign_in.unlock_error.clear();
                Task::none()
            }
            Message::RecoverSubmit => {
                let Stage::Recover(step) = &self.stage else {
                    return Task::none();
                };
                if self.sign_in.unlock_busy {
                    return Task::none();
                }
                let phrase = zeroize::Zeroizing::new(normalize_phrase(&step.phrase));
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
            _ => unreachable!("routed by `on_sign_in`"),
        }
    }

    /// The recovery phrase and its check, gone once passed (or abandoned):
    /// back to the desk they were opened from.
    fn leave_phrase(&mut self) {
        if matches!(self.stage, Stage::Phrase(_)) {
            self.stage = Stage::Desk;
        }
        self.sign_in.unlock_error.clear();
    }
}
