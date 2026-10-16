//! The person's native yes before the device key signs an op a view asks
//! for, in two cases. An identity op that removes or suspends a key or an
//! agent, or stops a program running an agent: `RemoveKey`, `Revoke`,
//! `Suspend`, `SetScript` with no script, and any `identity` op this app
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
    /// The same ask as a touch-first device draws it.
    pub card: Card,
}

/// The ask as the phone's confirm sheet words it (mobile
/// `src/kit/Confirm.tsx`, its `Ask`, and `src/network/consent.ts`): under
/// [`Words::title`] as its eyebrow, "<verb>: <target>", the op's facts
/// (its program, what it names, who signs), the line on what it takes
/// away, and its verb on the button beside Cancel, in danger for one that
/// takes something away. The host knows the op, its target's description
/// and the seated account's number, nothing more: where the phone names an
/// agent by its name, this names it by its number.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Card {
    pub title: String,
    pub facts: Vec<Fact>,
    pub warning: String,
    pub verb: &'static str,
    pub danger: bool,
}

/// One row of the card: a label, its value, the value in mono or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    pub label: String,
    pub value: String,
    pub mono: bool,
}

impl Fact {
    fn new(label: &str, value: impl Into<String>, mono: bool) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            mono,
        }
    }
}

/// What the phone's card says when an op asks a role's program and the
/// host knows no verb for it: its button, and the title's verb.
const APPROVE: &str = "Approve";

impl Words {
    /// The target's description of the op read in (`described`): the
    /// desktop's box ([`shown`]), and the card's title and its Operation
    /// row and fields, before who signs; the op's size where it describes
    /// nothing.
    pub fn describe(&mut self, description: Option<&describe::Description>, len: usize) {
        self.shown = Some(shown(description, len));
        let (operation, fields) = match description {
            Some(described) => (
                clip(&described.title),
                described
                    .fields
                    .iter()
                    .map(|field| Fact::new(&clip(&field.label), clip(&value(&field.value)), false))
                    .collect(),
            ),
            None => (format!("{len} bytes this app cannot read"), Vec::new()),
        };
        self.card.title = ask_title(
            self.card.verb,
            match description {
                Some(_) => &operation,
                None => "an operation this app cannot read",
            },
        );
        // after the program, before who signs
        let at = self.card.facts.len().min(1);
        let rows = std::iter::once(Fact::new("Operation", operation, false)).chain(fields);
        self.card.facts.splice(at..at, rows);
    }
}

/// "<verb>: <what>", where `what` is a described title ("Schedule · chat")
/// that a verb already saying its head drops it ("Schedule: chat"), as the
/// phone's `askTitle` does (mobile `src/network/words.ts`).
fn ask_title(verb: &str, what: &str) -> String {
    let said = what
        .split_once(" · ")
        .filter(|(head, _)| verb.to_lowercase().ends_with(&head.to_lowercase()));
    match said {
        Some((_, rest)) => format!("{verb}: {rest}"),
        None => format!("{verb}: {what}"),
    }
}

/// The card's last row: the seated account, or the device where none is
/// resolved yet (mobile `src/network/consent.ts`, `signer`).
fn signs(own: Option<u64>) -> Fact {
    Fact::new(
        "Signs",
        own.map_or_else(|| "this phone".to_owned(), |own| format!("account {own}")),
        false,
    )
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
/// One queue for the process, as there is one person. On the test services
/// (`contract::testing`), one per test thread: the tests share the process,
/// and a card one test asks for would open in another test's window as its
/// dialog closed.
pub fn waiting() -> &'static Mutex<VecDeque<Ask>> {
    #[cfg(any(test, feature = "test-support"))]
    {
        // `os()` first: on a test build it installs the test services if
        // nothing has yet, so which queue an ask lands in never depends on
        // whether an earlier read happened to install them.
        crate::contract::os();
        if crate::contract::testing::active() {
            thread_local! {
                static WAITING: &'static Mutex<VecDeque<Ask>> = Box::leak(Box::default());
            }
            return WAITING.with(|waiting| *waiting);
        }
    }
    static WAITING: OnceLock<Mutex<VecDeque<Ask>>> = OnceLock::new();
    WAITING.get_or_init(Mutex::default)
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
/// The identity ops that remove, suspend or stop running are asked in
/// this app's own words, whoever asks. Any other op to a role's program is asked when the
/// requesting program is bound to no role, in a sentence that names the
/// role and the target's own description of the op (`described`, read by
/// the request's task).
pub fn needed(
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
    let words = |said: String, shown: Option<String>, card: Card| Words {
        title: ACCOUNT,
        said,
        id: id.clone(),
        shown,
        described: false,
        card,
    };
    let target_row = Fact::new("Program", target, true);
    // the phone's account card (mobile `src/network/consent.ts`,
    // `accountAsk`), the agent by its number
    let agent = |verb: &'static str, account: u64, warning: String, named: Vec<Fact>| Card {
        title: format!("{verb}: agent #{account}"),
        facts: [vec![target_row.clone()], named, vec![signs(own)]].concat(),
        warning,
        verb,
        danger: true,
    };
    let described = |warning: String| Card {
        title: String::new(),
        facts: vec![target_row.clone(), signs(own)],
        warning,
        verb: APPROVE,
        danger: false,
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
                    described(
                        "This phone's key signs an identity operation this app cannot read. \
                         Approve only if you meant to."
                            .into(),
                    ),
                )
            });
        };
        let asked = match op {
            identity::Op::RemoveKey { account, key } if own == Some(account) => {
                let fingerprint = noded_client::join::fingerprint(&key);
                Some(words(
                    format!(
                        "{by} asks to remove a key from your account. Approve only if you meant to."
                    ),
                    Some(fingerprint.clone()),
                    Card {
                        title: format!("Remove key: {fingerprint}"),
                        facts: vec![
                            target_row.clone(),
                            Fact::new("Key", fingerprint, true),
                            signs(own),
                        ],
                        warning: "This key stops signing for your account.".into(),
                        verb: "Remove",
                        danger: true,
                    },
                ))
            }
            identity::Op::RemoveKey { account, key } => {
                let fingerprint = noded_client::join::fingerprint(&key);
                Some(words(
                    format!(
                        "{by} asks to remove a key from agent #{account}. Approve only if you meant to."
                    ),
                    Some(format!("#{account} · {fingerprint}")),
                    Card {
                        title: format!("Remove key: {fingerprint}"),
                        facts: vec![
                            target_row.clone(),
                            Fact::new("Agent", format!("#{account}"), false),
                            Fact::new("Key", fingerprint, true),
                            signs(own),
                        ],
                        warning: format!("This key stops signing for agent #{account}."),
                        verb: "Remove",
                        danger: true,
                    },
                ))
            }
            identity::Op::Revoke { account } => Some(words(
                format!("{by} asks to revoke agent #{account}. Its keys stop working for good."),
                Some(format!("#{account}")),
                agent(
                    "Revoke",
                    account,
                    format!("Revoking agent #{account} is final: its keys stop working for good."),
                    vec![Fact::new("Agent", format!("#{account}"), false)],
                ),
            )),
            identity::Op::Suspend { account } => Some(words(
                format!(
                    "{by} asks to suspend agent #{account}. It stops acting until it is resumed."
                ),
                Some(format!("#{account}")),
                agent(
                    "Suspend",
                    account,
                    format!("Agent #{account} stops acting until it is resumed."),
                    vec![Fact::new("Agent", format!("#{account}"), false)],
                ),
            )),
            identity::Op::SetScript {
                account,
                script: None,
            } => Some(words(
                format!(
                    "{by} asks to stop the program running agent #{account}. It acts for no program until it is given a script again."
                ),
                Some(format!("#{account}")),
                // Expo asks no card for it: the account card, as Suspend's
                agent(
                    "Stop",
                    account,
                    format!(
                        "Agent #{account} acts for no program until it is given a script again."
                    ),
                    vec![Fact::new("Agent", format!("#{account}"), false)],
                ),
            )),
            identity::Op::RegisterProgram { .. }
            | identity::Op::Create { .. }
            | identity::Op::CreateAgent { .. }
            | identity::Op::AddKey { .. }
            | identity::Op::SetName { .. }
            | identity::Op::SetProfile { .. }
            | identity::Op::Resume { .. }
            | identity::Op::SetOwnership { .. }
            | identity::Op::SetScript {
                script: Some(_), ..
            } => None,
        };
        if asked.is_some() {
            return asked;
        }
    }
    // a view no vote reviewed, writing to a role
    let roles = roles.filter(|_| !bound(program))?;
    let (role, _) = roles.iter().find(|(_, bound)| *bound == target)?;
    let (title, what, phone) = match role {
        abi::role::VALIDATORS => (
            NETWORK,
            "vote in the validators' program".to_owned(),
            "votes in the validators' program".to_owned(),
        ),
        abi::role::IDENTITY => (
            ACCOUNT,
            "sign an identity operation".to_owned(),
            "signs an identity operation".to_owned(),
        ),
        other => (
            NETWORK,
            format!("sign an op on the {other} role"),
            format!("signs an op on the {other} role"),
        ),
    };
    Some(Words {
        title,
        said: format!("{by} asks your key to {what}. Approve only if you meant to."),
        id,
        shown: None,
        described: true,
        card: described(format!(
            "This phone's key {phone}. Approve only if you meant to."
        )),
    })
}

/// Whether genesis bound `program` to a role the kernel calls (`roles`, as
/// the node's programs role answers them): never while no bindings are known.
pub(super) fn bound(program: &str, roles: Option<&abi::Roles>) -> bool {
    roles.is_some_and(|roles| roles.iter().any(|(_, bound)| bound == program))
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
        let stopped = asked(&identity::Op::SetScript {
            account: 12,
            script: None,
        })
        .unwrap();
        assert!(
            stopped
                .said
                .starts_with("Program chat asks to stop the program running agent #12."),
            "{stopped:?}"
        );
        assert_eq!(stopped.shown.as_deref(), Some("#12"));
        for op in [
            identity::Op::Resume { account: 12 },
            identity::Op::SetName {
                account: 12,
                name: "x".into(),
            },
            identity::Op::CreateAgent { name: "x".into() },
            identity::Op::SetScript {
                account: 12,
                script: Some(identity::Script {
                    calls: "harness".into(),
                    config: "agents/ada.md".into(),
                }),
            },
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
        let roles = abi::Roles::from_iter([
            (abi::role::PROGRAMS, "programs-b"),
            (abi::role::VALIDATORS, "valset-b"),
            (abi::role::IDENTITY, "identity-b"),
            (abi::role::BLOBS, "blobs-b"),
            (abi::role::AWAITS, "awaits-b"),
            (abi::role::EGRESS, "egress-b"),
        ]);
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
        let unbound: abi::Roles = roles
            .iter()
            .filter(|(role, _)| *role != abi::role::EGRESS)
            .collect();
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
        let roles = abi::Roles::from_iter([
            (abi::role::PROGRAMS, "programs"),
            (abi::role::VALIDATORS, "valset"),
            (abi::role::IDENTITY, identity::PROGRAM),
            (abi::role::BLOBS, "blobs"),
            (abi::role::AWAITS, "awaits"),
            (abi::role::EGRESS, "egress"),
        ]);
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
    /// a bound role's program (egress among them), the account's title for
    /// identity and the network's for every other, and leaves the words to the
    /// target's description; its op to any other program, any op from a
    /// role's own program, and every op while no bindings are known ask
    /// nothing new. The identity ops that remove or suspend keep their own
    /// words for both.
    #[test]
    fn a_view_no_vote_reviewed_asks_for_every_op_to_a_role() {
        let roles = abi::Roles::from_iter([
            (abi::role::PROGRAMS, "programs"),
            (abi::role::VALIDATORS, "valset"),
            (abi::role::IDENTITY, identity::PROGRAM),
            (abi::role::BLOBS, "blobs"),
            (abi::role::AWAITS, "awaits"),
            (abi::role::EGRESS, "egress"),
        ]);
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
            let phone = what
                .replacen("vote", "votes", 1)
                .replacen("sign ", "signs ", 1);
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
                    card: Card {
                        title: String::new(),
                        facts: vec![
                            Fact::new("Program", target, true),
                            Fact::new("Signs", "account 7", false),
                        ],
                        warning: format!("This phone's key {phone}. Approve only if you meant to."),
                        verb: APPROVE,
                        danger: false,
                    },
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

    /// Claim: a vote on a role's op reads on the card in that program's own
    /// words (halo's here), through the validators' program's description;
    /// a vote on the roles table names the role and the program it binds.
    #[test]
    fn a_role_vote_reads_in_its_programs_words_and_a_bind_names_both() {
        let open = abi::role::halo::Op::Open {
            name: "ducktape/live".into(),
            cap: abi::role::halo::Class::Client,
        };
        let vote = valset::Op::Vote(valset::Motion::Role {
            role: abi::role::HALO.into(),
            op: abi::encode(&open),
        });
        assert_eq!(
            shown(Some(&valset::describe(&vote)), 0),
            "Vote · Open lane · ducktape/live\ncap Client"
        );
        let bind = valset::Op::Vote(valset::Motion::Bind {
            role: abi::role::EGRESS.into(),
            program: Some("gateway".into()),
        });
        assert_eq!(
            shown(Some(&valset::describe(&bind)), 0),
            "Vote · bind egress to gateway\nrole egress · program gateway"
        );
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

    /// Claim: under touch the card is the phone's (mobile
    /// `src/network/consent.ts`): "Suspend: agent #12", the agent by its
    /// number (the host has no name for it), the program, the agent, who
    /// signs, what it takes away, and the verb in danger; an op a role's
    /// program describes is titled and listed in that program's words.
    #[test]
    fn the_phones_card_words_the_op_as_the_phone_does() {
        use describe::{Value, field};
        let card = |op: identity::Op, own: Option<u64>| {
            needed("chat", None, identity::PROGRAM, &abi::encode(&op), own)
                .unwrap()
                .card
        };
        let program = Fact::new("Program", identity::PROGRAM, true);
        let agent = Fact::new("Agent", "#12", false);
        assert_eq!(
            card(identity::Op::Suspend { account: 12 }, Some(7)),
            Card {
                title: "Suspend: agent #12".into(),
                facts: vec![
                    program.clone(),
                    agent.clone(),
                    Fact::new("Signs", "account 7", false)
                ],
                warning: "Agent #12 stops acting until it is resumed.".into(),
                verb: "Suspend",
                danger: true,
            }
        );
        assert_eq!(
            card(identity::Op::Revoke { account: 12 }, None),
            Card {
                title: "Revoke: agent #12".into(),
                facts: vec![
                    program.clone(),
                    agent,
                    Fact::new("Signs", "this phone", false)
                ],
                warning: "Revoking agent #12 is final: its keys stop working for good.".into(),
                verb: "Revoke",
                danger: true,
            }
        );
        let roles = abi::Roles::from_iter([(abi::role::PROGRAMS, "programs")]);
        let ask = || needed("explorer", Some(&roles), "programs", &[1, 2, 3], None).unwrap();
        let mut words = ask();
        words.describe(
            Some(&describe::Description {
                title: "Schedule · chat".into(),
                fields: vec![field("height", Value::count(2442))],
            }),
            3,
        );
        assert_eq!(words.card.title, "Approve: Schedule · chat");
        assert_eq!(
            words.card.facts,
            [
                Fact::new("Program", "programs", true),
                Fact::new("Operation", "Schedule · chat", false),
                Fact::new("height", "2442", false),
                Fact::new("Signs", "this phone", false),
            ]
        );
        assert_eq!((words.card.verb, words.card.danger), (APPROVE, false));
        let mut words = ask();
        words.describe(None, 3);
        assert_eq!(
            words.card.title,
            "Approve: an operation this app cannot read"
        );
        assert_eq!(
            words.card.facts[1],
            Fact::new("Operation", "3 bytes this app cannot read", false)
        );
        // a verb that says the title's head drops it, as the phone's does
        assert_eq!(ask_title("Schedule", "Schedule · chat"), "Schedule: chat");
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
            card: Card::default(),
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
