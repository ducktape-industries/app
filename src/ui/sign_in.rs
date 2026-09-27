//! Signing in: this device's key, a recovery phrase and its check, the
//! account step, passkeys, and approving another device. Each flow is a
//! file; `on_sign_in` only routes.

mod account;
mod approve;
mod key;
mod recovery;

use super::{AppMessage as Message, Ducktape};
use view_wire::Task;

impl Ducktape {
    /// The key, phrase, account and device-approval steps.
    pub(super) fn on_sign_in(&mut self, message: Message) -> Task<Message> {
        use Message as M;
        match message {
            m @ (M::PasswordTyped(_)
            | M::UnlockSubmit
            | M::DeviceKey(_)
            | M::Unlocked(_)
            | M::UnlockFailed(_)
            | M::Lock
            | M::BrowseWithoutKey
            | M::SignIn) => self.on_key(m),
            m @ (M::RecoveryKeyStart
            | M::PhraseCancel
            | M::PhraseWrittenDown
            | M::PhraseWordTyped(..)
            | M::PhraseCheckSubmit
            | M::RecoveryKeyAdded(_)
            | M::PhraseShowAgain
            | M::RestorePhraseTyped(_)
            | M::RecoverShow
            | M::RecoverCancel
            | M::RecoverSubmit) => self.on_recovery(m),
            m @ (M::LinkStart
            | M::LinkCancel
            | M::Joined(_)
            | M::AccountNameTyped(_)
            | M::PasskeyCreateSubmit
            | M::PasskeySignInSubmit
            | M::PasskeyUsePhone
            | M::PasskeyQr(_)
            | M::PasskeyCancel
            | M::PasskeyFailed(_)
            | M::PasskeyDone(_)
            | M::ShowCreateAccount
            | M::CreateAccountLater
            | M::CreateAccountSubmit
            | M::AccountCreated(_)) => self.on_account(m),
            m @ (M::ApproveOpen
            | M::ApproveCodeTyped(_)
            | M::ApproveFind
            | M::ApproveFound(_)
            | M::ApproveConfirm
            | M::ApproveDone(_)) => self.on_approve(m),
            _ => unreachable!("routed by `update`"),
        }
    }
}

/// Three distinct word positions (0-based, ascending) out of `words` to ask
/// back: the person types them to show the phrase was written down.
pub(super) fn quiz_positions(words: usize) -> [usize; 3] {
    let mut picked = rand::seq::index::sample(&mut rand::thread_rng(), words.max(3), 3).into_vec();
    picked.sort_unstable();
    [picked[0], picked[1], picked[2]]
}

/// Whether each answer is the phrase's word at the position asked,
/// ignoring case and surrounding space.
pub(super) fn quiz_matches(
    phrase: &str,
    asked: [usize; 3],
    answers: &[impl AsRef<str>; 3],
) -> bool {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    asked.iter().zip(answers).all(|(&nth, answer)| {
        words
            .get(nth)
            .is_some_and(|word| word.eq_ignore_ascii_case(answer.as_ref().trim()))
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
