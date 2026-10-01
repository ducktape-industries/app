//! The person's native yes before the device key signs an identity op that
//! removes or suspends a key or an agent: `RemoveKey`, `Revoke`, `Suspend`,
//! and any `identity` op this app cannot read (the deployed program may
//! know one this build does not). The app is a blind host, but it already
//! names `identity` (`backend/identity.rs`, `passkey.rs`, `join.rs`): this
//! is the one more place, and the one place these ops are named.
//!
//! The ask waits here, as a notice waits in `notify::center`: the kernel
//! queues an [`Ask`] with the request's `op.submit` task parked on its
//! answer, says `Intent::Consent`, and the shell shows the front of the
//! queue in its Approve-style dialog (`shell/layers/overlays/consent.rs`),
//! answering [`answer`] with the person's Approve or Cancel. Cancel, the
//! dialog closed any other way, or a window to ask in missing, refuses the
//! view `consent_refused`; nothing is signed and the sequence does not
//! move. One ask waits per view at a time: a second while one waits is
//! refused at once.
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use super::Guest;

/// What the dialog says for one op: the sentence, and the thing it names
/// (a key's fingerprint, an agent's number) on its own line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Words {
    pub(crate) said: String,
    pub(crate) shown: Option<String>,
}

/// One confirmation a view waits on.
pub(crate) struct Ask {
    pub(crate) id: u64,
    pub(crate) module: &'static str,
    pub(crate) instance: u64,
    pub(crate) words: Words,
    tell: tokio::sync::oneshot::Sender<bool>,
}

/// The asks waiting for the person, oldest first; the shell shows the front.
pub(crate) fn waiting() -> &'static Mutex<VecDeque<Ask>> {
    static WAITING: OnceLock<Mutex<VecDeque<Ask>>> = OnceLock::new();
    WAITING.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, VecDeque<Ask>> {
    waiting()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The front ask, as the dialog draws it: its id and its words. `None`
/// while nothing waits.
pub(crate) fn front() -> Option<(u64, Words)> {
    lock().front().map(|ask| (ask.id, ask.words.clone()))
}

/// The person's answer to ask `id`: Approve (`true`) lets the op's task go
/// on to sign, anything else refuses it. An id that is not the front's is
/// one already answered, and nothing happens. Answers whether another
/// ask waits behind it.
pub(crate) fn answer(id: u64, yes: bool) -> bool {
    let mut waiting = lock();
    if waiting.front().is_some_and(|ask| ask.id == id)
        && let Some(ask) = waiting.pop_front()
    {
        // the task may be gone (its view torn down): nothing to tell
        let _ = ask.tell.send(yes);
    }
    !waiting.is_empty()
}

/// Every ask refused: the person had nowhere to answer (no console).
pub(crate) fn refuse_all() {
    for ask in lock().drain(..) {
        let _ = ask.tell.send(false);
    }
}

/// Whether an `op.submit` to `target` carrying `body` needs the person's
/// yes, and the words to ask it with. `program` is the requesting view's
/// program id, the roster's name for it: a view's manifest name is the
/// view's own word, and a hostile one would pick a name that reads as
/// someone else's (or carries a bidi override). The id is the one the
/// chain lists it under.
pub(super) fn needed(program: &str, target: &str, body: &[u8]) -> Option<Words> {
    if target != identity::MODULE {
        return None;
    }
    let Ok(op) = abi::decode::<identity::Op>(body) else {
        return Some(Words {
            said: format!(
                "{program} asks your key to sign an identity operation this app cannot read."
            ),
            shown: None,
        });
    };
    match op {
        identity::Op::RemoveKey { key, .. } => Some(Words {
            said: format!(
                "{program} asks to remove a key from your account. Approve only if you meant to."
            ),
            shown: Some(crate::backend::join::fingerprint(&key)),
        }),
        identity::Op::Revoke { account } => Some(Words {
            said: format!(
                "{program} asks to revoke agent #{account}. Its keys stop working for good."
            ),
            shown: Some(format!("#{account}")),
        }),
        identity::Op::Suspend { account } => Some(Words {
            said: format!(
                "{program} asks to suspend agent #{account}. It stops acting until it is resumed."
            ),
            shown: Some(format!("#{account}")),
        }),
        identity::Op::RegisterModule { .. }
        | identity::Op::Create { .. }
        | identity::Op::CreateAgent { .. }
        | identity::Op::AddKey { .. }
        | identity::Op::SetName { .. }
        | identity::Op::SetProfile { .. }
        | identity::Op::Resume { .. } => None,
    }
}

/// Queues the ask for `guest`'s request and tells the shell; the receiver
/// is what the request's task waits on. `None`, with the request refused,
/// while another ask of this view waits.
pub(super) fn ask(
    guest: &mut Guest,
    id: u64,
    words: Words,
) -> Option<tokio::sync::oneshot::Receiver<bool>> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let mut waiting = lock();
    if waiting
        .iter()
        .any(|ask| ask.module == guest.module && ask.instance == guest.instance)
    {
        drop(waiting);
        guest.refuse(
            id,
            super::wire::methods::refusal::CONSENT_REFUSED,
            "another confirmation for this view is still waiting",
        );
        return None;
    }
    let (tell, told) = tokio::sync::oneshot::channel();
    waiting.push_back(Ask {
        id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        module: guest.module,
        instance: guest.instance,
        words,
        tell,
    });
    drop(waiting);
    guest.intents.push(super::Intent::Consent);
    Some(told)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claim: exactly the ops that drop or stop a key or an agent are
    /// asked, naming the requesting program; an op this app cannot read is
    /// asked too, never passed; every other identity op, and any other
    /// program, is not.
    #[test]
    fn the_ops_that_remove_or_suspend_are_asked_and_nothing_else_is() {
        let asked = |op: &identity::Op| needed("chat", identity::MODULE, &abi::encode(op));
        let removed = asked(&identity::Op::RemoveKey {
            account: 7,
            key: vec![1, 2, 3],
        })
        .unwrap();
        assert!(
            removed.said.starts_with("chat asks to remove a key"),
            "{removed:?}"
        );
        assert_eq!(
            removed.shown.as_deref(),
            Some(crate::backend::join::fingerprint(&[1, 2, 3]).as_str())
        );
        let revoked = asked(&identity::Op::Revoke { account: 12 }).unwrap();
        assert!(revoked.said.contains("revoke agent #12"), "{revoked:?}");
        assert_eq!(revoked.shown.as_deref(), Some("#12"));
        let suspended = asked(&identity::Op::Suspend { account: 12 }).unwrap();
        assert!(
            suspended.said.contains("suspend agent #12"),
            "{suspended:?}"
        );
        for op in [
            identity::Op::Resume { account: 12 },
            identity::Op::SetName {
                account: 12,
                name: "x".into(),
            },
            identity::Op::CreateAgent { name: "x".into() },
            identity::Op::Create {
                name: "x".into(),
                scheme: abi::Scheme::Ed25519,
            },
        ] {
            assert_eq!(asked(&op), None, "{op:?}");
        }
        let unread = needed("chat", identity::MODULE, &[0xff, 0xff]).unwrap();
        assert!(unread.said.contains("cannot read"), "{unread:?}");
        assert_eq!(unread.shown, None);
        assert_eq!(needed("chat", "chat", &[0xff, 0xff]), None);
    }
}
