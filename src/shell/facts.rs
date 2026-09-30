//! `Facts`: the model's state as a draw reads it, copied out so the draw
//! can hold `cx` mutably while it builds.

use super::*;

/// The facts a draw reads, copied out of the model: a draw needs `cx`
/// mutably (fields, listeners) while the model is borrowed from it, so a
/// screen cannot hold `&Ducktape` as it builds. Secrets are not copied
/// here; the screen that shows one reads it itself.
#[derive(Clone)]
pub(crate) struct Facts {
    pub(crate) dark: bool,
    pub(crate) endpoint_error: String,
    pub(crate) recent_endpoints: Vec<crate::backend::RecentEndpoint>,
    pub(crate) connecting: bool,
    pub(crate) network: String,
    /// The node the console is on.
    pub(crate) connected_rpc: String,
    pub(crate) other_chain: bool,
    pub(crate) status: String,
    pub(crate) error: String,
    pub(crate) signer_key: String,
    pub(crate) unlock_error: String,
    pub(crate) unlock_busy: bool,
    pub(crate) key_exists: bool,
    pub(crate) seating: bool,
    pub(crate) locked: bool,
    /// The code this device waits under for another to approve it.
    pub(crate) link_code: String,
    /// The joining key's fingerprint, once its code was found.
    pub(crate) approve_fingerprint: Option<String>,
    pub(crate) passkey_waiting: bool,
    /// The QR URL, once the person picked the phone.
    pub(crate) passkey_qr: Option<String>,
    pub(crate) phrase_quiz: Option<[usize; 3]>,
    pub(crate) motion: bool,
}

impl Ducktape {
    pub(crate) fn facts(&self) -> Facts {
        Facts {
            dark: self.dark(),
            endpoint_error: self.endpoint_error.clone(),
            recent_endpoints: self.recent_endpoints.clone(),
            connecting: self.connecting,
            network: self.network.clone(),
            connected_rpc: self.connected_rpc.clone(),
            other_chain: self.other_chain,
            status: self.status.clone(),
            error: self.error.clone(),
            signer_key: self.signer_key.clone(),
            unlock_error: self.sign_in.unlock_error.clone(),
            unlock_busy: self.sign_in.unlock_busy,
            key_exists: self.key_exists,
            seating: self.sign_in.seating,
            locked: self.sign_in.locked,
            link_code: match &self.stage {
                crate::Stage::Account(step) => step.link_code.clone(),
                _ => String::new(),
            },
            approve_fingerprint: self.approve_fingerprint(),
            passkey_waiting: matches!(&self.stage, crate::Stage::Account(step) if step.passkey_task.is_some()),
            passkey_qr: self.passkey_qr_shown(),
            phrase_quiz: match &self.stage {
                crate::Stage::Phrase(step) => step.quiz,
                _ => None,
            },
            motion: self.motion,
        }
    }
}
