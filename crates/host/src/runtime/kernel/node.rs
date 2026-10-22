//! The node-backed methods and how a view request becomes a task: a
//! handler is `fn(Node, Vec<u8>) -> Answered`, run on [`handle`] with an
//! in-flight slot from [`Replies::admit`], its answer put in [`Replies`];
//! the node is asked once and a transport failure is the answer. A subscription
//! (`program.changes`, `chain.heads`) is one long task writing items. Every
//! task is a [`NodeTask`] in `view.tasks`, aborted when dropped, and answers
//! through its [`Claim`] on the request, so a reconnect that takes the
//! request from it is the only one to answer it from then on.
use super::super::{consent, picker};
use super::*;
use noded_client::wire as noded;
use view_wire::methods::Method as _;

pub mod artifact;
mod standing;

pub type Answered = std::pin::Pin<Box<dyn std::future::Future<Output = Answer> + Send>>;
/// One node-backed method: the node to ask and the request's payload in,
/// the answer out. Not a [`methods::Call`], which is `program.query`'s and
/// `op.submit`'s envelope.
type NodeMethod = fn(Node, Vec<u8>) -> Answered;

pub(in crate::runtime) type Joints = HashMap<(String, String), std::sync::Weak<standing::Joint>>;

/// The node a view's request goes to, and the network its frames name.
#[derive(Clone)]
pub(super) struct Node {
    pub(super) client: RpcClient,
    pub(super) network: String,
    pub(super) on: super::super::roster::On,
}

impl Node {
    /// The roster the request reads: its set's, or the app's.
    pub(super) fn roster(&self) -> super::super::Roster {
        self.on.roster()
    }

    /// What signs its queries: a set's, the reader (the seated key is the
    /// network in hand's); the views', the seated key or the reader.
    pub(super) fn signer(&self) -> std::sync::Arc<dyn noded_client::Signer> {
        match self.on.followed() {
            true => crate::contract::os().reader(),
            false => backend::query_signer(&self.network),
        }
    }
}

/// A refusal the transport produced, not the node's own word.
pub(super) fn transport_failed(refusal: &wire::Error) -> bool {
    matches!(
        refusal.code.as_str(),
        refusal::RPC_CLIENT | refusal::NODE_FAILED
    )
}

/// Runs `method` on the connected node as a task, asking once: a transport
/// failure is the answer, and the view decides whether to ask again.
pub(super) fn spawn_asked(
    view: &mut View,
    id: u64,
    payload: &[u8],
    method: NodeMethod,
    stage: &'static str,
) {
    spawn_method(
        view,
        id,
        payload,
        move |node, ask, _| method(node, ask),
        stage,
        None,
    );
}

/// `op.submit` with no person to ask: [`spawn_asked`] with [`submit`],
/// whose frame, once it goes out, answers in doubt if a reconnect takes
/// the request from its task ([`reconnected`]).
pub(super) fn spawn_submit(view: &mut View, id: u64, payload: &[u8]) {
    spawn_method(view, id, payload, submit, "host_call.op.submit", None);
}

/// `op.submit` of an op the person confirms first (`consent::needed`):
/// only the person's activation opens the dialog, taken here at the press
/// as `link.open` takes it (a view resubmitting after Cancel, or asking on
/// a clock, gets `needs_gesture` and no second card); refused
/// `session_locked` at once with no key to sign it (no dialog that cannot
/// lead to a signature); else its task asks the person ([`Asking`]) and
/// runs it as [`spawn_asked`] once they approve, refused
/// `consent_refused` otherwise.
pub(super) fn spawn_consented(
    view: &mut View,
    id: u64,
    payload: &[u8],
    call: methods::Call,
    words: consent::Words,
) {
    if !view.take_activation() {
        view.refuse(
            id,
            refusal::NEEDS_GESTURE,
            "this op asks the person and needs a press or key",
        );
        return;
    }
    if !backend::seated(&view.connection.network) {
        view.refuse(
            id,
            refusal::SESSION_LOCKED,
            "this device's key is locked; unlock it first",
        );
        return;
    }
    if connected(view, id).is_none() {
        return;
    }
    let asking = Asking {
        waiting: consent::waiting(),
        program: view.program,
        instance: view.instance,
        words,
        call,
    };
    spawn_method(
        view,
        id,
        payload,
        submit,
        "host_call.op.submit",
        Some(asking),
    );
}

/// A consented op's ask, made by its task: the card goes up only once its
/// words are whole, and goes with the task (aborted with its view, the ask
/// is never queued or is withdrawn).
struct Asking {
    /// the queue the request came in on (`consent::queue_in`)
    waiting: &'static consent::Queue,
    program: &'static str,
    instance: u64,
    words: consent::Words,
    call: methods::Call,
}

impl Asking {
    /// The person's yes: the op described into the words where they show
    /// the target's own description (its size when the target's describe
    /// program reads nothing or fails), the ask queued and
    /// the shell woken, and its answer. `consent_refused` for anything but
    /// Approve, and while another ask of this view waits.
    async fn yes(self, node: &Node) -> Result<(), wire::Error> {
        let Asking {
            waiting,
            program,
            instance,
            mut words,
            call,
        } = self;
        if words.described {
            let len = call.body.len();
            let described = super::describe::described(node, &call.target, call.body)
                .await
                .ok()
                .flatten();
            words.describe(described.as_ref(), len);
        }
        let refused = |message: &str| wire::Error::new(refusal::CONSENT_REFUSED, message);
        let Some(mut told) = consent::queue_in(waiting, program, instance, words) else {
            return Err(refused(
                "another confirmation for this view is still waiting",
            ));
        };
        consent::moved();
        match told.answer().await {
            Some(true) => Ok(()),
            _ => Err(refused("the person did not confirm")),
        }
    }
}

/// `stage` names the call in the perf registry (`host_call.<kind>`): its
/// latency from here to the reply. `asking`, when given, is the person's yes the task asks for before it asks the
/// node at all ([`Asking::yes`]).
#[allow(clippy::too_many_arguments, reason = "one spawn, every node method")]
fn spawn_method(
    view: &mut View,
    id: u64,
    payload: &[u8],
    method: impl FnOnce(Node, Vec<u8>, Claim) -> Answered + Send + 'static,
    stage: &'static str,
    asking: Option<Asking>,
) {
    let ask = payload.to_vec();
    let Some(node) = connected(view, id) else {
        return;
    };
    let slot = view.replies.admit();
    let replies = view.replies.clone();
    let claim = Claim::default();
    let held = claim.clone();
    let key = view.perf_key();
    // not a `perf::Timer`: its drop would record a call cancelled by the
    // view, which never got a reply
    let asked = crate::perf::on().then(std::time::Instant::now);
    run(view, id, slot, None, claim, async move {
        if let Some(asking) = asking
            && let Err(refused) = asking.yes(&node).await
        {
            replies.answer(&held, id, Err(refused), true);
            return;
        }
        let result = method(node, ask, held.clone()).await;
        if let Some(asked) = asked {
            crate::perf::record(key, stage, asked.elapsed().as_micros() as u64);
        }
        replies.answer(&held, id, result, true);
    });
}

/// Every task starts here: an in-flight slot ([`Replies::admit`]), then
/// [`run`]. The task answers request `id` through `claim`
/// ([`Replies::answer`]).
fn start(
    view: &mut View,
    id: u64,
    follows: Option<Follow>,
    claim: Claim,
    task: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let slot = view.replies.admit();
    run(view, id, slot, follows, claim, task);
}

/// `task` on [`handle`] holding its `slot`, its [`NodeTask`] kept in
/// `view.tasks` (finished ones pruned) so it is aborted with the view.
fn run(
    view: &mut View,
    id: u64,
    slot: InFlight,
    follows: Option<Follow>,
    claim: Claim,
    task: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let task = rt::task::spawn(async move {
        let _slot = slot;
        task.await;
    });
    view.tasks
        .retain(|(_, pending)| !pending.task.is_finished());
    view.tasks.push((
        id,
        NodeTask {
            task,
            follows,
            claim,
        },
    ));
}

/// One SUBSCRIPTION's writing end, handed to the loop that feeds it: every
/// item goes through the same park as a node socket's, so a source that
/// outruns the redraw waits for the redraw that takes its item.
pub(in crate::runtime) struct Items {
    replies: std::sync::Arc<Replies>,
    drained: tokio::sync::watch::Receiver<()>,
    claim: Claim,
    id: u64,
}

impl Items {
    /// One more item. `false` when the view is gone or the subscription
    /// was taken from this task (a reconnect opened it again): it should
    /// end — the caller returns, and everything it holds is dropped.
    pub(in crate::runtime) async fn send(&mut self, result: Answer) -> bool {
        self.replies
            .subscription_item(&mut self.drained, &self.claim, self.id, result)
            .await
    }

    /// The refusal that ends the subscription.
    pub(super) fn last(&self, refusal: wire::Error) {
        self.end(Err(refusal));
    }

    pub(super) fn end(&self, last: Answer) {
        self.replies.answer(&self.claim, self.id, last, true);
    }
}

pub(super) fn spawn_subscription<Body, Fut>(view: &mut View, id: u64, follows: Follow, body: Body)
where
    Body: FnOnce(Items) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let replies = view.replies.clone();
    let claim = Claim::default();
    let items = Items {
        drained: replies.drains(),
        replies,
        claim: claim.clone(),
        id,
    };
    start(view, id, Some(follows), claim, body(items));
}

/// A subscription fed by the app itself, not the node (`store.changes`,
/// `host.screen`): a reconnect leaves it running ([`Follow::Host`]).
#[cfg(test)]
pub(in crate::runtime) fn hold_for_test(
    view: &mut View,
    id: u64,
    task: impl std::future::Future<Output = ()> + Send + 'static,
) {
    view.tasks.push((
        id,
        NodeTask {
            task: rt::task::spawn(task),
            follows: None,
            claim: Claim::default(),
        },
    ));
}

pub(in crate::runtime) fn spawn_host_subscription<Body, Fut>(view: &mut View, id: u64, body: Body)
where
    Body: FnOnce(Items) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    spawn_subscription(view, id, Follow::Host, body);
}

/// Runs `future` on the kernel runtime and delivers its result as the one
/// reply to request `id`; counted in flight, aborted with the view. For a
/// host method that waits on something other than the node (`notify.post`
/// waits on its banner, `store.*` on its file).
pub(in crate::runtime) fn spawn_reply(
    view: &mut View,
    id: u64,
    future: impl std::future::Future<Output = Answer> + Send + 'static,
) {
    let replies = view.replies.clone();
    let claim = Claim::default();
    let held = claim.clone();
    start(view, id, None, claim, async move {
        replies.answer(&held, id, future.await, true);
    })
}

/// The node to ask, the one the view's own connection names, or `None`
/// with the request REFUSED: `stale_connection` for a view from before the
/// app's last (re)connect, `not_connected` without a node.
pub(super) fn connected(view: &mut View, id: u64) -> Option<Node> {
    if view.connection.stale() {
        view.refuse(
            id,
            refusal::STALE_CONNECTION,
            "view belongs to a previous network connection",
        );
        return None;
    }
    let node = node_on(&view.connection);
    if node.is_none() {
        view.refuse(id, refusal::NOT_CONNECTED, "not connected to a node");
    }
    node
}

/// The node `connection` names, if it names one.
fn node_on(connection: &super::super::Connection) -> Option<Node> {
    match (&connection.client, &connection.network) {
        (Some(client), network) if !network.is_empty() => Some(Node {
            client: client.clone(),
            network: network.clone(),
            on: connection.on.clone(),
        }),
        _ => None,
    }
}

/// A connect inside the network, under a view that kept its code
/// (`View::reconnect`): each node subscription goes on, opened again on
/// `connection`, and its follower with it — a `program.changes` one is told
/// first to re-read (`None`, as when the node closes the socket), a
/// `chain.heads` one starts over at the new node's tip. A submit whose frame
/// went out answers in doubt (`Submitted::InDoubt`: the old node may hold
/// it); every other request still running on the old connection is refused
/// `stale_connection`, for the view to ask again. A request whose task had
/// already ended it is left ended: each id ends once.
pub(in crate::runtime) fn reconnected(view: &mut View, connection: &super::super::Connection) {
    // counted again as each opens
    view.live_subscriptions.clear();
    view.joints.clear();
    for (id, mut task) in std::mem::take(&mut view.tasks) {
        // the app's own feed has no socket on the old connection
        if matches!(task.follows, Some(Follow::Host)) {
            view.tasks.push((id, task));
            continue;
        }
        // taken from the task before anything is answered for it: a
        // request its task already ended (a subscription the old node
        // ended, a reply not yet pruned) stays ended, and whatever the
        // aborting task still sends is dropped
        let Some(standing) = view.replies.take(&task.claim) else {
            continue;
        };
        let follows = task.follows.take();
        // dropped, it aborts: the old connection's socket goes with it
        drop(task);
        match (follows, node_on(connection), standing) {
            (Some(Follow::Changes(program)), Some(node), _) => {
                view.replies
                    .item(id, Ok(methods::encode(&None::<methods::Change>)), false);
                follow_changes(view, id, node, program);
            }
            (Some(Follow::Heads), Some(node), _) => follow_heads(view, id, node),
            (Some(Follow::Watch(call)), Some(node), _) => follow_watch(view, id, node, call),
            (Some(Follow::Live(follows)), Some(_), _) => super::live::reopen(view, id, follows),
            // a submit whose frame went out: the old node may hold it
            (None, _, Some(standing)) => view.reply(id, standing),
            _ => view.refuse(id, refusal::STALE_CONNECTION, "network connection changed"),
        }
    }
}

/// `program.changes <program>`: one item per block that wrote to the program,
/// carrying the keys it wrote.
pub(super) fn changes(view: &mut View, id: u64, payload: &[u8]) {
    let program = methods::decode::<String>(payload)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if program.is_empty() {
        view.refuse(
            id,
            refusal::MALFORMED_REQUEST,
            "`program.changes` names no program",
        );
        return;
    }
    let Some(node) = connected(view, id) else {
        return;
    };
    follow_changes(view, id, node, program);
}

enum Heard {
    Back,
    /// It is no longer wanted: it lets go of the feed, and the last
    /// follower gone, the socket closes.
    Away,
    /// The feed's next item for the view.
    Item(Answer),
}

fn follow_changes(view: &mut View, id: u64, node: Node, program: String) {
    standing::join(view, id, node, program.clone(), None);
    view.live_subscriptions.push((id, program));
}

pub(super) fn watch(view: &mut View, id: u64, payload: &[u8]) {
    let Some(call) = enveloped(view, id, payload) else {
        return;
    };
    let Some(node) = connected(view, id) else {
        return;
    };
    follow_watch(view, id, node, call);
}

fn follow_watch(view: &mut View, id: u64, node: Node, call: methods::Call) {
    standing::join(view, id, node, call.target.clone(), Some(call));
}

fn follow(view: &mut View, id: u64, node: Node) {
    let replies = view.replies.clone();
    let mut wanted = Wanted::of(view);
    // joined before the socket can send anything, where it may join now
    let mut joined = wanted.now().then(|| heads_feed(&node.client));
    let claim = Claim::default();
    let held = claim.clone();
    start(view, id, Some(Follow::Heads), claim, async move {
        let mut drained = replies.drains();
        loop {
            let heard = match &joined {
                None => {
                    wanted.until(true).await;
                    Heard::Back
                }
                Some((_, slot)) => tokio::select! {
                    item = slot.take() => Heard::Item(item),
                    () = wanted.until(false) => Heard::Away,
                },
            };
            let item = match heard {
                Heard::Away => {
                    joined = None;
                    continue;
                }
                Heard::Back => {
                    joined = Some(heads_feed(&node.client));
                    continue;
                }
                // a node that did not take the socket ends the subscription with why
                Heard::Item(Err(refused)) => {
                    replies.answer(&held, id, Err(refused), true);
                    return;
                }
                Heard::Item(item) => item,
            };
            // a view that stops taking items (its app behind another)
            // holds no socket while it does not
            let told = tokio::select! {
                told = replies.subscription_item(&mut drained, &held, id, item) => told,
                () = wanted.until(false) => {
                    joined = None;
                    continue;
                }
            };
            if !told {
                return;
            }
        }
    });
}

/// `program.changes`' word to re-read everything (`None`).
fn reread() -> Vec<u8> {
    methods::encode(&None::<methods::Change>)
}

/// When a view's node subscription holds its socket ([`wanted`]), as
/// its app and its screen move. A phone behind another
/// app, or on a tab whose panes are seated but not shown, asks the node
/// nothing for them: its notices come by push. A desktop pane keeps its
/// feeds while it is presented, focused or not.
struct Wanted {
    front: tokio::sync::watch::Receiver<bool>,
    /// the view's screen, where it gates the feeds
    /// (`OsFacts::feeds_follow_screen`; never a notifier's)
    on_screen: Option<tokio::sync::watch::Receiver<bool>>,
    presented: Option<tokio::sync::watch::Receiver<bool>>,
}

impl Wanted {
    fn of(view: &View) -> Self {
        let gated = crate::contract::os().facts().feeds_follow_screen && !view.notifier;
        Self {
            front: view.in_front.clone(),
            on_screen: gated.then(|| view.on_screen.subscribe()),
            presented: (!view.notifier).then(|| view.presented.subscribe()),
        }
    }

    fn now(&self) -> bool {
        let shown = self.on_screen.as_ref().map(|shown| *shown.borrow());
        let presented = self.presented.as_ref().map(|presented| *presented.borrow());
        wanted(*self.front.borrow(), shown, presented)
    }

    /// Once it is wanted (`true`), or once it no longer is.
    async fn until(&mut self, wanted: bool) {
        while self.now() != wanted {
            let Self {
                front,
                on_screen,
                presented,
            } = self;
            // a value whose sender is gone moves no more
            let front = async {
                if front.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! {
                () = front => {}
                () = moved(on_screen) => {}
                () = moved(presented) => {}
            }
        }
    }
}

async fn moved(watched: &mut Option<tokio::sync::watch::Receiver<bool>>) {
    let moved = match watched {
        Some(watched) => watched.changed().await.is_ok(),
        None => false,
    };
    if !moved {
        std::future::pending::<()>().await;
    }
}

/// Whether a view's node subscription is wanted now: while the app is in
/// front, where the view's screen gates it while that is on screen, and while it is presented.
fn wanted(in_front: bool, on_screen: Option<bool>, presented: Option<bool>) -> bool {
    in_front && on_screen.unwrap_or(true) && presented.unwrap_or(true)
}

struct Feed<T, S> {
    followers: Followers<T>,
    shared: S,
    socket: rt::task::Task,
}

type Followers<T> = std::sync::Arc<Mutex<Fanout<T>>>;

struct Fanout<T> {
    slots: Vec<std::sync::Weak<Slot<T>>>,
    newest: Option<T>,
    ended: bool,
}

impl<T: Clone> Fanout<T> {
    fn send(&mut self, item: T) {
        self.slots.retain(|follower| match follower.upgrade() {
            Some(slot) => {
                slot.offer(item.clone());
                true
            }
            None => false,
        });
    }
}

type Merge<T> = fn(Option<T>, T) -> T;

struct Slot<T> {
    pending: Mutex<Option<T>>,
    filled: tokio::sync::Notify,
    merge: Merge<T>,
}

impl<T> Slot<T> {
    fn merging(merge: Merge<T>) -> Self {
        Slot {
            pending: Mutex::new(None),
            filled: tokio::sync::Notify::new(),
            merge,
        }
    }

    fn offer(&self, item: T) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *pending = Some((self.merge)(pending.take(), item));
        self.filled.notify_one();
    }

    async fn take(&self) -> T {
        loop {
            let pending = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(item) = pending {
                return item;
            }
            self.filled.notified().await;
        }
    }
}

/// What a slot holds once `item` comes to it holding `pending`: the item,
/// when the view took the last; a refusal, which ends the subscription,
/// whatever else is there; otherwise the word to re-read (`None`), for the
/// view to read what moved since it last took one.
fn coalesced(pending: Option<Answer>, item: Answer) -> Answer {
    match (pending, item) {
        (None, item) => item,
        (Some(Err(refused)), _) | (_, Err(refused)) => Err(refused),
        (Some(Ok(_)), Ok(_)) => Ok(reread()),
    }
}

fn newest(pending: Option<Answer>, item: Answer) -> Answer {
    match (pending, item) {
        (Some(Err(refused)), _) => Err(refused),
        (_, item) => item,
    }
}

impl<T, S> Drop for Feed<T, S> {
    fn drop(&mut self) {
        self.socket.abort();
    }
}

pub(crate) struct Writes {
    heard: tokio::sync::mpsc::Receiver<()>,
    follower: rt::task::Task,
}

impl Writes {
    pub(crate) async fn next(&mut self) -> Option<()> {
        self.heard.recv().await
    }

    #[cfg(test)]
    pub(crate) fn idle() -> (tokio::sync::mpsc::Sender<()>, Self) {
        let (tell, heard) = tokio::sync::mpsc::channel(1);
        let follower = rt::task::spawn(std::future::pending());
        (tell, Self { heard, follower })
    }
}

impl Drop for Writes {
    fn drop(&mut self) {
        self.follower.abort();
    }
}

pub(crate) fn writes(client: &RpcClient, program: &str) -> Writes {
    let (tell, heard) = tokio::sync::mpsc::channel(1);
    let (feed, slot) = changes_feed(client, program);
    let follower = rt::task::spawn(async move {
        let _feed = feed;
        loop {
            match slot.take().await.change {
                Some(Ok(_)) => {
                    let _ = tell.try_send(());
                }
                Some(Err(_)) => return,
                None => {}
            }
        }
    });
    Writes { heard, follower }
}

type Feeds<K, T, S> = HashMap<K, std::sync::Weak<Feed<T, S>>>;

type Changes = (
    std::sync::Arc<Feed<standing::Delivery, std::sync::Arc<standing::Standings>>>,
    std::sync::Arc<Slot<standing::Delivery>>,
);

type Heads = (
    std::sync::Arc<Feed<Answer, ()>>,
    std::sync::Arc<Slot<Answer>>,
);

fn changes_feed(client: &RpcClient, program: &str) -> Changes {
    type Kept = Feeds<(String, String), standing::Delivery, std::sync::Arc<standing::Standings>>;
    static FEEDS: OnceLock<Mutex<Kept>> = OnceLock::new();
    let key = (client.endpoint().to_owned(), program.to_owned());
    let open = |followers| {
        let standings = std::sync::Arc::new(standing::Standings::default());
        let socket = standing::socket(
            client.clone(),
            program.to_owned(),
            followers,
            standings.clone(),
        );
        (standings, rt::task::spawn(socket))
    };
    joined(
        FEEDS.get_or_init(Mutex::default),
        key,
        standing::delivered,
        open,
    )
}

fn heads_feed(client: &RpcClient) -> Heads {
    static FEEDS: OnceLock<Mutex<Feeds<String, Answer, ()>>> = OnceLock::new();
    let open = |followers| ((), rt::task::spawn(heads_socket(client.clone(), followers)));
    joined(
        FEEDS.get_or_init(Mutex::default),
        client.endpoint().to_owned(),
        newest,
        open,
    )
}

fn joined<K: std::hash::Hash + Eq, T: Clone, S>(
    feeds: &Mutex<Feeds<K, T, S>>,
    key: K,
    merge: Merge<T>,
    open: impl FnOnce(Followers<T>) -> (S, rt::task::Task),
) -> (std::sync::Arc<Feed<T, S>>, std::sync::Arc<Slot<T>>) {
    let mut feeds = feeds
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let slot = std::sync::Arc::new(Slot::merging(merge));
    let follower = std::sync::Arc::downgrade(&slot);
    let running = feeds
        .get(&key)
        .and_then(std::sync::Weak::upgrade)
        .filter(|feed| !feed.socket.is_finished());
    if let Some(feed) = running {
        let mut fanout = feed
            .followers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !fanout.ended {
            if let Some(item) = fanout.newest.clone() {
                slot.offer(item);
            }
            fanout.slots.push(follower);
            drop(fanout);
            return (feed, slot);
        }
    }
    feeds.retain(|_, feed| feed.strong_count() > 0);
    let followers: Followers<T> = std::sync::Arc::new(Mutex::new(Fanout {
        slots: vec![follower],
        newest: None,
        ended: false,
    }));
    let (shared, socket) = open(followers.clone());
    let feed = std::sync::Arc::new(Feed {
        followers,
        shared,
        socket,
    });
    feeds.insert(key, std::sync::Arc::downgrade(&feed));
    (feed, slot)
}

/// Whether the node itself turned the changes socket down (its refusal, or
/// a 4xx), rather than not being reached: a 5xx is a proxy or a node that
/// is away, and the socket is opened again.
fn node_refuses(error: &noded_client::Error) -> bool {
    use noded_client::Error;
    match error {
        Error::Refused(_) | Error::Decode(_) => true,
        Error::Failed { status, .. } => (400..500).contains(status),
        Error::Unreachable(_) | Error::Transport(_) => false,
    }
}

/// The node's block time once its status answers: read at once, then at
/// half `block_time_ms` (the one it last gave) until it does. A node that
/// never gave one has no pace to wait at: its first unanswered read is why.
async fn answering(client: &RpcClient, block_time_ms: Option<u64>) -> noded_client::Result<u64> {
    loop {
        let error = match client.status().await {
            Ok(status) => return Ok(status.block_time_ms),
            Err(error) => error,
        };
        let Some(block_time_ms) = block_time_ms else {
            return Err(error);
        };
        tracing::debug!(target: "ducktape::app", %error, "status not answered");
        rt::time::sleep(std::time::Duration::from_millis(block_time_ms / 2)).await;
    }
}

pub(super) fn heads(view: &mut View, id: u64, payload: &[u8]) {
    if !payload.is_empty() {
        view.refuse(
            id,
            refusal::MALFORMED_REQUEST,
            "`chain.heads` takes no payload",
        );
        return;
    }
    let Some(node) = connected(view, id) else {
        return;
    };
    follow_heads(view, id, node);
}

fn follow_heads(view: &mut View, id: u64, node: Node) {
    follow(view, id, node);
}

fn end<T: Clone>(followers: &Followers<T>, item: T) {
    let mut fanout = followers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    fanout.ended = true;
    fanout.send(item);
}

fn tell_newest(followers: &Followers<Answer>, head: noded::Head) {
    let item = Ok(methods::encode(&methods::Head {
        height: head.height,
        time: head.time,
        id: head.id,
    }));
    let mut fanout = followers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    fanout.newest = Some(item.clone());
    fanout.send(item);
}

async fn heads_socket(client: RpcClient, followers: Followers<Answer>) {
    use futures::StreamExt as _;
    let mut block_time_ms = None;
    let mut away = false;
    loop {
        let lost = match client.heads().await {
            Ok(mut heads) => {
                away = false;
                if block_time_ms.is_none() {
                    block_time_ms = client
                        .status()
                        .await
                        .ok()
                        .map(|status| status.block_time_ms);
                }
                while let Some(Ok(head)) = heads.next().await {
                    tell_newest(&followers, head);
                }
                "the socket closed".to_owned()
            }
            Err(error) if node_refuses(&error) => {
                tracing::debug!(target: "ducktape::app", %error, "heads stream not opened");
                end(&followers, Err(refused(error)));
                return;
            }
            Err(error) => error.to_string(),
        };
        if !std::mem::replace(&mut away, true) {
            tracing::warn!(target: "ducktape::app", reason = lost,
                "heads stream lost: it opens again once the node answers");
        }
        match answering(&client, block_time_ms).await {
            Ok(answered) => block_time_ms = Some(answered),
            Err(error) => {
                end(&followers, Err(refused(error)));
                return;
            }
        }
    }
}

/// A running request or subscription; dropping it aborts the task, which is
/// how a cancel, a swap or a teardown stops a socket waiting on the node.
pub(in crate::runtime) struct NodeTask {
    task: rt::task::Task,
    /// What a node subscription follows, which [`reconnected`] opens again
    /// on a new connection; `None` for a request answered once.
    follows: Option<Follow>,
    /// The task's hold on its request, which a reconnect takes from it.
    claim: Claim,
}

/// A subscription, as a reconnect opens it again.
pub(super) enum Follow {
    /// `program.changes` of this program.
    Changes(String),
    Watch(methods::Call),
    /// `chain.heads`.
    Heads,
    /// A `live.*` request (`live.rs`).
    Live(super::live::LiveFollow),
    /// The app's own feed: it goes on as it is.
    Host,
}

impl Drop for NodeTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn call_of(ask: &[u8]) -> Result<methods::Call, wire::Error> {
    let call: methods::Call = methods::decode(ask).map_err(malformed)?;
    let target = call.target.trim();
    match program::is_id(target) {
        true => Ok(methods::Call {
            target: target.to_owned(),
            body: call.body,
        }),
        false => Err(malformed("request names no target")),
    }
}

/// The envelope of `program.query` or `op.submit` from `view`; `None`
/// with the request refused `malformed_request` when it names no
/// program. Any program may be named: whether a view may read or write
/// it is that program's to decide, never the view's manifest's. The task
/// decodes the envelope again: a string and the bytes, cheaper than
/// carrying it.
pub(super) fn enveloped(view: &mut View, id: u64, payload: &[u8]) -> Option<methods::Call> {
    match call_of(payload) {
        Ok(call) => Some(call),
        Err(refusal) => {
            view.reply(id, Err(refusal));
            None
        }
    }
}

pub(super) fn query(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let call = call_of(&ask)?;
        backend::signed_query(
            &node.client,
            backend::Layer::Confirmed,
            &*node.signer(),
            &node.network,
            &call.target,
            call.body,
        )
        .await
    })
}

/// `op.submit`; `claim` is its request's, which it leaves in doubt once
/// the frame goes out ([`submitted`]).
pub(super) fn submit(node: Node, ask: Vec<u8>, claim: Claim) -> Answered {
    Box::pin(async move {
        let call = call_of(&ask)?;
        submitted(node, call.target, call.body, claim).await
    })
}

/// What the node made of the frame, as `methods::Submitted`: every arm
/// carries the frame's id (`Body::id`) and the last height that may carry
/// it (`Body::until`). The node's preconfirm receipt is `Applied` or
/// `Refused` (pooled, the refusal a prediction). An answer that went
/// missing after the bytes went out (the transport, a status this client
/// does not read, a reply it cannot decode) is `InDoubt`: the node may hold
/// the frame. `Err` is only for a frame nothing pooled: unsigned, unsent
/// (`rpc_client`), or refused outright by the node.
async fn submitted(node: Node, target: String, payload: Vec<u8>, claim: Claim) -> Answer {
    let frame = backend::seated_frame(&node.client, &node.network, &target, payload)
        .await
        .map_err(|refusal| match transport_failed(&refusal) {
            true => wire::Error::new(refusal::RPC_CLIENT, refusal.message),
            false => refusal,
        })?;
    let (id, until) = (frame.body.id(), frame.body.until());
    // from here the node may hold the frame: a request taken from this
    // task now (a reconnect) answers in doubt, as a lost answer does
    claim.stand(Ok(methods::encode(&methods::Submitted::InDoubt {
        frame: id,
        until,
    })));
    let submitted = match node.client.preconfirm(frame.encode()).await {
        Ok(receipt) => match receipt.outcome {
            abi::Outcome::Applied { output } => methods::Submitted::Applied {
                frame: id,
                until,
                output,
            },
            abi::Outcome::Rejected(refusal) => methods::Submitted::Refused {
                frame: id,
                until,
                refusal: wire::Error::new(refusal.reason, refusal.sentence),
            },
        },
        Err(error @ (noded_client::Error::Refused(_) | noded_client::Error::Unreachable(_))) => {
            return Err(refused(error));
        }
        Err(
            noded_client::Error::Transport(_)
            | noded_client::Error::Failed { .. }
            | noded_client::Error::Decode(_),
        ) => methods::Submitted::InDoubt { frame: id, until },
    };
    Ok(methods::encode(&submitted))
}

/// One blob of an `op.put`, checked at the ask: its kind, and its bytes,
/// read where they are each time they are needed.
pub(super) struct Blob {
    kind: String,
    body: Body,
}

enum Body {
    Bytes(Vec<u8>),
    /// a slice of a file the view holds, within its size as held
    Picked {
        file: std::sync::Arc<picker::Picked>,
        offset: u64,
        len: u64,
    },
}

impl Blob {
    /// The bytes of its body, as `op.put`'s `Sent` counts them.
    fn len(&self) -> u64 {
        match &self.body {
            Body::Bytes(bytes) => bytes.len() as u64,
            Body::Picked { len, .. } => *len,
        }
    }

    /// The blob framed (`kind len\0body`), in parts: its header, then its
    /// body, a picked slice read on a blocking thread one part ahead of
    /// the parts taken (`picker::read`), so no blob is held whole. A file
    /// whose size changed since the pick is refused `host_fault`.
    fn framed(&self) -> futures::stream::BoxStream<'static, Result<Vec<u8>, wire::Error>> {
        use futures::StreamExt as _;
        let header =
            futures::stream::once(std::future::ready(Ok(blob::header(&self.kind, self.len()))));
        match &self.body {
            Body::Bytes(bytes) => {
                let body = futures::stream::once(std::future::ready(Ok(bytes.clone())));
                header.chain(body).boxed()
            }
            Body::Picked { file, offset, len } => {
                let (file, offset, len) = (file.clone(), *offset, *len);
                let (parts, taken) = tokio::sync::mpsc::channel(1);
                rt::task::spawn(async move {
                    let _ = rt::task::blocking(move || {
                        let read = picker::read(&file, offset, len, |part| {
                            parts.blocking_send(Ok(part.to_vec())).is_ok()
                        });
                        if let Err(refusal) = read {
                            let _ = parts.blocking_send(Err(refusal));
                        }
                    })
                    .await;
                });
                let body = futures::stream::unfold(taken, |mut taken| async move {
                    taken.recv().await.map(|part| (part, taken))
                });
                header.chain(body).boxed()
            }
        }
    }
}

/// `op.put`'s blobs, checked at the ask: no blob, a kind the node does
/// not take, or a slice of no file the view holds (or past its end) is
/// `malformed_request`.
fn blobs_of(view: &View, payload: &[u8]) -> Result<Vec<Blob>, wire::Error> {
    let put: methods::PutBlobs = methods::decode(payload).map_err(malformed)?;
    if put.blobs.is_empty() {
        return Err(malformed("`op.put` names no blob"));
    }
    let blob = |methods::NewBlob { kind, body }| {
        if !blob::is_kind(&kind) {
            return Err(malformed(
                "a blob's kind is one word, without spaces or NUL",
            ));
        }
        let body = match body {
            methods::BlobBody::Bytes(bytes) => Body::Bytes(bytes),
            methods::BlobBody::Picked { file, offset, len } => {
                let picked = view
                    .picker
                    .picked(file)
                    .ok_or_else(|| malformed(format!("file {file} is not one the view holds")))?;
                if offset.checked_add(len).is_none_or(|end| end > picked.size) {
                    return Err(malformed(format!(
                        "{len} bytes from {offset} run past file {file}'s {} bytes",
                        picked.size
                    )));
                }
                Body::Picked {
                    file: picked,
                    offset,
                    len,
                }
            }
        };
        Ok(Blob { kind, body })
    };
    put.blobs.into_iter().map(blob).collect()
}

/// `op.put`: its blobs checked here ([`blobs_of`]), then put by its task
/// ([`put`]), which goes with the view as every node task does. A
/// subscription: each `Sent` is an item as the upload takes the bytes (not
/// parked for the redraw, which would hold the upload up while no window
/// draws the view), then `Held` ends it. A reconnect refuses it
/// `stale_connection`, as any request on the old node.
pub(super) fn spawn_put(view: &mut View, id: u64, payload: &[u8]) {
    let blobs = match blobs_of(view, payload) {
        Ok(blobs) => blobs,
        Err(refusal) => return view.reply(id, Err(refusal)),
    };
    let replies = view.replies.clone();
    let putting = move |node, _, held: Claim| -> Answered {
        let sent = move |bytes| {
            let item = methods::encode(&methods::Putting::Sent(bytes));
            replies.answer(&held, id, Ok(item), false);
        };
        Box::pin(put(node, blobs, sent))
    };
    spawn_method(view, id, &[], putting, "host_call.op.put", None);
}

/// Whether the view's request `id` can reach the node now, refusing it
/// when not ([`connected`]): asked before a dialog opens for it.
#[cfg(feature = "ui")]
pub(in crate::runtime) fn reaches_node(view: &mut View, id: u64) -> bool {
    connected(view, id).is_some()
}

/// `file.save`: once `chosen` answers the file the person chose, its task
/// ([`saved`]) writes the blobs into it, as a node task that goes with the
/// view.
#[cfg(feature = "ui")]
pub(in crate::runtime) fn spawn_save(
    view: &mut View,
    id: u64,
    save: methods::SaveFile,
    chosen: impl std::future::Future<Output = Result<Option<std::path::PathBuf>, wire::Error>>
    + Send
    + 'static,
) {
    let save = move |node, _, _| -> Answered { Box::pin(saved(node, save, chosen)) };
    spawn_method(view, id, &[], save, "host_call.file.save", None);
}

/// `file.save` on a shell that keeps files its own way
/// (`contract::Files::keep`): the blobs written to `path`, in a folder of
/// its own, as [`saved`] writes them, then the file handed to the shell;
/// `true` once the person kept it, `false` when they cancelled. The folder
/// goes once the shell answers, whatever it answered.
#[cfg(feature = "ui")]
pub(in crate::runtime) fn spawn_keep(
    view: &mut View,
    id: u64,
    save: methods::SaveFile,
    path: std::path::PathBuf,
    files: &'static dyn crate::contract::Files,
) {
    let keep = move |node, _, _| -> Answered { Box::pin(kept(node, save, path, files)) };
    spawn_method(view, id, &[], keep, "host_call.file.save", None);
}

/// [`spawn_keep`]'s task.
#[cfg(feature = "ui")]
async fn kept(
    node: Node,
    save: methods::SaveFile,
    path: std::path::PathBuf,
    files: &'static dyn crate::contract::Files,
) -> Answer {
    let folder = path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let made = folder.clone();
    rt::task::blocking(move || std::fs::create_dir_all(made))
        .await
        .map_err(host_fault)?
        .map_err(host_fault)?;
    let folder = crate::runtime::picker::KeptFolder(folder);
    let written = path.clone();
    match saved(node, save, async move { Ok(Some(written)) }).await {
        Ok(_) => crate::runtime::picker::keep(files, &path, folder).await,
        Err(refusal) => Err(refusal),
    }
}

/// `file.save`'s task: the file the person chose (none: cancelled, `false`),
/// and each blob of `save` read from the node in order and written on a
/// blocking thread to a new file beside it, which takes its place once
/// every one is written (`true`). A save that fails or goes with the view
/// leaves what was there as it was, and nothing beside it. A blob the node
/// does not hold is refused `node_failed`, a file that cannot be written
/// `host_fault`, naming it by its name alone.
#[cfg(feature = "ui")]
async fn saved(
    node: Node,
    save: methods::SaveFile,
    chosen: impl std::future::Future<Output = Result<Option<std::path::PathBuf>, wire::Error>>,
) -> Answer {
    let Some(path) = chosen.await? else {
        return Ok(methods::encode(&false));
    };
    let methods::SaveFile { name, blobs } = save;
    let named = move |error: std::io::Error| host_fault(format!("{name}: {error}"));
    let folder = path
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let mut file = rt::task::blocking(move || beside(&folder))
        .await
        .map_err(host_fault)?
        .map_err(named.clone())?;
    for digest in blobs {
        let Some(framed) = node
            .client
            .blob(abi::BlobId::Sha256(digest))
            .await
            .map_err(refused)?
        else {
            let hex = noded_client::hex::hex_encode(&digest);
            let missing = format!("the node holds no blob sha256:{hex}");
            return Err(wire::Error::new(refusal::NODE_FAILED, missing));
        };
        let body = noded_client::wire::unframe(&framed)
            .ok_or_else(|| host_fault("blob has no header"))?
            .to_vec();
        file = rt::task::blocking(move || {
            use std::io::Write as _;
            file.write_all(&body).map(|()| file)
        })
        .await
        .map_err(host_fault)?
        .map_err(named.clone())?;
    }
    rt::task::blocking(move || file.persist(path))
        .await
        .map_err(host_fault)?
        .map_err(|persisting| named(persisting.error))?;
    Ok(methods::encode(&true))
}

/// A new file in `folder` for a save to write, made as `File::create`
/// makes one (the umask applied, not a temporary file's 0600), and removed
/// if it is dropped before it takes the chosen file's place.
#[cfg(feature = "ui")]
fn beside(folder: &std::path::Path) -> std::io::Result<tempfile::NamedTempFile> {
    tempfile::Builder::new()
        .prefix(".ducktape-save-")
        .make_in(folder, |path| {
            std::fs::File::options()
                .write(true)
                .create_new(true)
                .open(path)
        })
}

/// An event the host makes for the view on [`handle`], counted in flight
/// until `event` answers: delivered at the view's next redraw, or nothing
/// when it answers none.
#[cfg(feature = "ui")]
pub(in crate::runtime) fn spawn_event(
    view: &View,
    event: impl std::future::Future<Output = Option<wire::Event>> + Send + 'static,
) {
    let slot = view.replies.admit();
    let replies = view.replies.clone();
    rt::task::spawn(async move {
        let _slot = slot;
        if let Some(event) = event.await {
            replies.event(event);
        }
    });
}

/// `op.put`'s task. A hash pass (each blob framed and hashed a part at a
/// time) makes the bundle's `Put`, which the seated key signs to the program
/// the node binds to the blobs role; a stream pass reads each blob again
/// into the node's blob upload after it, so no pass holds the bundle
/// whole. A file that changed between the passes no longer hashes to its
/// id, and the node's refusal of it is the answer. `sent` hears the bytes
/// of the blobs' bodies the upload has taken so far, each time it takes
/// one, the last once the upload is done. The answer, `Held` with each
/// blob's sha256 digest in order, waits for the network to hold the
/// bundle.
pub(super) async fn put(
    node: Node,
    blobs: Vec<Blob>,
    sent: impl Fn(u64) + Send + Sync + 'static,
) -> Answer {
    use futures::StreamExt as _;
    use sha2::Digest as _;
    let (mut ids, mut entries) = (Vec::with_capacity(blobs.len()), Vec::new());
    for blob in &blobs {
        let (mut hasher, mut framed) = (sha2::Sha256::new(), 0);
        let mut parts = blob.framed();
        while let Some(part) = parts.next().await {
            let part = part?;
            framed += part.len() as u64;
            hasher.update(&part);
        }
        let digest: [u8; 32] = hasher.finalize().into();
        ids.push(digest);
        entries.push(blob::contract::Entry {
            id: abi::BlobId::Sha256(digest),
            len: framed,
            kind: blob.kind.clone(),
        });
    }
    let roles = node.client.status().await.map_err(refused)?.roles;
    let target = roles
        .bound(abi::role::BLOBS)
        .map_err(|refusal| wire::Error::new(refusal.reason, refusal.sentence))?
        .clone();
    let op = blob::contract::Op::Put { blobs: entries };
    let frame =
        backend::seated_frame(&node.client, &node.network, &target, abi::encode(&op)).await?;
    // both followed from before the upload, so nothing between is missed
    let mut blocks = node.client.changes(noded::RECENT).await.map_err(refused)?;
    let mut bundles = node.client.changes(&target).await.map_err(refused)?;
    let unread = std::sync::Arc::new(std::sync::Mutex::new(None));
    let total: u64 = blobs.iter().map(Blob::len).sum();
    let sent = std::sync::Arc::new(sent);
    let stream = futures::stream::iter(blobs).flat_map({
        let (unread, sent) = (unread.clone(), sent.clone());
        let mut before = 0;
        move |blob| {
            // asked for the next blob: the upload took the ones before
            if before > 0 {
                sent(before);
            }
            before += blob.len();
            let unread = unread.clone();
            blob.framed().map(move |part| {
                part.map_err(|refusal| {
                    let error = std::io::Error::other(refusal.message.clone());
                    *unread.lock().expect("an unread blob") = Some(refusal);
                    error
                })
            })
        }
    });
    let mut heard = None;
    let uploaded = {
        let upload = std::pin::pin!(node.client.put_blobs(frame.encode(), stream));
        let lapse = std::pin::pin!(lapsed(
            &node,
            &mut blocks,
            frame.body.id(),
            frame.body.until(),
            &mut heard
        ));
        match futures::future::select(upload, lapse).await {
            futures::future::Either::Left((uploaded, _)) => uploaded,
            futures::future::Either::Right((lapsed, _)) => return Err(lapsed),
        }
    };
    if let Err(error) = uploaded {
        // a blob that could not be read again ended the upload: its refusal
        let unread = unread.lock().expect("an unread blob").take();
        return Err(unread.unwrap_or_else(|| refused(error)));
    }
    sent(total);
    let bundle = landed(
        &node,
        &mut blocks,
        frame.body.id(),
        frame.body.until(),
        heard,
    )
    .await?;
    drop(blocks);
    let key = blob::contract::bundle_key(bundle);
    while let Some(change) = bundles.next().await {
        let held = change
            .map_err(refused)?
            .writes
            .into_iter()
            .any(|(written, value)| {
                written == key && blob::contract::held_since(value.as_deref()).is_some()
            });
        if held {
            return Ok(methods::encode(&methods::Putting::Held(ids)));
        }
    }
    Err(wire::Error::new(
        refusal::NODE_FAILED,
        format!(
            "the node stopped telling the blobs program's changes before bundle {bundle} was held"
        ),
    ))
}

async fn lapsed(
    node: &Node,
    blocks: &mut (impl futures::Stream<Item = noded_client::Result<noded::Change>> + Unpin),
    id: [u8; 32],
    until: u64,
    heard: &mut Option<u64>,
) -> wire::Error {
    use futures::StreamExt as _;
    let ask = noded::TxAsk {
        tx: noded::TxRef::Hash(id),
        most: 0,
    };
    while let Some(change) = blocks.next().await {
        let height = match change {
            Ok(change) => change.height,
            Err(error) => return refused(error),
        };
        *heard = Some(height);
        if height <= until {
            continue;
        }
        let carried = match node.client.tx(&ask).await {
            Ok(located) => located.is_some_and(|located| located.tx.receipt.is_some()),
            Err(error) => return refused(error),
        };
        if carried {
            return futures::future::pending().await;
        }
        return wire::Error::new(
            abi::reason::ANCHOR,
            format!(
                "the upload was still going past block {until}, the last its frame may land at: attach it again"
            ),
        );
    }
    wire::Error::new(
        refusal::NODE_FAILED,
        "the node stopped telling its blocks before the upload ended",
    )
}

/// The bundle the block that carries frame `id` put, read off `blocks` (a
/// change a block): the program's refusal where it rejected the put, and
/// `anchor` once a block at `until` (the last that may carry the frame)
/// came without it.
async fn landed(
    node: &Node,
    blocks: &mut (impl futures::Stream<Item = noded_client::Result<noded::Change>> + Unpin),
    id: [u8; 32],
    until: u64,
    heard: Option<u64>,
) -> Result<u64, wire::Error> {
    use futures::StreamExt as _;
    let ask = noded::TxAsk {
        tx: noded::TxRef::Hash(id),
        most: 0,
    };
    let heard = futures::stream::iter(heard.map(Ok::<u64, noded_client::Error>));
    let mut heights = heard.chain(blocks.map(|change| change.map(|change| change.height)));
    while let Some(height) = heights.next().await {
        let height = height.map_err(refused)?;
        let located = node.client.tx(&ask).await.map_err(refused)?;
        if let Some(receipt) = located.and_then(|located| located.tx.receipt) {
            return match receipt.outcome {
                abi::Outcome::Applied { output } => abi::decode(&output).map_err(|refusal| {
                    wire::Error::new(view_wire::code::UNEXPECTED_REPLY, refusal.sentence)
                }),
                abi::Outcome::Rejected(refusal) => {
                    Err(wire::Error::new(refusal.reason, refusal.sentence))
                }
            };
        }
        if height >= until {
            return Err(wire::Error::new(
                abi::reason::ANCHOR,
                format!(
                    "no block carried the put by block {until}, the last its frame may land at"
                ),
            ));
        }
    }
    Err(wire::Error::new(
        refusal::NODE_FAILED,
        "the node stopped telling its blocks before the put landed",
    ))
}

/// The blob id a `blob.*` ask spells, `sha256:<hex>` or `sha1:<hex>`.
fn blob_id(ask: &[u8]) -> Result<abi::BlobId, wire::Error> {
    let id: String = methods::decode(ask).map_err(malformed)?;
    let (kind, hex) = id
        .split_once(':')
        .ok_or_else(|| malformed("id is `sha256:<hex>` or `sha1:<hex>`"))?;
    let digest = noded_client::hex::hex_decode(hex).map_err(malformed)?;
    Ok(match (kind, digest.len()) {
        ("sha256", 32) => abi::BlobId::Sha256(digest.try_into().expect("32 bytes")),
        ("sha1", 20) => abi::BlobId::Sha1(digest.try_into().expect("20 bytes")),
        _ => return Err(malformed("id is `sha256:<hex>` or `sha1:<hex>`")),
    })
}

/// `blob.manifest`: any blob, as `blob.get` reads any blob, read as a seat
/// reads it: code a listed program runs through the seats' cache
/// (`views::program_bytes`, kept under its id), any other blob (an upload,
/// a scheduled change's code) kept nowhere (`views::unkept_bytes`). The
/// view in its `ducktape.view` section or the blob itself for a view-only
/// entry, and
/// that view's manifest as `manifest_of` would read it. A blob is a view by
/// its manifest section, read or not: one this host cannot read is a view
/// with no manifest, never a program that draws nothing. The bytes stay on
/// the host. `None` is the node's word that it holds no such blob.
pub(super) fn blob_manifest(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        use backend::views::{Fetch, VIEW_SECTION, program_bytes, section, unkept_bytes};
        use view_wire::manifest::{MANIFEST_SECTION, read_manifest};
        let id = blob_id(&ask)?;
        let read = match node.roster().runs(&id) {
            true => program_bytes(&node.client, &id)
                .await
                .map(|(bytes, _)| bytes),
            false => unkept_bytes(&node.client, &id).await,
        };
        let bytes = match read {
            Ok(bytes) => bytes,
            Err(Fetch::NotHeld) => return Ok(methods::encode(&None::<methods::Manifested>)),
            Err(Fetch::Unreachable(reason)) => {
                return Err(wire::Error::new(refusal::RPC_CLIENT, reason));
            }
            Err(error) => return Err(host_fault(error)),
        };
        let view = section(&bytes, VIEW_SECTION)
            // no section: the blob is a view itself, or carries none
            .or_else(|| section(&bytes, MANIFEST_SECTION).map(|_| &bytes[..]))
            .map(|view| methods::CarriedView {
                size: view.len() as u64,
                manifest: read_manifest(view),
            });
        Ok(methods::encode(&Some(methods::Manifested {
            size: bytes.len() as u64,
            view,
        })))
    })
}

pub(super) fn blob_get(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let id = blob_id(&ask)?;
        // absent is `None`, never a refusal: the ask itself did not fail
        let Some(framed) = node.client.blob(id).await.map_err(refused)? else {
            return Ok(methods::encode(&None::<Vec<u8>>));
        };
        let body =
            noded_client::wire::unframe(&framed).ok_or_else(|| host_fault("blob has no header"))?;
        Ok(methods::encode(&Some(body.to_vec())))
    })
}

pub(super) fn status(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        if !ask.is_empty() {
            return Err(malformed("chain.status takes no payload"));
        }
        let status = node.client.status().await.map_err(refused)?;
        Ok(methods::encode(&methods::NodeStatus {
            chain_id: status.network,
            time: status.time,
            block_time_ms: status.block_time_ms,
            epoch_length: status.epoch_length,
            height: status.height,
            tip: status.tip,
            root: status.root.0,
            epoch: status.epoch,
            identity: status.identity,
            contract: status.contract,
        }))
    })
}

pub(super) fn network(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        if !ask.is_empty() {
            return Err(malformed("chain.network takes no payload"));
        }
        let network = node.client.network().await.map_err(|error| match error {
            noded_client::Error::Failed { status: 404, .. } => wire::Error::new(
                refusal::UNKNOWN_REQUEST,
                "This node doesn't report its validators' votes. Update the node.",
            ),
            error => refused(error),
        })?;
        let members = network
            .members
            .into_iter()
            .map(|peer| methods::Peer {
                key: peer.key,
                signed: peer.signed,
            })
            .collect();
        Ok(methods::encode(&methods::NetworkStatus {
            height: network.height,
            members,
        }))
    })
}

pub(super) fn invite(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let ttl = methods::decode::<methods::CreateInvite>(&ask)
            .map_err(malformed)?
            .ttl_days;
        if ttl == 0 {
            return Err(malformed("ttl_days must be a positive integer"));
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(host_fault)?
            .as_millis() as u64;
        let ask = crate::contract::MintInvite {
            client: node.client.clone(),
            ttl_days: ttl,
            now_ms,
        };
        let Some(minted) = crate::contract::os().mint_invite(ask) else {
            return Err(wire::Error::new(
                refusal::INVITE_UNSUPPORTED,
                "Only the machine running this node can mint its invites.",
            ));
        };
        minted.await
    })
}

fn block_of(block: noded::Finalized) -> methods::Block {
    methods::Block {
        height: block.height,
        id: block.id,
        parent: block.parent,
        time: block.time,
        epoch: block.epoch,
        proposer: block.proposer,
        txs: block.txs.into_iter().map(tx_of).collect(),
    }
}

fn tx_of(tx: noded::Tx) -> methods::Tx {
    methods::Tx {
        hash: tx.hash,
        signer: tx.signer,
        anchor: methods::Tip {
            height: tx.anchor.height,
            id: tx.anchor.id,
        },
        target: tx.target,
        size: tx.size,
        payload: tx.payload,
        receipt: tx.receipt.map(receipt_of),
    }
}

fn receipt_of(receipt: noded::Receipt) -> methods::Receipt {
    methods::Receipt {
        program: receipt.program,
        outcome: match receipt.outcome {
            abi::Outcome::Applied { output } => methods::Outcome::Applied { output },
            abi::Outcome::Rejected(refusal) => {
                methods::Outcome::Rejected(wire::Error::new(refusal.reason, refusal.sentence))
            }
        },
        events: receipt.events,
        nested: receipt.nested.into_iter().map(receipt_of).collect(),
    }
}

/// `chain.blocks`: a page of finalized blocks from the node's archive.
pub(super) fn blocks(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let methods::BlockPage {
            before,
            limit,
            most,
        } = methods::ChainBlocks::decode_request(&ask).map_err(malformed)?;
        let page = noded::Blocks {
            before,
            limit,
            most,
        };
        let page = node.client.blocks(&page).await.map_err(refused)?;
        Ok(methods::encode(&methods::BlockList {
            blocks: page.blocks.into_iter().map(block_of).collect(),
            listed_from: page.listed_from,
        }))
    })
}

/// `chain.block`: one finalized block by height or id, if the node has it.
pub(super) fn block(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let methods::BlockAsk { by, most } =
            methods::ChainBlock::decode_request(&ask).map_err(malformed)?;
        let by = match by {
            methods::BlockRef::Height(height) => noded::BlockRef::Height(height),
            methods::BlockRef::Id(id) => noded::BlockRef::Id(id),
        };
        let ask = noded::BlockAsk { by, most };
        let block = node.client.block(&ask).await.map_err(refused)?;
        Ok(methods::encode(&block.map(block_of)))
    })
}

pub(super) fn tx(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let ask: methods::TxAsk = methods::decode(&ask).map_err(malformed)?;
        let tx = match ask.tx {
            methods::TxRef::At { height, position } => noded::TxRef::At { height, position },
            methods::TxRef::Hash(hash) => noded::TxRef::Hash(hash),
        };
        let ask = noded::TxAsk { tx, most: ask.most };
        let located = node.client.tx(&ask).await.map_err(refused)?;
        Ok(methods::encode(&located.map(|located| methods::Located {
            height: located.height,
            position: located.position,
            time: located.time,
            tx: tx_of(located.tx),
        })))
    })
}

#[cfg(test)]
mod tests;
