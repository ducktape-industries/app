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
//! view's manifest, memory budget and fuel per tick, its requests routed as
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
use super::*;
use crate::backend::views::Program;
use notify::{Permission, Settings};

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

/// The notifiers running, by program.
#[derive(Default)]
pub(super) struct Notifiers {
    running: Mutex<HashMap<&'static str, Running>>,
}

/// The app's notifiers.
pub(super) fn notifiers() -> &'static Notifiers {
    static NOTIFIERS: OnceLock<Notifiers> = OnceLock::new();
    NOTIFIERS.get_or_init(Notifiers::default)
}

impl Notifiers {
    /// After a roster read on `asked_of`: one notifier for each of
    /// `programs` the person has not blocked, on that connection, running
    /// the code listed now. A program blocked or gone, a code moved and an
    /// older connection stop theirs. A program that ships none has a thread
    /// that reads its blob and ends.
    pub(super) fn reconcile(&self, asked_of: &Connection, programs: &[Program]) {
        let blocked = blocked();
        let wanted: HashMap<&'static str, abi::BlobId> = programs
            .iter()
            .filter(|program| !program.bare && !blocked.contains(&program.name))
            .map(|program| (intern(&program.name), program.code))
            .collect();
        let mut running = lock(&self.running);
        running.retain(|program, run| {
            run.rev == asked_of.rev && wanted.get(program) == Some(&run.code)
        });
        for (program, code) in wanted {
            if !running.contains_key(program) {
                running.insert(program, start(program, code, asked_of.clone()));
            }
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
    Settings::kept()
        .views
        .into_iter()
        .filter(|(_, permission)| *permission == Permission::Block)
        .map(|(program, _)| program)
        .collect()
}

/// The app left the network: every notifier stops.
pub(crate) fn disconnected() {
    notifiers().stop_all();
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

/// `program`'s notifier out of `code`, on a thread of its own, until its
/// stop.
fn start(program: &'static str, code: abi::BlobId, asked_of: Connection) -> Running {
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let rev = asked_of.rev;
    // a test's prefs are its own thread's (`backend::prefs`): the notifier
    // it starts reads the same
    #[cfg(any(test, feature = "test-support"))]
    let prefs = crate::backend::read_prefs().unwrap_or_default();
    let spawned = std::thread::Builder::new()
        .name(format!("notifier {program}"))
        .spawn(move || {
            #[cfg(any(test, feature = "test-support"))]
            crate::backend::edit_prefs(|kept| *kept = prefs);
            let Some(notifier) = load(program, &code, &asked_of) else {
                return;
            };
            drive(notifier, stopped);
        });
    if let Err(error) = spawned {
        tracing::warn!(target: "ducktape::app", program, %error, "no thread to run the notifier on");
    }
    Running { code, rev, stop }
}

/// The notifier `code`'s blob carries, started (`init`): none when the
/// blob carries none, or it does not load, which app.log says.
fn load(program: &'static str, code: &abi::BlobId, asked_of: &Connection) -> Option<View> {
    let client = asked_of.client.as_ref()?;
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
fn drive(mut notifier: View, mut stopped: tokio::sync::watch::Receiver<bool>) {
    let mut answered = notifier.replies.changes();
    let mut session = props().subscribe();
    loop {
        if *stopped.borrow_and_update() {
            return;
        }
        answered.borrow_and_update();
        let current = session.borrow_and_update().clone();
        let again = redrawn(&mut notifier, &current);
        for intent in std::mem::take(&mut notifier.intents) {
            said(notifier.program, intent);
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

type Said = futures::channel::mpsc::UnboundedSender<(&'static str, Intent)>;

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
