//! Each program's NOTIFIER: the headless module its code blob carries as
//! the `ducktape.notify` section (`backend::views::NOTIFY_SECTION`; the
//! programs SDK's `view::Notifier`). The program decides there what becomes
//! a notice, for whom, in what words, under what tag and link, and when it
//! is taken down; the app runs it and carries what it asks, as it does a
//! view's. No program is named here, and no notice is judged here.
//!
//! One runs per program the network in hand lists, whatever panes are
//! open, on a thread of its own: never on the window thread, never on the
//! kernel's runtime. It is a [`View`] with no tree (`View::notifier`),
//! seated by the shell's runner as a view is (`contract::ViewInstance`: a
//! module with no `snapshot`/`restore`, its tick answering a turn): a
//! view's manifest and memory budget, its requests routed as
//! a view's (`View::answer`, the kernel contract), its props the session
//! every view gets. Its manifest may not declare what needs the person in
//! front of it ([`UNATTENDED`]): it is refused before it starts.
//!
//! - A roster read starts one for each listed program whose blob carries
//!   the section, and replaces one whose program's code moved.
//! - It stops when its program leaves the roster, on a connect (the read
//!   after it starts each again, on the new node: what came meanwhile is
//!   the notifier's to catch up on, from what it keeps in its store), when
//!   the app leaves the network ([`disconnected`]), and when the person
//!   blocks its program's notices ([`permission_moved`]); one let through
//!   again starts.
//! - What it asks the app to move (a badge, a notice posted) reaches the
//!   window thread over [`intents`], to `Seats`.
//!
//! A host with no screen (`crates/pushd`) runs the same notifiers for the
//! accounts it carries notices for: each a [`Recipient`] ([`notify_for`]),
//! whose notifiers have that account's session, store, centre and channel
//! where the app's have the person's at this device. The program still
//! decides every notice; the recipient only says whose they are and where
//! they go.
use super::*;
use crate::backend::views::Program;
use crate::contract::Banners;
use notify::{CenterHandle, Permission, Settings};

/// The instance a notifier's requests and perf samples go under: no pane's.
pub(crate) const INSTANCE: u64 = u64::MAX;

/// What a notifier may not declare: it runs with no one in front of it, so
/// nothing a person's press or yes admits (`link.open`, the clipboard, an
/// op, an invite), and it draws nothing (`web`).
const UNATTENDED: [Capability; 5] = [
    Capability::Op,
    Capability::Invite,
    Capability::Link,
    Capability::Clipboard,
    Capability::Web,
];

/// A busy notifier's next turn, as a busy view's (`seat/entity.rs`).
const BUSY_TURN: Duration = Duration::from_millis(16);

/// One running notifier: the code it runs, the connection it runs on, and
/// its stop.
struct Running {
    code: abi::BlobId,
    rev: u64,
    stop: tokio::sync::watch::Sender<bool>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

/// Whom a set of notifiers runs for when it is not the person at this
/// device: an account a host with no screen carries notices for
/// (`crates/pushd`). Each field is what the app keeps once per process for
/// the person in front of it; the kernel reads it off each of the set's
/// notifiers ([`View::recipient`]).
pub struct Recipient {
    /// The session its notifiers are handed (`host.session`), as
    /// [`props`](super::props) encodes it; the app's is the one `Seats`
    /// hands every pane.
    pub session: tokio::sync::watch::Sender<Option<Vec<u8>>>,
    /// Where its notifiers' store files go, `<store>/<chain>/<program>.borsh`;
    /// the app's are under `<config>/store`.
    pub store: PathBuf,
    /// Its notification centre: the fold by tag, the burst limit.
    pub center: CenterHandle,
    /// Its word on notices, as the prefs hold the person's.
    pub settings: Settings,
    /// Where a notice its centre lets through goes; the app's go to the OS.
    pub banners: Arc<dyn Banners>,
    /// What its notifiers ask the host to move (a badge), each with its
    /// program; the app's go to `Seats` ([`intents`]).
    pub said: Said,
}

/// The notifiers running for one recipient (`None`: the person at this
/// device), by program.
#[derive(Default)]
pub(super) struct Notifiers {
    running: Mutex<HashMap<&'static str, Running>>,
    recipient: Option<Arc<Recipient>>,
}

/// The app's notifiers: the person's at this device.
pub(super) fn notifiers() -> &'static Notifiers {
    static NOTIFIERS: OnceLock<Notifiers> = OnceLock::new();
    NOTIFIERS.get_or_init(Notifiers::default)
}

/// The notifiers of each recipient a host carries notices for.
fn recipients() -> std::sync::MutexGuard<'static, Vec<Arc<Notifiers>>> {
    static RECIPIENTS: OnceLock<Mutex<Vec<Arc<Notifiers>>>> = OnceLock::new();
    lock(RECIPIENTS.get_or_init(Mutex::default))
}

/// Every listed program's notifier runs for `recipient` too, as for the
/// person at this device: started by each roster read (now, when the
/// network in hand was read), stopped and started again with a connect,
/// for as long as the [`Carried`] handed back lives.
pub fn notify_for(recipient: Recipient) -> Carried {
    let notifiers = Arc::new(Notifiers {
        running: Mutex::default(),
        recipient: Some(Arc::new(recipient)),
    });
    recipients().push(notifiers.clone());
    let asked_of = connection().lock().expect("views rpc").clone();
    if asked_of.client.is_some() {
        let programs = roster().lock().clone();
        notifiers.reconcile(&asked_of, &programs);
    }
    Carried(notifiers)
}

/// A recipient's notifiers, running while this lives: dropped (the
/// account's last device left), every one of them stops and no roster read
/// starts them again.
pub struct Carried(Arc<Notifiers>);

impl Drop for Carried {
    fn drop(&mut self) {
        recipients().retain(|set| !Arc::ptr_eq(set, &self.0));
        self.0.stop_all();
    }
}

/// After a roster read on `asked_of`: the person's notifiers, unless this
/// host has no one at it (`Preload::Headless`), and every recipient's.
pub(super) fn reconcile_all(asked_of: &Connection, programs: &[Program]) {
    if crate::contract::os().limits().preload != crate::contract::Preload::Headless {
        notifiers().reconcile(asked_of, programs);
    }
    let sets = recipients().clone();
    for set in sets {
        set.reconcile(asked_of, programs);
    }
}

/// Every notifier stopped, the person's and every recipient's.
pub(super) fn stop_every() {
    notifiers().stop_all();
    let sets = recipients().clone();
    for set in sets {
        set.stop_all();
    }
}

impl Notifiers {
    /// After a roster read on `asked_of`: one notifier for each of
    /// `programs` the person has not blocked, on that connection, running
    /// the code listed now. A program blocked or gone, a code moved and an
    /// older connection stop theirs. The ones to start are found one after
    /// another on one thread ([`start`]): a program that ships none costs a
    /// read of its blob and no thread.
    pub(super) fn reconcile(&self, asked_of: &Connection, programs: &[Program]) {
        let blocked = match &self.recipient {
            Some(recipient) => blocked_in(&recipient.settings),
            None => blocked(),
        };
        let wanted: HashMap<&'static str, abi::BlobId> = programs
            .iter()
            .filter(|program| !program.bare && !blocked.contains(&program.name))
            .map(|program| (intern(&program.name), program.code))
            .collect();
        let mut running = lock(&self.running);
        running.retain(|program, run| {
            run.rev == asked_of.rev && wanted.get(program) == Some(&run.code)
        });
        let mut starting = Vec::new();
        for (program, code) in wanted {
            if !running.contains_key(program) {
                let (stop, stopped) = tokio::sync::watch::channel(false);
                let rev = asked_of.rev;
                running.insert(program, Running { code, rev, stop });
                starting.push((program, code, stopped));
            }
        }
        if !starting.is_empty() {
            start(starting, asked_of.clone(), self.recipient.clone());
        }
    }

    /// Every notifier stopped.
    pub(super) fn stop_all(&self) {
        lock(&self.running).clear();
    }

    fn stop(&self, program: &str) {
        lock(&self.running).remove(program);
    }

    /// The programs a notifier runs for now.
    #[cfg(test)]
    pub(super) fn running(&self) -> Vec<&'static str> {
        let mut running: Vec<_> = lock(&self.running).keys().copied().collect();
        running.sort();
        running
    }
}

/// The programs whose notices the person blocked.
fn blocked() -> Vec<String> {
    blocked_in(&Settings::kept())
}

/// The programs `settings` block.
fn blocked_in(settings: &Settings) -> Vec<String> {
    settings
        .views
        .iter()
        .filter(|(_, permission)| **permission == Permission::Block)
        .map(|(program, _)| program.clone())
        .collect()
}

/// The app left the network: every notifier stops.
pub(crate) fn disconnected() {
    stop_every();
}

/// The person's word on `program`'s notices: blocked stops its notifier,
/// anything else starts it if the network in hand lists the program.
pub(crate) fn permission_moved(program: &str, permission: Permission) {
    if permission == Permission::Block {
        return notifiers().stop(program);
    }
    let asked_of = connection().lock().expect("views rpc").clone();
    if asked_of.client.is_some() {
        let programs = roster().lock().clone();
        notifiers().reconcile(&asked_of, &programs);
    }
}

/// Each of `programs`' notifiers out of its code, for `recipient` (none:
/// the person at this device), each on a thread of its own until its stop.
/// They are found and loaded one after another on one thread, so a set's
/// first read holds one blob's pieces at a time, on one thread, whatever
/// the roster lists.
fn start(
    programs: Vec<(
        &'static str,
        abi::BlobId,
        tokio::sync::watch::Receiver<bool>,
    )>,
    asked_of: Connection,
    recipient: Option<Arc<Recipient>>,
) {
    // a test's prefs are its own thread's (`backend::prefs`): the notifier
    // it starts reads the same
    #[cfg(any(test, feature = "test-support"))]
    let prefs = crate::backend::read_prefs().unwrap_or_default();
    let found = move || {
        #[cfg(any(test, feature = "test-support"))]
        crate::backend::edit_prefs(|kept| *kept = prefs.clone());
        for (program, code, stopped) in programs {
            if *stopped.borrow() {
                continue;
            }
            let Some(mut notifier) = load(program, &code, &asked_of) else {
                continue;
            };
            let session = match &recipient {
                Some(recipient) => recipient.session.subscribe(),
                None => props().subscribe(),
            };
            notifier.recipient = recipient.clone();
            #[cfg(any(test, feature = "test-support"))]
            let prefs = prefs.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("notifier {program}"))
                .spawn(move || {
                    #[cfg(any(test, feature = "test-support"))]
                    crate::backend::edit_prefs(|kept| *kept = prefs);
                    drive(notifier, stopped, session);
                });
            if let Err(error) = spawned {
                tracing::warn!(target: "ducktape::app", program, %error, "no thread to run the notifier on");
            }
        }
    };
    if let Err(error) = std::thread::Builder::new()
        .name("notifiers".into())
        .spawn(found)
    {
        tracing::warn!(target: "ducktape::app", %error, "no thread to find the notifiers on");
    }
}

/// The notifier `code`'s blob carries, started (`init`): none when the
/// blob carries none, or it does not load, which app.log says. One code
/// loads at a time ([`loading`]): started for many recipients at once, it
/// is fetched and compiled once, and the rest take it from the caches
/// (`backend::views::notifier_of`, the runner's compiled code).
fn load(program: &'static str, code: &abi::BlobId, asked_of: &Connection) -> Option<View> {
    let client = asked_of.client.as_ref()?;
    let gate = loading(code);
    let _loading = lock(&gate);
    let bytes = handle().block_on(crate::backend::views::notifier_of(client, code));
    let refused = |reason: String| {
        tracing::warn!(target: "ducktape::app", program, reason = %reason, "notifier not started");
    };
    let bytes = match bytes {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return None,
        Err(error) => {
            refused(error.to_string());
            return None;
        }
    };
    let notifier = handle().block_on(View::notifier(program, &bytes, asked_of));
    if let Err(reason) = &notifier {
        refused(reason.clone());
    }
    notifier.ok()
}

/// The load of `code`'s notifier, one at a time.
fn loading(code: &abi::BlobId) -> Arc<Mutex<()>> {
    type Gates = HashMap<abi::BlobId, std::sync::Weak<Mutex<()>>>;
    static GATES: OnceLock<Mutex<Gates>> = OnceLock::new();
    let mut gates = lock(GATES.get_or_init(Mutex::default));
    if let Some(gate) = gates.get(code).and_then(std::sync::Weak::upgrade) {
        return gate;
    }
    gates.retain(|_, gate| gate.strong_count() > 0);
    let gate = Arc::new(Mutex::new(()));
    gates.insert(*code, Arc::downgrade(&gate));
    gate
}

impl View {
    /// A notifier's module refused when it declares what needs a person in
    /// front of it ([`UNATTENDED`]), else checked and prepared as a view's
    /// is (its manifest, its wire), seated by the shell's runner on
    /// `connection` and `init`ed.
    pub(super) async fn notifier(
        program: &'static str,
        bytes: &[u8],
        connection: &Connection,
    ) -> Result<Self, String> {
        let shown = format!("{program} notifier");
        let listing = manifest_of(bytes);
        if let Some(capability) = listing
            .capabilities
            .iter()
            .find(|capability| UNATTENDED.contains(capability))
        {
            return Err(format!(
                "{shown}: a notifier runs with no one in front of it, and declares `{}`",
                capability.as_str()
            ));
        }
        let runner = crate::contract::runner();
        // held until the instance holds the code itself
        let (code, _held) =
            Self::prepare(&**runner, bytes, &shown).map_err(|failure| failure.to_string())?;
        let mut notifier = Self::seated(program, INSTANCE, runner, code, &shown).await?;
        notifier.notifier = true;
        // nothing snapshots a notifier: no replay is kept for it
        notifier.kept = None;
        notifier.connection = connection.clone();
        notifier.name = listing.name;
        notifier.capabilities = listing.capabilities;
        notifier.targets = listing.targets;
        notifier.wire = listing.wire;
        notifier.inited(&shown).await?;
        Ok(notifier)
    }
}

/// Turns `notifier` until its stop: on its first turn, each answer, each
/// session move, each clock item due, and again soon while it says busy.
/// A trap ends it, as one ends a view.
fn drive(
    mut notifier: View,
    mut stopped: tokio::sync::watch::Receiver<bool>,
    mut session: tokio::sync::watch::Receiver<Option<Vec<u8>>>,
) {
    let mut answered = notifier.replies.changes();
    loop {
        if *stopped.borrow_and_update() {
            return;
        }
        answered.borrow_and_update();
        let current = session.borrow_and_update().clone();
        let again = redrawn(&mut notifier, &current);
        for intent in std::mem::take(&mut notifier.intents) {
            match &notifier.recipient {
                Some(recipient) => {
                    let _ = recipient.said.unbounded_send((notifier.program, intent));
                }
                None => said(notifier.program, intent),
            }
        }
        if let Some(fault) = &notifier.fault {
            tracing::warn!(target: "ducktape::app", program = notifier.program,
                reason = "notifier_trapped", error = %fault, "notifier ended");
            return;
        }
        let busy = notifier.frame.busy;
        let due = kernel::next_tick(&notifier.clocks).map(tokio::time::Instant::from_std);
        handle().block_on(async {
            if again {
                if busy {
                    tokio::time::sleep(BUSY_TURN).await;
                }
                return;
            }
            tokio::select! {
                _ = answered.changed() => {}
                _ = session.changed() => {}
                _ = stopped.changed() => {}
                _ = sleep_until(due) => {}
            }
        });
    }
}

/// One redraw of `notifier`, its tick's answer awaited on this thread (it
/// lands at once on wasmtime, later on JSC): whether it is due again at
/// once.
fn redrawn(notifier: &mut View, props: &Option<Vec<u8>>) -> bool {
    match notifier.begin_redraw(props) {
        Redraw::Quiet => return false,
        Redraw::Ready => {}
        Redraw::Ticking => {
            let landed = handle().block_on(view::next(&notifier.landing()));
            notifier.redraw_landed(landed);
        }
    }
    notifier.finish_redraw(props)
}

/// Until `due`, or never.
async fn sleep_until(due: Option<tokio::time::Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

/// The session every view gets as its props, as `Seats` last encoded it:
/// a notifier is handed the same.
fn props() -> &'static tokio::sync::watch::Sender<Option<Vec<u8>>> {
    static PROPS: OnceLock<tokio::sync::watch::Sender<Option<Vec<u8>>>> = OnceLock::new();
    PROPS.get_or_init(|| tokio::sync::watch::channel(None).0)
}

/// `Seats` hands the notifiers the session it hands every seat.
pub(crate) fn set_props(session: Vec<u8>) {
    props().send_if_modified(|props| {
        let moved = props.as_ref() != Some(&session);
        *props = Some(session);
        moved
    });
}

/// Where a set of notifiers' intents go, each with its program.
pub type Said = futures::channel::mpsc::UnboundedSender<(&'static str, Intent)>;

#[cfg(not(any(test, feature = "test-support")))]
static HEARD: Mutex<Option<Said>> = Mutex::new(None);

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// Each test's `Seats` is its thread's own: one test's never replaces,
    /// and so never drops, another's (a drop wakes that test's task from
    /// this thread, which its scheduler refuses). A notifier a test starts
    /// runs on a thread of its own and so routes nothing.
    static HEARD: std::cell::RefCell<Option<Said>> = const { std::cell::RefCell::new(None) };
}

/// `change` on where the notifiers' intents go.
fn heard<R>(change: impl FnOnce(&mut Option<Said>) -> R) -> R {
    #[cfg(not(any(test, feature = "test-support")))]
    return change(&mut lock(&HEARD));
    #[cfg(any(test, feature = "test-support"))]
    HEARD.with(|heard| change(&mut heard.borrow_mut()))
}

/// What the notifiers ask the app to move, each with its program, for the
/// window thread (`Seats` routes it as a pane's). Installing it replaces
/// the one before; the app installs one.
pub(crate) fn intents() -> futures::channel::mpsc::UnboundedReceiver<(&'static str, Intent)> {
    let (send, receive) = futures::channel::mpsc::unbounded();
    heard(|heard| *heard = Some(send));
    receive
}

fn said(program: &'static str, intent: Intent) {
    heard(|heard| {
        if let Some(send) = heard {
            let _ = send.unbounded_send((program, intent));
        }
    });
}

/// A turn's answers as a notifier takes them: the borsh of the answers and
/// stream items, the only events a notifier hears (it has no tree, no
/// theme, no pane).
pub(super) fn answers(events: Vec<wire::Event>) -> Vec<u8> {
    let answers: Vec<wire::notifier::Answer> = events
        .into_iter()
        .filter_map(|event| match event {
            wire::Event::Response { id, result, done } => {
                Some(wire::notifier::Answer { id, result, done })
            }
            _ => None,
        })
        .collect();
    borsh::to_vec(&answers).expect("answers encode")
}

/// A notifier's turn, as the frame a view would have answered with nothing
/// drawn: its requests, its cancels and whether it is busy.
pub(super) fn turn(bytes: &[u8]) -> Result<(wire::Frame, wire::SanitizeReport), String> {
    let turn: wire::notifier::Turn = borsh::from_slice(bytes).map_err(|error| error.to_string())?;
    let frame = wire::Frame {
        requests: turn.requests,
        cancels: turn.cancels,
        busy: turn.busy,
        unchanged: true,
        ..wire::Frame::default()
    };
    Ok((frame, Default::default()))
}

#[cfg(test)]
mod tests;
