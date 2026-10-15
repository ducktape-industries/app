//! The person's native yes before the device key signs an op a view asks
//! for, in two cases. An identity op that removes or suspends a key or an
//! agent: `RemoveKey`, `Revoke`, `Suspend`, and any `identity` op this app
//! cannot read (the deployed program may know one this build does not).
//! And every op to a program bound to a role (the programs role, the
//! validators, identity) from a view whose own program is not: an account
//! with a grant may change that view's code (no vote reviews it), and the
//! card says what the op does in the target program's own words
//! (`program.describe`). The app is a blind host, but it already names
//! `identity` (`backend/identity.rs`, `passkey.rs`, `join.rs`): this is the
//! one more place, and the one place these ops are named; every other
//! program it knows only by its role (`roles`).
//!
//! The ask waits here, as a notice waits in `notify::center`: the
//! request's `op.submit` task queues an [`Ask`] once its words are whole
//! (the target's description read), parks on its answer, and wakes the
//! shell ([`changes_channel`]), which shows the front of the queue in its
//! Approve-style dialog (`shell/layers/overlays/consent.rs`), answering
//! [`answer`] with the person's Approve or Cancel. Cancel, the dialog
//! closed any other way, or a window to ask in missing, refuses the view
//! `consent_refused`; nothing is signed. One ask waits per view at a time:
//! a second while one waits is refused as it would be queued.
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::{Mutex, OnceLock};

/// The card's title for an op to the person's account (identity's).
pub const ACCOUNT: &str = "Confirm a change to your account";
pub const NETWORK: &str = "Confirm a change to the network";

/// What the dialog says for one op: its title ([`ACCOUNT`] or
/// [`NETWORK`]), the sentence, the asking program's id in it (its bytes,
/// which the card sets in mono; `None` when System asks), and what it
/// shows on its own: the thing the op names (a key's fingerprint, an
/// agent's number), or, `described`, the target program's own description
/// of the op ([`shown`]), which the request's task reads in before it
/// queues the ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Words {
    pub title: &'static str,
    pub said: String,
    pub id: Option<Range<usize>>,
    pub shown: Option<String>,
    pub described: bool,
}

/// The request's end of its ask: what its task waits on for the person's
/// yes. An ask lives only as long as its request: this dropped unanswered
/// (the request refused after it was queued, its task aborted with its
/// view torn down), the ask is withdrawn, and the shell told to sync the
/// card (`changes_channel`), so no card stays up whose Approve goes
/// nowhere.
pub struct Told(tokio::sync::oneshot::Receiver<bool>);

impl Told {
    /// The person's answer; `None` when nobody can answer any more.
    pub async fn answer(&mut self) -> Option<bool> {
        (&mut self.0).await.ok()
    }
}

impl Drop for Told {
    fn drop(&mut self) {
        moved();
    }
}

/// The shell's wake: one message each time a request queued its ask or
/// let go of it, from whichever thread did. Installing it replaces the one before; the
/// app installs one (`launch`); a test syncs by hand.
pub fn changes_channel() -> futures::channel::mpsc::UnboundedReceiver<()> {
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

pub(super) fn moved() {
    if let Some(send) = &*changes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
    {
        let _ = send.unbounded_send(());
    }
}

/// One confirmation a view waits on.
pub struct Ask {
    pub id: u64,
    pub program: &'static str,
    pub instance: u64,
    pub words: Words,
    tell: tokio::sync::oneshot::Sender<bool>,
}

/// The asks waiting for the person, oldest first; the shell shows the front.
/// One queue for the process, as there is one person. Under test, one per
/// test thread: the tests share the process, and a card one test asks for
/// would open in another test's window as its dialog closed.
pub fn waiting() -> &'static Mutex<VecDeque<Ask>> {
    #[cfg(not(any(test, feature = "test-support")))]
    {
        static WAITING: OnceLock<Mutex<VecDeque<Ask>>> = OnceLock::new();
        WAITING.get_or_init(Mutex::default)
    }
    #[cfg(any(test, feature = "test-support"))]
    {
        thread_local! {
            static WAITING: &'static Mutex<VecDeque<Ask>> = Box::leak(Box::default());
        }
        WAITING.with(|waiting| *waiting)
    }
}

/// The queue, less every ask whose request let go of it (`Told`).
fn lock() -> std::sync::MutexGuard<'static, VecDeque<Ask>> {
    lock_in(waiting())
}

fn lock_in(
    waiting: &'static Mutex<VecDeque<Ask>>,
) -> std::sync::MutexGuard<'static, VecDeque<Ask>> {
    let mut waiting = waiting
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    waiting.retain(|ask| !ask.tell.is_closed());
    waiting
}

/// The front ask, as the dialog draws it: its id and its words. `None`
/// while nothing waits.
pub fn front() -> Option<(u64, Words)> {
    lock().front().map(|ask| (ask.id, ask.words.clone()))
}

/// The person's answer to ask `id`: Approve (`true`) lets the op's task go
/// on to sign, anything else refuses it. An id that is not the front's is
/// one already answered, and nothing happens. Answers whether another
/// ask waits behind it.
pub fn answer(id: u64, yes: bool) -> bool {
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
pub fn refuse_all() {
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
/// role the kernel calls (`roles`, as the node's programs role answers them;
/// `None` when it does not), "Program <id>" for any other, the id as the
/// chain lists it and set in mono. No id reads as the first: a program
/// named `system` asks as "Program system". `own` is the account the seated key holds as the app
/// resolved it, `None` until it has: a `RemoveKey` names the account it
/// strips, which a manager may make an agent's, and the card says which.
///
/// The identity ops that remove or suspend are asked in this app's own
/// words, whoever asks. Any other op to a role's program is asked when the
/// requesting program is bound to no role, in a sentence that names the
/// role and the target's own description of the op (`described`, read by
/// the request's task).
pub(super) fn needed(
    program: &str,
    roles: Option<&abi::Roles>,
    target: &str,
    body: &[u8],
    own: Option<u64>,
) -> Option<Words> {
    let bound = |name: &str| bound(name, roles);
    const PROGRAM: &str = "Program ";
    let (by, id) = match bound(program) {
        true => ("System".to_owned(), None),
        false => (
            format!("{PROGRAM}{program}"),
            Some(PROGRAM.len()..PROGRAM.len() + program.len()),
        ),
    };
    // every sentence begins with `by`, so `id` is where the id is in it
    let words = |said: String, shown: Option<String>| Words {
        title: ACCOUNT,
        said,
        id: id.clone(),
        shown,
        described: false,
    };
    if target == identity::PROGRAM {
        let Ok(op) = abi::decode::<identity::Op>(body) else {
            // the deployed identity may know it: its own words, if it does
            return Some(Words {
                described: true,
                ..words(
                    format!(
                        "{by} asks your key to sign an identity operation this app cannot read."
                    ),
                    None,
                )
            });
        };
        let asked = match op {
            identity::Op::RemoveKey { account, key } if own == Some(account) => Some(words(
                format!(
                    "{by} asks to remove a key from your account. Approve only if you meant to."
                ),
                Some(noded_client::join::fingerprint(&key)),
            )),
            identity::Op::RemoveKey { account, key } => Some(words(
                format!(
                    "{by} asks to remove a key from agent #{account}. Approve only if you meant to."
                ),
                Some(format!(
                    "#{account} · {}",
                    noded_client::join::fingerprint(&key)
                )),
            )),
            identity::Op::Revoke { account } => Some(words(
                format!("{by} asks to revoke agent #{account}. Its keys stop working for good."),
                Some(format!("#{account}")),
            )),
            identity::Op::Suspend { account } => Some(words(
                format!(
                    "{by} asks to suspend agent #{account}. It stops acting until it is resumed."
                ),
                Some(format!("#{account}")),
            )),
            identity::Op::RegisterProgram { .. }
            | identity::Op::Create { .. }
            | identity::Op::CreateAgent { .. }
            | identity::Op::AddKey { .. }
            | identity::Op::SetName { .. }
            | identity::Op::SetProfile { .. }
            | identity::Op::Resume { .. }
            | identity::Op::SetAudience { .. }
            | identity::Op::SetRunner { .. } => None,
        };
        if asked.is_some() {
            return asked;
        }
    }
    // a view no vote reviewed, writing to a role
    let roles = roles.filter(|_| !bound(program))?;
    let (title, what) = if target == roles.programs {
        (NETWORK, "sign an op on the programs role")
    } else if target == roles.validators {
        (NETWORK, "vote in the validators' program")
    } else if target == roles.identity {
        (ACCOUNT, "sign an identity operation")
    } else if target == roles.blobs {
        (NETWORK, "sign an op on the blobs role")
    } else if target == roles.awaits {
        (NETWORK, "sign an op on the awaits role")
    } else if roles.egress.as_deref() == Some(target) {
        (NETWORK, "sign an op on the egress role")
    } else {
        return None;
    };
    Some(Words {
        title,
        said: format!("{by} asks your key to {what}. Approve only if you meant to."),
        id,
        shown: None,
        described: true,
    })
}

/// Whether genesis bound `program` to a role the kernel calls (`roles`, as
/// the node's programs role answers them): never while no bindings are known.
pub(super) fn bound(program: &str, roles: Option<&abi::Roles>) -> bool {
    roles.is_some_and(|roles| {
        [
            &roles.programs,
            &roles.validators,
            &roles.identity,
            &roles.blobs,
            &roles.awaits,
        ]
        .into_iter()
        .chain(roles.egress.as_ref())
        .any(|role| role.as_str() == program)
    })
}

/// What the card shows for an op its target describes: the program's own
/// title, then its fields, `label value`, on one line; the op's size where
/// it describes nothing (no describe module, bytes it cannot read, the
/// node not reached). The op is the view's: every title, label and value
/// is [`clip`]ped.
pub fn shown(description: Option<&describe::Description>, len: usize) -> String {
    let Some(description) = description else {
        return format!("{len} bytes this app cannot read");
    };
    let title = clip(&description.title);
    if description.fields.is_empty() {
        return title;
    }
    let fields: Vec<String> = description
        .fields
        .iter()
        .map(|field| format!("{} {}", clip(&field.label), clip(&value(&field.value))))
        .collect();
    format!("{title}\n{}", fields.join(" · "))
}

/// The longest a title or a value runs on the card before it is clipped.
const MAX_VALUE: usize = 160;

/// `text` as one line of the card: its plain chars (`kernel::plain`: no
/// line break to fake a fields line, no bidi control to reorder one), at
/// most [`MAX_VALUE`] of them.
fn clip(text: &str) -> String {
    let text: String = text.chars().filter(super::kernel::plain).collect();
    match text.char_indices().nth(MAX_VALUE) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

/// One described value as text: an account by its number, a key by its
/// fingerprint (as the card shows a key it names), a hash or bytes short.
fn value(shown: &describe::Value) -> String {
    use describe::Value;
    let short = |bytes: &[u8]| design::short_hex(&abi::hex(bytes));
    match shown {
        Value::Text(text) | Value::Program(text) => text.clone(),
        Value::Account(number) => format!("#{number}"),
        Value::Key(key) => noded_client::join::fingerprint(key),
        Value::Hash(bytes) => short(bytes),
        Value::Amount { value, decimals } => match 10u128.checked_pow(u32::from(*decimals)) {
            Some(scale) if *decimals > 0 => format!(
                "{}.{:0width$}",
                value / scale,
                value % scale,
                width = usize::from(*decimals)
            ),
            _ => value.to_string(),
        },
        Value::Time(millis) => date(*millis),
        Value::Bytes { len: 0, .. } => "0 bytes".into(),
        Value::Bytes {
            len: 1..=32,
            preview,
        } => short(preview),
        Value::Bytes { len, preview } => format!("{len} bytes · {}", short(preview)),
        Value::List(items) => items.iter().map(value).collect::<Vec<_>>().join(", "),
    }
}

/// Milliseconds since the epoch as a UTC date: `12 Oct 2026, 14:03 UTC`.
fn date(millis: u64) -> String {
    let seconds = millis / 1000;
    let of_day = seconds % 86_400;
    format!(
        "{}, {:02}:{:02} UTC",
        super::day(seconds),
        of_day / 3_600,
        of_day % 3_600 / 60,
    )
}

/// The ask for view (`program`, `instance`), queued in `waiting`; `None`
/// while another of that view's waits. The shell is not told here
/// ([`moved`] is). `waiting` is the queue the request came in on, taken at
/// its press: the request's task queues the ask from the runtime's thread,
/// and under test each test thread has its own.
pub(super) fn queue_in(
    waiting: &'static Mutex<VecDeque<Ask>>,
    program: &'static str,
    instance: u64,
    words: Words,
) -> Option<Told> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let mut waiting = lock_in(waiting);
    if waiting
        .iter()
        .any(|ask| ask.program == program && ask.instance == instance)
    {
        return None;
    }
    let (tell, told) = tokio::sync::oneshot::channel();
    waiting.push_back(Ask {
        id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        program,
        instance,
        words,
        tell,
    });
    Some(Told(told))
}

/// [`queue_in`] this thread's queue.
#[cfg(any(test, feature = "test-support"))]
pub fn queue(program: &'static str, instance: u64, words: Words) -> Option<Told> {
    queue_in(waiting(), program, instance, words)
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
            |op: &identity::Op| needed("chat", None, identity::PROGRAM, &abi::encode(op), Some(7));
        let remove = identity::Op::RemoveKey {
            account: 7,
            key: vec![1, 2, 3],
        };
        let fingerprint = noded_client::join::fingerprint(&[1, 2, 3]);
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
                needed("chat", None, identity::PROGRAM, &abi::encode(&remove), own).unwrap();
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
        let unread = needed("chat", None, identity::PROGRAM, &[0xff, 0xff], Some(7)).unwrap();
        assert!(unread.said.contains("cannot read"), "{unread:?}");
        // the deployed identity's own words, read in by the request's task
        assert_eq!((unread.shown, unread.described), (None, true));
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
            needed(program, roles, identity::PROGRAM, &suspend, Some(7))
                .unwrap()
                .said
        };
        let roles = abi::Roles {
            programs: "programs-b".into(),
            validators: "valset-b".into(),
            identity: "identity-b".into(),
            blobs: "blobs-b".into(),
            awaits: "awaits-b".into(),
            egress: Some("egress-b".into()),
        };
        for program in [
            "programs-b",
            "valset-b",
            "identity-b",
            "blobs-b",
            "awaits-b",
            "egress-b",
        ] {
            assert_eq!(
                said(program, Some(&roles)),
                "System asks to suspend agent #12. It stops acting until it is resumed."
            );
        }
        for program in ["chat", identity::PROGRAM, "programs"] {
            assert_eq!(
                said(program, Some(&roles)),
                format!(
                    "Program {program} asks to suspend agent #12. It stops acting until it is resumed."
                )
            );
        }
        assert!(said("programs-b", None).starts_with("Program programs-b asks"));
        let unbound = abi::Roles {
            egress: None,
            ..roles
        };
        assert!(said("egress-b", Some(&unbound)).starts_with("Program egress-b asks"));
    }

    /// Claim: no program id reads as the system. A program named `system`
    /// asks as "Program system", its id the part set in mono; only a
    /// program bound to a role asks as "System", with no id to set. An id
    /// outside the one spelling (`System`, `"\nSystem"`) never reaches the
    /// card: the roster leaves it off (`seat::tests`).
    #[test]
    fn no_program_id_asks_as_the_system() {
        let suspend = abi::encode(&identity::Op::Suspend { account: 12 });
        let roles = abi::Roles {
            programs: "programs".into(),
            validators: "valset".into(),
            identity: identity::PROGRAM.into(),
            blobs: "blobs".into(),
            awaits: "awaits".into(),
            egress: Some("egress".into()),
        };
        let asked = |program: &str| {
            needed(program, Some(&roles), identity::PROGRAM, &suspend, Some(7)).unwrap()
        };
        let words = asked("system");
        assert_eq!(
            words.said,
            "Program system asks to suspend agent #12. It stops acting until it is resumed."
        );
        assert_eq!(words.id.map(|id| &words.said[id]), Some("system"));
        let system = asked("valset");
        assert_eq!(
            (system.said.as_str(), system.id),
            (
                "System asks to suspend agent #12. It stops acting until it is resumed.",
                None
            )
        );
    }

    /// Claim: a view whose program is bound to no role asks for every op to
    /// a role's program, the network's title for the programs role and the
    /// validators, the account's for identity, and leaves the words to the
    /// target's description; its op to any other program, any op from a
    /// role's own program, and every op while no bindings are known ask
    /// nothing new. The identity ops that remove or suspend keep their own
    /// words for both.
    #[test]
    fn a_view_no_vote_reviewed_asks_for_every_op_to_a_role() {
        let roles = abi::Roles {
            programs: "programs".into(),
            validators: "valset".into(),
            identity: identity::PROGRAM.into(),
            blobs: "blobs".into(),
            awaits: "awaits".into(),
            egress: Some("egress".into()),
        };
        let op = [1, 2, 3];
        let add_key = abi::encode(&identity::Op::AddKey {
            scheme: abi::Scheme::Ed25519,
            label: None,
            consent: identity::Consent {
                account: 7,
                key: vec![1, 2, 3],
                expires_at: 0,
                proof: vec![],
            },
        });
        let asked = |program: &str, target: &str, body: &[u8]| {
            needed(program, Some(&roles), target, body, Some(7))
        };
        for (target, body, title, what) in [
            (
                "valset",
                &op[..],
                NETWORK,
                "vote in the validators' program",
            ),
            (
                "programs",
                &op[..],
                NETWORK,
                "sign an op on the programs role",
            ),
            (
                identity::PROGRAM,
                &add_key[..],
                ACCOUNT,
                "sign an identity operation",
            ),
            ("blobs", &op[..], NETWORK, "sign an op on the blobs role"),
            ("awaits", &op[..], NETWORK, "sign an op on the awaits role"),
            ("egress", &op[..], NETWORK, "sign an op on the egress role"),
        ] {
            let words = asked("chat", target, body).expect(target);
            assert_eq!(
                words,
                Words {
                    title,
                    said: format!(
                        "Program chat asks your key to {what}. Approve only if you meant to."
                    ),
                    id: Some(8..12),
                    shown: None,
                    described: true,
                },
                "{target}"
            );
            for own in [
                "valset",
                "programs",
                identity::PROGRAM,
                "blobs",
                "awaits",
                "egress",
            ] {
                assert_eq!(asked(own, target, body), None, "{own} to {target}");
            }
            assert_eq!(
                needed("chat", None, target, body, Some(7)),
                None,
                "{target}"
            );
        }
        for target in ["chat", "forge"] {
            assert_eq!(asked("chat", target, &op), None, "{target}");
        }
        let remove = abi::encode(&identity::Op::RemoveKey {
            account: 7,
            key: vec![1, 2, 3],
        });
        for (program, by) in [("chat", "Program chat"), ("programs", "System")] {
            let words = asked(program, identity::PROGRAM, &remove).unwrap();
            assert_eq!(
                words.said,
                format!(
                    "{by} asks to remove a key from your account. Approve only if you meant to."
                )
            );
            assert_eq!((words.title, words.described), (ACCOUNT, false));
        }
    }

    /// Claim: the card shows a description as the program's title, then
    /// its fields as `label value` on one line, each value as what it is;
    /// a long value clipped; and the op's size where nothing describes it.
    #[test]
    fn a_description_reads_as_its_title_then_its_fields() {
        use describe::{Value, field};
        let description = describe::Description {
            title: "Schedule · chat".into(),
            fields: vec![
                field("change", Value::text("Set")),
                field("program", Value::Program("chat".into())),
                field("code", Value::Hash(vec![0xab; 32])),
                field("account", Value::Account(7)),
                field(
                    "amount",
                    Value::Amount {
                        value: 12_345,
                        decimals: 2,
                    },
                ),
                field("count", Value::count(3)),
                field("expires at", Value::Time(1_791_900_000_000)),
                field("leap", Value::Time(951_782_400_000)),
                field("params", Value::bytes(&[0xcd; 40])),
                field("empty", Value::bytes(&[])),
                field(
                    "both",
                    Value::List(vec![Value::Account(1), Value::Account(2)]),
                ),
            ],
        };
        assert_eq!(
            shown(Some(&description), 99),
            "Schedule · chat\nchange Set · program chat · code abababab…abab · account #7 · \
             amount 123.45 · count 3 · expires at 13 Oct 2026, 14:00 UTC · \
             leap 29 Feb 2000, 00:00 UTC · params 40 bytes · cdcdcdcd…cdcd · empty 0 bytes · \
             both #1, #2"
        );
        let long = describe::Description {
            title: "x".repeat(400),
            fields: vec![],
        };
        assert_eq!(shown(Some(&long), 9), format!("{}…", "x".repeat(MAX_VALUE)));
        assert_eq!(shown(None, 3), "3 bytes this app cannot read");
        // the op's own text fakes no line and reorders nothing: a line
        // break, a bidi override, a paragraph separator go, in the title, a
        // label and a value alike
        let forged = "a\nb\u{202e}c\u{2029}";
        let forged = describe::Description {
            title: format!("Set name · {forged}"),
            fields: vec![field(forged, Value::text(forged))],
        };
        assert_eq!(shown(Some(&forged), 9), "Set name · abc\nabc abc");
    }

    /// Claim: an ask is in the queue exactly as long as its request holds
    /// its `Told`; let go of unanswered, it is gone from the front, it no
    /// longer blocks the view's next ask, and the shell is woken to sync.
    #[test]
    fn an_ask_lives_only_as_long_as_its_request() {
        let mut woken = changes_channel();
        let words = Words {
            title: ACCOUNT,
            said: "x".into(),
            id: None,
            shown: None,
            described: false,
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
