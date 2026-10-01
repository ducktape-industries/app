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
use std::ops::Range;
use std::sync::{Mutex, OnceLock};

use super::Guest;

/// What the dialog says for one op: the sentence, the asking program's id
/// in it (its bytes, which the card sets in mono; `None` when System asks),
/// and the thing it names (a key's fingerprint, an agent's number) on its
/// own line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Words {
    pub(crate) said: String,
    pub(crate) id: Option<Range<usize>>,
    pub(crate) shown: Option<String>,
}

/// The request's end of its ask: what its task waits on for the person's
/// yes. An ask lives only as long as its request: this dropped unanswered
/// (the request refused after it was queued, its task aborted with its
/// view torn down), the ask is withdrawn, and the shell told to sync the
/// card (`changes_channel`), so no card stays up whose Approve goes
/// nowhere.
pub(crate) struct Told(tokio::sync::oneshot::Receiver<bool>);

impl Told {
    /// The person's answer; `None` when nobody can answer any more.
    pub(crate) async fn answer(&mut self) -> Option<bool> {
        (&mut self.0).await.ok()
    }
}

impl Drop for Told {
    fn drop(&mut self) {
        moved();
    }
}

/// The shell's wake: one message each time an ask's request let go of it,
/// from whichever thread did. Installing it replaces the one before; the
/// app installs one (`launch`); a test syncs by hand.
pub(crate) fn changes_channel() -> futures::channel::mpsc::UnboundedReceiver<()> {
    let (send, receive) = futures::channel::mpsc::unbounded();
    *changes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(send);
    receive
}

fn changes() -> &'static Mutex<Option<futures::channel::mpsc::UnboundedSender<()>>> {
    static CHANGES: OnceLock<Mutex<Option<futures::channel::mpsc::UnboundedSender<()>>>> =
        OnceLock::new();
    CHANGES.get_or_init(Mutex::default)
}

fn moved() {
    if let Some(send) = &*changes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
    {
        let _ = send.unbounded_send(());
    }
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
/// One queue for the process, as there is one person. Under test, one per
/// test thread: the tests share the process, and a card one test asks for
/// would open in another test's window as its dialog closed.
pub(crate) fn waiting() -> &'static Mutex<VecDeque<Ask>> {
    #[cfg(not(test))]
    {
        static WAITING: OnceLock<Mutex<VecDeque<Ask>>> = OnceLock::new();
        WAITING.get_or_init(Mutex::default)
    }
    #[cfg(test)]
    {
        thread_local! {
            static WAITING: &'static Mutex<VecDeque<Ask>> = Box::leak(Box::default());
        }
        WAITING.with(|waiting| *waiting)
    }
}

/// The queue, less every ask whose request let go of it (`Told`).
fn lock() -> std::sync::MutexGuard<'static, VecDeque<Ask>> {
    let mut waiting = waiting()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    waiting.retain(|ask| !ask.tell.is_closed());
    waiting
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
/// someone else's (or carries a bidi override). The card names the asker
/// in one of two phrasings: "System" when genesis bound that program to a
/// role the kernel calls (`roles`, as the node's registry answers them;
/// `None` when it does not), "Program <id>" for any other, the id as the
/// chain lists it and set in mono. No id reads as the first: a program
/// named `system` asks as "Program system". `own` is the account the seated key holds as the app
/// resolved it, `None` until it has: a `RemoveKey` names the account it
/// strips, which a manager may make an agent's, and the card says which.
pub(super) fn needed(
    program: &str,
    roles: Option<&abi::Roles>,
    target: &str,
    body: &[u8],
    own: Option<u64>,
) -> Option<Words> {
    if target != identity::MODULE {
        return None;
    }
    let bound = roles.is_some_and(|roles| {
        [&roles.registry, &roles.validators, &roles.identity]
            .iter()
            .any(|role| role.as_str() == program)
    });
    const PROGRAM: &str = "Program ";
    let (by, id) = match bound {
        true => ("System".to_owned(), None),
        false => (
            format!("{PROGRAM}{program}"),
            Some(PROGRAM.len()..PROGRAM.len() + program.len()),
        ),
    };
    // every sentence begins with `by`, so `id` is where the id is in it
    let words = |said: String, shown: Option<String>| Words {
        said,
        id: id.clone(),
        shown,
    };
    let Ok(op) = abi::decode::<identity::Op>(body) else {
        return Some(words(
            format!("{by} asks your key to sign an identity operation this app cannot read."),
            None,
        ));
    };
    match op {
        identity::Op::RemoveKey { account, key } if own == Some(account) => Some(words(
            format!("{by} asks to remove a key from your account. Approve only if you meant to."),
            Some(crate::backend::join::fingerprint(&key)),
        )),
        identity::Op::RemoveKey { account, key } => Some(words(
            format!(
                "{by} asks to remove a key from agent #{account}. Approve only if you meant to."
            ),
            Some(format!(
                "#{account} · {}",
                crate::backend::join::fingerprint(&key)
            )),
        )),
        identity::Op::Revoke { account } => Some(words(
            format!("{by} asks to revoke agent #{account}. Its keys stop working for good."),
            Some(format!("#{account}")),
        )),
        identity::Op::Suspend { account } => Some(words(
            format!("{by} asks to suspend agent #{account}. It stops acting until it is resumed."),
            Some(format!("#{account}")),
        )),
        identity::Op::RegisterModule { .. }
        | identity::Op::Create { .. }
        | identity::Op::CreateAgent { .. }
        | identity::Op::AddKey { .. }
        | identity::Op::SetName { .. }
        | identity::Op::SetProfile { .. }
        | identity::Op::Resume { .. } => None,
    }
}

/// Queues the ask for `guest`'s request and tells the shell; the `Told`
/// is what the request's task waits on, and holds the ask in the queue.
/// `None`, with the request refused, while another ask of this view waits.
pub(super) fn ask(guest: &mut Guest, id: u64, words: Words) -> Option<Told> {
    let told = queue(guest.module, guest.instance, words);
    match told {
        Some(_) => guest.intents.push(super::Intent::Consent),
        None => guest.refuse(
            id,
            super::wire::methods::refusal::CONSENT_REFUSED,
            "another confirmation for this view is still waiting",
        ),
    }
    told
}

/// The ask itself, queued for view (`module`, `instance`); `None` while
/// another of that view's waits. The shell is not told here (`ask` is).
pub(crate) fn queue(module: &'static str, instance: u64, words: Words) -> Option<Told> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let mut waiting = lock();
    if waiting
        .iter()
        .any(|ask| ask.module == module && ask.instance == instance)
    {
        return None;
    }
    let (tell, told) = tokio::sync::oneshot::channel();
    waiting.push_back(Ask {
        id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        module,
        instance,
        words,
        tell,
    });
    Some(Told(told))
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
        let asked =
            |op: &identity::Op| needed("chat", None, identity::MODULE, &abi::encode(op), Some(7));
        let remove = identity::Op::RemoveKey {
            account: 7,
            key: vec![1, 2, 3],
        };
        let fingerprint = crate::backend::join::fingerprint(&[1, 2, 3]);
        let removed = asked(&remove).unwrap();
        assert!(
            removed
                .said
                .starts_with("Program chat asks to remove a key from your account"),
            "{removed:?}"
        );
        assert_eq!(removed.shown.as_deref(), Some(fingerprint.as_str()));
        // the signed account is not the seated key's: an agent's, and the
        // card says whose, beside the key (also while none is resolved yet)
        for own in [Some(8), None] {
            let removed =
                needed("chat", None, identity::MODULE, &abi::encode(&remove), own).unwrap();
            assert!(
                removed
                    .said
                    .starts_with("Program chat asks to remove a key from agent #7"),
                "{removed:?}"
            );
            assert_eq!(
                removed.shown.as_deref(),
                Some(format!("#7 · {fingerprint}").as_str())
            );
        }
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
        let unread = needed("chat", None, identity::MODULE, &[0xff, 0xff], Some(7)).unwrap();
        assert!(unread.said.contains("cannot read"), "{unread:?}");
        assert_eq!(unread.shown, None);
        assert_eq!(needed("chat", None, "chat", &[0xff, 0xff], Some(7)), None);
    }

    /// Claim: the card names a program genesis bound to a role the kernel
    /// calls "System", by the bindings the node answered and no name the
    /// app assumes; any other program, and every program while no bindings
    /// are known, by its program id.
    #[test]
    fn a_role_program_asks_as_system_and_any_other_by_its_id() {
        let suspend = abi::encode(&identity::Op::Suspend { account: 12 });
        let said = |program: &str, roles: Option<&abi::Roles>| {
            needed(program, roles, identity::MODULE, &suspend, Some(7))
                .unwrap()
                .said
        };
        let roles = abi::Roles {
            registry: "registry-b".into(),
            validators: "valset-b".into(),
            identity: "identity-b".into(),
        };
        for program in ["registry-b", "valset-b", "identity-b"] {
            assert_eq!(
                said(program, Some(&roles)),
                "System asks to suspend agent #12. It stops acting until it is resumed."
            );
        }
        for program in ["chat", identity::MODULE, "module-registry"] {
            assert_eq!(
                said(program, Some(&roles)),
                format!(
                    "Program {program} asks to suspend agent #12. It stops acting until it is resumed."
                )
            );
        }
        assert!(said("registry-b", None).starts_with("Program registry-b asks"));
    }

    /// Claim: no program id reads as the system. A program named `system`,
    /// or `System` (landed before the registry refused that spelling), asks
    /// as "Program <id>", its id the part set in mono; only a program bound
    /// to a role asks as "System", with no id to set.
    #[test]
    fn no_program_id_asks_as_the_system() {
        let suspend = abi::encode(&identity::Op::Suspend { account: 12 });
        let roles = abi::Roles {
            registry: "module-registry".into(),
            validators: "valset".into(),
            identity: identity::MODULE.into(),
        };
        let asked = |program: &str| {
            needed(program, Some(&roles), identity::MODULE, &suspend, Some(7)).unwrap()
        };
        for program in ["System", "system"] {
            let words = asked(program);
            assert_eq!(
                words.said,
                format!(
                    "Program {program} asks to suspend agent #12. It stops acting until it is resumed."
                )
            );
            assert_eq!(words.id.map(|id| &words.said[id]), Some(program));
        }
        let system = asked("valset");
        assert_eq!(
            (system.said.as_str(), system.id),
            (
                "System asks to suspend agent #12. It stops acting until it is resumed.",
                None
            )
        );
    }

    /// Claim: an ask is in the queue exactly as long as its request holds
    /// its `Told`; let go of unanswered, it is gone from the front, it no
    /// longer blocks the view's next ask, and the shell is woken to sync.
    #[test]
    fn an_ask_lives_only_as_long_as_its_request() {
        let mut woken = changes_channel();
        let words = Words {
            said: "x".into(),
            id: None,
            shown: None,
        };
        let told = queue("t", 1, words.clone()).expect("queued");
        let (id, _) = front().expect("waits");
        assert!(queue("t", 1, words.clone()).is_none(), "one per view");
        drop(told);
        assert_eq!(front(), None, "withdrawn with its request");
        assert!(woken.try_recv().is_ok(), "the shell is told");
        assert!(!answer(id, true), "nothing to answer");
        let told = queue("t", 1, words).expect("the view may ask again");
        let (next, _) = front().unwrap();
        assert_ne!(id, next);
        assert!(!answer(next, false));
        drop(told);
    }
}
